use font8x8::{BASIC_FONTS, UnicodeFonts};
use limine::framebuffer::{Framebuffer, MemoryModel};
use spin::Mutex;

const FONT_WIDTH: usize = 8;
const FONT_HEIGHT: usize = 8;

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
        }
    }

    fn init(&mut self, framebuffer: &Framebuffer<'_>) {
        if framebuffer.memory_model() != MemoryModel::RGB {
            return;
        }

        let bytes_per_pixel = (framebuffer.bpp() as usize).div_ceil(8);
        if bytes_per_pixel != 3 && bytes_per_pixel != 4 {
            return;
        }

        self.addr = framebuffer.addr() as usize;
        self.width = framebuffer.width() as usize;
        self.height = framebuffer.height() as usize;
        self.pitch = framebuffer.pitch() as usize;
        self.bytes_per_pixel = bytes_per_pixel;
        self.red_mask_size = framebuffer.red_mask_size();
        self.red_mask_shift = framebuffer.red_mask_shift();
        self.green_mask_size = framebuffer.green_mask_size();
        self.green_mask_shift = framebuffer.green_mask_shift();
        self.blue_mask_size = framebuffer.blue_mask_size();
        self.blue_mask_shift = framebuffer.blue_mask_shift();
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.initialized = true;
        self.clear_screen();
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
        if self.cursor_x + FONT_WIDTH > self.width {
            self.new_line();
        }
        if self.cursor_y + FONT_HEIGHT > self.height {
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
                self.put_pixel(self.cursor_x + col_idx, self.cursor_y + row_idx, r, g, b);
            }
        }

        self.cursor_x += FONT_WIDTH;
    }

    fn new_line(&mut self) {
        self.cursor_x = 0;
        self.cursor_y += FONT_HEIGHT;
        if self.cursor_y + FONT_HEIGHT > self.height {
            self.clear_screen();
        }
    }

    fn clear_screen(&mut self) {
        if !self.initialized {
            return;
        }

        let total_bytes = self.pitch.saturating_mul(self.height);
        for offset in 0..total_bytes {
            unsafe {
                core::ptr::write_volatile((self.addr + offset) as *mut u8, 0);
            }
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    fn put_pixel(&self, x: usize, y: usize, r: u8, g: u8, b: u8) {
        if x >= self.width || y >= self.height || self.bytes_per_pixel == 0 {
            return;
        }

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
    }
}

static EARLY_CONSOLE: Mutex<FramebufferConsole> = Mutex::new(FramebufferConsole::new());

pub fn init(framebuffer: &Framebuffer<'_>) {
    let mut console = EARLY_CONSOLE.lock();
    if console.initialized {
        return;
    }
    console.init(framebuffer);
}

pub fn putc(c: u8) {
    EARLY_CONSOLE.lock().write_byte(c);
}

pub fn write_str(s: &str) {
    for byte in s.bytes() {
        putc(byte);
    }
}

pub fn is_initialized() -> bool {
    EARLY_CONSOLE.lock().initialized
}
