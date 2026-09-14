use crate::sync::IrqSpinLock;
#[cfg(feature = "linux-boot")]
use core::sync::atomic::AtomicUsize;
use core::sync::atomic::{AtomicBool, Ordering};
use font8x8::{BASIC_FONTS, UnicodeFonts};
#[cfg(feature = "limine")]
use limine::framebuffer::{FRAMEBUFFER_RGB, Framebuffer};

const FONT_WIDTH: usize = 8;
const FONT_HEIGHT: usize = 8;
const FONT_SCALE: usize = 2;
const GLYPH_WIDTH: usize = FONT_WIDTH * FONT_SCALE;
const GLYPH_HEIGHT: usize = FONT_HEIGHT * FONT_SCALE;
// DIAGNOSTIC: Keep early-console output on the framebuffer, but temporarily
// stop mirroring normal kernel, TTY, and breadcrumb output into it.
const DIAGNOSTIC_ENABLE_FBCON_REDIRECTION: bool = false;
static REDIRECTION_ENABLED: AtomicBool = AtomicBool::new(DIAGNOSTIC_ENABLE_FBCON_REDIRECTION);
static KEEP_BOOT_CONSOLE: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "linux-boot")]
static EMERGENCY_ADDR: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "linux-boot")]
static EMERGENCY_WIDTH: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "linux-boot")]
static EMERGENCY_HEIGHT: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "linux-boot")]
static EMERGENCY_PITCH: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "linux-boot")]
static EMERGENCY_ROTATED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "linux-boot")]
static EMERGENCY_RED_LOW: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "linux-boot")]
static EMERGENCY_SURFACE_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "linux-boot")]
static EMERGENCY_CURSOR: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy)]
struct FramebufferConsole {
    addr: usize,
    width: usize,
    height: usize,
    pitch: usize,
    bytes_per_pixel: usize,
    red_mask_size: u8,
    red_mask_shift: u8,
    green_mask_size: u8,
    green_mask_shift: u8,
    blue_mask_size: u8,
    blue_mask_shift: u8,
    cursor_x: usize,
    cursor_y: usize,
    initialized: bool,
    rotated: bool,
    opaque: bool,
}

impl FramebufferConsole {
    const fn new() -> Self {
        Self {
            addr: 0,
            width: 0,
            height: 0,
            pitch: 0,
            bytes_per_pixel: 0,
            red_mask_size: 0,
            red_mask_shift: 0,
            green_mask_size: 0,
            green_mask_shift: 0,
            blue_mask_size: 0,
            blue_mask_shift: 0,
            cursor_x: 0,
            cursor_y: 0,
            initialized: false,
            rotated: false,
            opaque: false,
        }
    }

    #[cfg(feature = "limine")]
    fn init(&mut self, framebuffer: &Framebuffer) {
        if framebuffer.memory_model != FRAMEBUFFER_RGB {
            return;
        }

        let bytes_per_pixel = (framebuffer.bpp as usize).div_ceil(8);
        if bytes_per_pixel != 3 && bytes_per_pixel != 4 {
            return;
        }

        self.addr = framebuffer.address() as usize;
        self.width = framebuffer.width as usize;
        self.height = framebuffer.height as usize;
        self.pitch = framebuffer.pitch as usize;
        self.bytes_per_pixel = bytes_per_pixel;
        self.red_mask_size = framebuffer.red_mask_size;
        self.red_mask_shift = framebuffer.red_mask_shift;
        self.green_mask_size = framebuffer.green_mask_size;
        self.green_mask_shift = framebuffer.green_mask_shift;
        self.blue_mask_size = framebuffer.blue_mask_size;
        self.blue_mask_shift = framebuffer.blue_mask_shift;
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.initialized = true;
        self.clear_screen();
    }

    fn surface_height(&self) -> usize {
        if self.rotated {
            self.width
        } else {
            self.height
        }
    }

    fn write_byte(&mut self, byte: u8) {
        if !self.initialized {
            return;
        }

        match byte {
            b'\r' => self.cursor_x = 0,
            b'\n' => self.new_line(),
            b'\t' => {
                for _ in 0..4 {
                    self.write_byte(b' ');
                }
            }
            0x20..=0x7e => self.draw_char(byte as char),
            _ => self.draw_char('?'),
        }
    }

    fn draw_char(&mut self, ch: char) {
        if self.cursor_x + GLYPH_WIDTH > self.width {
            self.new_line();
        }
        if self.cursor_y + GLYPH_HEIGHT > self.height {
            self.clear_screen();
        }

        let glyph = BASIC_FONTS.get(ch).or_else(|| BASIC_FONTS.get('?'));
        let Some(glyph) = glyph else {
            return;
        };

        for (row_idx, row) in glyph.iter().enumerate() {
            for col_idx in 0..FONT_WIDTH {
                let bit = (row >> col_idx) & 1;
                let (r, g, b) = if bit != 0 {
                    (0xff, 0xff, 0xff)
                } else {
                    (0x00, 0x00, 0x00)
                };

                let base_x = self.cursor_x + (col_idx * FONT_SCALE);
                let base_y = self.cursor_y + (row_idx * FONT_SCALE);
                for dy in 0..FONT_SCALE {
                    for dx in 0..FONT_SCALE {
                        self.put_pixel(base_x + dx, base_y + dy, r, g, b);
                    }
                }
            }
        }

        self.cursor_x += GLYPH_WIDTH;
    }

    fn new_line(&mut self) {
        self.cursor_x = 0;
        self.cursor_y += GLYPH_HEIGHT;
        if self.cursor_y + GLYPH_HEIGHT > self.height {
            self.clear_screen();
        }
    }

    fn clear_screen(&mut self) {
        if !self.initialized {
            return;
        }

        let total_bytes = self.pitch.saturating_mul(self.surface_height());
        if self.opaque {
            for offset in (0..total_bytes).step_by(4) {
                unsafe {
                    core::ptr::write_volatile((self.addr + offset) as *mut u32, 0xff000000);
                }
            }
        } else {
            for offset in 0..total_bytes {
                unsafe {
                    core::ptr::write_volatile((self.addr + offset) as *mut u8, 0);
                }
            }
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    fn put_pixel(&self, x: usize, y: usize, r: u8, g: u8, b: u8) {
        if x >= self.width || y >= self.height || self.bytes_per_pixel == 0 {
            return;
        }

        let (x, y) = if self.rotated {
            (y, self.width - 1 - x)
        } else {
            (x, y)
        };
        let offset = y
            .saturating_mul(self.pitch)
            .saturating_add(x.saturating_mul(self.bytes_per_pixel));
        let pixel = self.pack_color(r, g, b).to_le_bytes();
        for (idx, byte) in pixel.iter().enumerate().take(self.bytes_per_pixel) {
            unsafe {
                core::ptr::write_volatile((self.addr + offset + idx) as *mut u8, *byte);
            }
        }
    }

    fn pack_component(value: u8, mask_size: u8, mask_shift: u8) -> u32 {
        if mask_size == 0 {
            return 0;
        }
        let max = (1u32 << mask_size) - 1;
        (((value as u32) * max + 127) / 255) << mask_shift
    }

    fn pack_color(&self, r: u8, g: u8, b: u8) -> u32 {
        Self::pack_component(r, self.red_mask_size, self.red_mask_shift)
            | Self::pack_component(g, self.green_mask_size, self.green_mask_shift)
            | Self::pack_component(b, self.blue_mask_size, self.blue_mask_shift)
            | if self.opaque { 0xff000000 } else { 0 }
    }
}

static EARLY_CONSOLE: IrqSpinLock<FramebufferConsole> = IrqSpinLock::new(FramebufferConsole::new());

pub fn console_lock_addr() -> usize {
    &EARLY_CONSOLE as *const _ as usize
}

#[cfg(feature = "limine")]
pub fn init(framebuffer: &Framebuffer) {
    let mut console = EARLY_CONSOLE.lock();
    if console.initialized {
        return;
    }
    console.init(framebuffer);
}

/// Initialize an already validated, NonCacheable-mapped Linux boot surface.
#[cfg(feature = "linux-boot")]
pub(crate) fn init_linux_framebuffer(
    addr: usize,
    width: usize,
    height: usize,
    pitch: usize,
    red_low: bool,
    rotated: bool,
) {
    let mut console = EARLY_CONSOLE.lock();
    if console.initialized {
        return;
    }
    console.addr = addr;
    console.width = if rotated { height } else { width };
    console.height = if rotated { width } else { height };
    console.pitch = pitch;
    console.bytes_per_pixel = 4;
    console.red_mask_size = 8;
    console.red_mask_shift = if red_low { 0 } else { 16 };
    console.green_mask_size = 8;
    console.green_mask_shift = 8;
    console.blue_mask_size = 8;
    console.blue_mask_shift = if red_low { 16 } else { 0 };
    console.rotated = rotated;
    console.opaque = true;
    console.initialized = true;
    console.clear_screen();
    EMERGENCY_WIDTH.store(console.width, Ordering::Relaxed);
    EMERGENCY_HEIGHT.store(console.height, Ordering::Relaxed);
    EMERGENCY_PITCH.store(pitch, Ordering::Relaxed);
    EMERGENCY_ROTATED.store(rotated, Ordering::Relaxed);
    EMERGENCY_RED_LOW.store(red_low, Ordering::Relaxed);
    EMERGENCY_ADDR.store(addr, Ordering::Release);
    REDIRECTION_ENABLED.store(true, Ordering::Release);
    crate::log::register_emergency_putc(emergency_framebuffer_putc);
}

/// Panic output uses its own atomic cursor, without the normal console lock.
#[cfg(feature = "linux-boot")]
fn emergency_framebuffer_putc(byte: u8) {
    let sequence = EMERGENCY_SURFACE_SEQUENCE.load(Ordering::Acquire);
    if sequence & 1 != 0 {
        return;
    }
    let addr = EMERGENCY_ADDR.load(Ordering::Acquire);
    if addr == 0 || byte == b'\r' {
        return;
    }
    let width = EMERGENCY_WIDTH.load(Ordering::Relaxed);
    let height = EMERGENCY_HEIGHT.load(Ordering::Relaxed);
    let pitch = EMERGENCY_PITCH.load(Ordering::Relaxed);
    let rotated = EMERGENCY_ROTATED.load(Ordering::Relaxed);
    let red_low = EMERGENCY_RED_LOW.load(Ordering::Relaxed);
    core::sync::atomic::fence(Ordering::Acquire);
    if EMERGENCY_SURFACE_SEQUENCE.load(Ordering::Relaxed) != sequence {
        return;
    }
    let columns = width / GLYPH_WIDTH;
    let rows = height / GLYPH_HEIGHT;
    if columns == 0 || rows == 0 {
        return;
    }
    if byte == b'\n' {
        let _ = EMERGENCY_CURSOR.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cursor| {
            Some((cursor / columns + 1) * columns % (columns * rows))
        });
        return;
    }
    let cell = EMERGENCY_CURSOR.fetch_add(1, Ordering::Relaxed) % (columns * rows);
    let mut console = FramebufferConsole {
        addr,
        width,
        height,
        pitch,
        bytes_per_pixel: 4,
        red_mask_size: 8,
        red_mask_shift: if red_low { 0 } else { 16 },
        green_mask_size: 8,
        green_mask_shift: 8,
        blue_mask_size: 8,
        blue_mask_shift: if red_low { 16 } else { 0 },
        cursor_x: (cell % columns) * GLYPH_WIDTH,
        cursor_y: (cell / columns) * GLYPH_HEIGHT,
        initialized: true,
        rotated,
        opaque: true,
    };
    console.draw_char(if byte.is_ascii_graphic() || byte == b' ' {
        byte as char
    } else {
        '?'
    });
}

pub fn putc(c: u8) {
    EARLY_CONSOLE.lock().write_byte(c);
}

pub fn write_str(s: &str) {
    for byte in s.bytes() {
        putc(byte);
    }
}

/// Write a whole string while holding the console lock once.
///
/// Unlike `write_str` (which acquires the lock per byte and therefore
/// interleaves with concurrent writers), this holds `EARLY_CONSOLE` across
/// the entire string so the output is atomic w.r.t. other CPUs. Use for
/// diagnostic dumps that must remain readable under SMP contention.
pub fn write_raw(s: &str) {
    let mut console = EARLY_CONSOLE.lock();
    for byte in s.bytes() {
        console.write_byte(byte);
    }
}

pub fn is_initialized() -> bool {
    EARLY_CONSOLE.lock().initialized
}

pub(crate) fn is_redirection_enabled() -> bool {
    REDIRECTION_ENABLED.load(Ordering::Acquire)
}

pub fn deactivate() {
    if keep_boot_console() {
        return;
    }
    let mut console = EARLY_CONSOLE.lock();
    console.initialized = false;
}

/// Retain boot-console output when the display driver can preserve its surface.
/// This is an explicit diagnostic option, independent of distribution policy.
pub fn keep_boot_console() -> bool {
    KEEP_BOOT_CONSOLE.load(Ordering::Acquire)
}

pub(crate) fn configure(cmdline: &str) {
    KEEP_BOOT_CONSOLE.store(
        cmdline
            .split_whitespace()
            .any(|word| word == "keep_bootcon"),
        Ordering::Release,
    );
}

/// Previous boot-console surface retained for a failed native display handoff.
/// The native driver must keep the original backing alive until rollback is
/// no longer possible. This token does not own the framebuffer allocation.
#[must_use]
pub struct EarlyFramebufferSurface {
    console: FramebufferConsole,
}

#[cfg(feature = "linux-boot")]
fn publish_emergency_surface(console: &FramebufferConsole) {
    EMERGENCY_SURFACE_SEQUENCE.fetch_add(1, Ordering::AcqRel);
    EMERGENCY_ADDR.store(0, Ordering::Release);
    EMERGENCY_WIDTH.store(console.width, Ordering::Relaxed);
    EMERGENCY_HEIGHT.store(console.height, Ordering::Relaxed);
    EMERGENCY_PITCH.store(console.pitch, Ordering::Relaxed);
    EMERGENCY_ROTATED.store(console.rotated, Ordering::Relaxed);
    EMERGENCY_RED_LOW.store(console.red_mask_shift == 0, Ordering::Relaxed);
    EMERGENCY_ADDR.store(console.addr, Ordering::Release);
    EMERGENCY_SURFACE_SEQUENCE.fetch_add(1, Ordering::Release);
}

/// Move boot and emergency output onto a native display's linear surface.
/// Contents and the logical dimensions must be preserved by the caller; the
/// existing cursor is retained. No framebuffer or console policy is selected.
///
/// # Safety
/// `addr` must map the entire `pitch * height` surface with noncacheable or
/// device attributes and remain valid while either console can use it,
/// including after [`deactivate`]. The caller must preserve the old surface
/// until rollback completes and outstanding emergency writers finish, and
/// must serialize display ownership changes.
pub unsafe fn replace_surface(
    addr: usize,
    width: usize,
    height: usize,
    pitch: usize,
    red_low: bool,
    rotated: bool,
) -> Result<Option<EarlyFramebufferSurface>, &'static str> {
    let mut console = EARLY_CONSOLE.lock();
    if !console.initialized {
        return Ok(None);
    }
    let logical_width = if rotated { height } else { width };
    let logical_height = if rotated { width } else { height };
    if addr == 0
        || width == 0
        || height == 0
        || addr & 3 != 0
        || pitch & 3 != 0
        || width.checked_mul(4).is_none_or(|row| row > pitch)
        || pitch
            .checked_mul(height)
            .and_then(|size| addr.checked_add(size))
            .is_none()
        || console.width != logical_width
        || console.height != logical_height
    {
        return Err("native early console surface layout mismatch");
    }
    let previous = EarlyFramebufferSurface { console: *console };
    console.addr = addr;
    console.pitch = pitch;
    console.bytes_per_pixel = 4;
    console.red_mask_size = 8;
    console.green_mask_size = 8;
    console.blue_mask_size = 8;
    console.red_mask_shift = if red_low { 0 } else { 16 };
    console.green_mask_shift = 8;
    console.blue_mask_shift = if red_low { 16 } else { 0 };
    console.rotated = rotated;
    console.opaque = true;
    #[cfg(feature = "linux-boot")]
    publish_emergency_surface(&console);
    Ok(Some(previous))
}

/// Restore boot and emergency output after a native display rolls back.
///
/// # Safety
/// The token's original mapping must still be live and displayed. The caller
/// must serialize this with display ownership and console-surface changes.
pub unsafe fn restore_surface(surface: EarlyFramebufferSurface) {
    let mut console = EARLY_CONSOLE.lock();
    *console = surface.console;
    #[cfg(feature = "linux-boot")]
    publish_emergency_surface(&console);
}

/// Rebind the early framebuffer after the page-table handoff.
/// Both mappings are explicit; virtual-address ordering does not identify which
/// mapping a pointer belongs to. The boot path calls this once before rendering.
pub fn relocate_direct_map(
    old: crate::vm::direct_map::DirectMapWindow,
    new: crate::vm::direct_map::DirectMapWindow,
) {
    use crate::mem::address::VirtAddr;
    let mut console = EARLY_CONSOLE.lock();
    if !console.initialized {
        return;
    }
    let bytes = console
        .pitch
        .checked_mul(console.surface_height())
        .and_then(|size| size.checked_sub(1))
        .expect("invalid framebuffer byte range");
    let old_base = VirtAddr::new(console.addr);
    let paddr = old
        .virt_to_phys(old_base)
        .expect("framebuffer is outside the boot mapping");
    let last_paddr = old
        .virt_to_phys(
            old_base
                .checked_add(bytes)
                .expect("framebuffer VA overflows"),
        )
        .expect("framebuffer end is outside the boot mapping");
    new.phys_to_virt(last_paddr)
        .expect("framebuffer end is outside the runtime mapping");
    console.addr = new
        .phys_to_virt(paddr)
        .expect("framebuffer is outside the runtime mapping")
        .as_usize();
    #[cfg(feature = "linux-boot")]
    if EMERGENCY_ADDR.load(Ordering::Acquire) != 0 {
        EMERGENCY_ADDR.store(console.addr, Ordering::Release);
    }
}
