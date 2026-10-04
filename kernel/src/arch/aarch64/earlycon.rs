use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[repr(usize)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EarlyUartKind {
    None = 0,
    Pl011 = 1,
    QcomGeni = 2,
    Ns16550 = 3,
}

impl EarlyUartKind {
    fn from_raw(value: usize) -> Self {
        match value {
            1 => Self::Pl011,
            2 => Self::QcomGeni,
            3 => Self::Ns16550,
            _ => Self::None,
        }
    }
}

static EARLY_UART_KIND: AtomicUsize = AtomicUsize::new(EarlyUartKind::None as usize);
static EARLY_UART_VADDR: AtomicUsize = AtomicUsize::new(0);
static NS16550_REG_SHIFT: AtomicUsize = AtomicUsize::new(0);
static NS16550_IO_WIDTH: AtomicUsize = AtomicUsize::new(1);
static BOOT_SELECTED_UART: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "limine")]
static PENDING_QCOM_GENI_PADDR: AtomicUsize = AtomicUsize::new(0);

fn publish_uart(kind: EarlyUartKind, vaddr: usize) {
    EARLY_UART_VADDR.store(vaddr, Ordering::Relaxed);
    EARLY_UART_KIND.store(kind as usize, Ordering::Release);
    crate::log::register_emergency_putc(emergency_uart_putc);
}

/// Return whether the boot contract selected an early UART.
///
/// Runtime console discovery uses this to avoid turning an unrelated serial
/// device into the kernel console merely because its driver was registered.
pub(crate) fn has_active_uart() -> bool {
    BOOT_SELECTED_UART.load(Ordering::Acquire)
}

fn try_uart_putc(c: u8) -> bool {
    let kind = EarlyUartKind::from_raw(EARLY_UART_KIND.load(Ordering::Acquire));
    let uart = EARLY_UART_VADDR.load(Ordering::Relaxed);
    if kind == EarlyUartKind::None || uart == 0 {
        return false;
    }

    match kind {
        EarlyUartKind::None => return false,
        EarlyUartKind::Pl011 => {
            const UART_DR: usize = 0x000;
            const UART_FR: usize = 0x018;
            const UART_FR_TXFF: u32 = 1 << 5;

            // SAFETY: registration publishes an FDT-validated PL011 MMIO page
            // only after its Device mapping is active in Scarlet's HHDM.
            unsafe {
                while ((uart + UART_FR) as *const u32).read_volatile() & UART_FR_TXFF != 0 {
                    core::hint::spin_loop();
                }
                ((uart + UART_DR) as *mut u32).write_volatile(c as u32);
            }
        }
        EarlyUartKind::QcomGeni => {
            return crate::drivers::uart::qcom_geni::early_write_byte(uart, c);
        }
        EarlyUartKind::Ns16550 => {
            return ns16550_write_byte(
                uart,
                NS16550_REG_SHIFT.load(Ordering::Relaxed),
                NS16550_IO_WIDTH.load(Ordering::Relaxed),
                c,
            );
        }
    }

    true
}

fn emergency_uart_putc(c: u8) {
    let kind = EarlyUartKind::from_raw(EARLY_UART_KIND.load(Ordering::Acquire));
    let uart = EARLY_UART_VADDR.load(Ordering::Relaxed);
    if kind == EarlyUartKind::None || uart == 0 {
        return;
    }

    match kind {
        EarlyUartKind::None => {}
        EarlyUartKind::Pl011 | EarlyUartKind::Ns16550 => {
            let _ = try_uart_putc(c);
        }
        EarlyUartKind::QcomGeni => {
            let _ = crate::drivers::uart::qcom_geni::try_emergency_write_byte(uart, c);
        }
    }
}

fn ns16550_write_byte(uart: usize, reg_shift: usize, io_width: usize, c: u8) -> bool {
    const LSR: usize = 5;
    const LSR_THRE: u8 = 1 << 5;
    // A missing or stalled device must not hang boot or emergency output.
    const TX_POLL_LIMIT: usize = 100_000;

    // SAFETY: boot registration validates the register layout and publishes
    // this address only after its complete Device mapping is active.
    unsafe {
        for _ in 0..TX_POLL_LIMIT {
            let lsr = uart + (LSR << reg_shift);
            let status = if io_width == 4 {
                (lsr as *const u32).read_volatile() as u8
            } else {
                (lsr as *const u8).read_volatile()
            };
            if status & LSR_THRE != 0 {
                if io_width == 4 {
                    (uart as *mut u32).write_volatile(c as u32);
                } else {
                    (uart as *mut u8).write_volatile(c);
                }
                return true;
            }
            core::hint::spin_loop();
        }
    }
    false
}

/// Validated register geometry of an FDT-selected NS16550 UART.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Ns16550Layout {
    reg_shift: usize,
    io_width: usize,
}

impl Ns16550Layout {
    pub(crate) fn new(reg_shift: usize, io_width: usize) -> Option<Self> {
        // Support byte and little-endian word accesses with 1/2/4/8-byte
        // register spacing. Word accesses must remain naturally aligned.
        if reg_shift > 3 || !matches!(io_width, 1 | 4) || io_width > (1 << reg_shift) {
            return None;
        }
        Some(Self {
            reg_shift,
            io_width,
        })
    }

    pub(crate) fn required_size(self) -> usize {
        (5 << self.reg_shift) + self.io_width
    }

    pub(crate) fn alignment(self) -> usize {
        self.io_width
    }
}

/// Activate an FDT-validated NS16550 after its Device mapping is installed.
#[cfg(feature = "linux-boot")]
pub(crate) fn register_linux_boot_ns16550(
    paddr: usize,
    layout: Ns16550Layout,
    direct_map: crate::vm::direct_map::DirectMapWindow,
) {
    NS16550_REG_SHIFT.store(layout.reg_shift, Ordering::Relaxed);
    NS16550_IO_WIDTH.store(layout.io_width, Ordering::Relaxed);
    publish_uart(
        EarlyUartKind::Ns16550,
        direct_map
            .phys_to_virt(crate::mem::address::PhysAddr::new(paddr as u64))
            .expect("early UART is outside the direct map")
            .as_usize(),
    );
    BOOT_SELECTED_UART.store(true, Ordering::Release);
}

/// Write one byte to the active early UART or framebuffer fallback.
///
/// # Arguments
///
/// * `c` - Byte to emit.
pub fn early_putc(c: u8) {
    if try_uart_putc(c) {
        #[cfg(feature = "linux-boot")]
        if crate::earlyfb::is_redirection_enabled() {
            crate::earlyfb::putc(c);
        }
        return;
    }

    crate::earlyfb::putc(c);
}

/// Registers an FDT-validated PL011 physical address for early output.
///
/// # Arguments
///
/// * `paddr` - Physical base of the PL011 register window.
#[cfg(feature = "linux-boot")]
pub(crate) fn register_linux_boot_pl011(
    paddr: usize,
    direct_map: crate::vm::direct_map::DirectMapWindow,
) {
    publish_uart(
        EarlyUartKind::Pl011,
        direct_map
            .phys_to_virt(crate::mem::address::PhysAddr::new(paddr as u64))
            .expect("early UART is outside the direct map")
            .as_usize(),
    );
    BOOT_SELECTED_UART.store(true, Ordering::Release);
}

/// Prepare an FDT-selected Qualcomm GENI UART for the Limine page-table handoff.
///
/// The UART remains inactive until [`activate_after_boot_page_table_switch`]
/// confirms that Scarlet's Device-typed HHDM mapping is live.
///
/// # Arguments
///
/// * `paddr` - Physical base of the GENI serial-engine register window.
#[cfg(feature = "limine")]
pub(crate) fn prepare_limine_qcom_geni(paddr: usize) {
    PENDING_QCOM_GENI_PADDR.store(paddr, Ordering::Release);
}

/// Activate a prepared early UART after Scarlet installs its boot page table.
///
/// # Returns
///
/// `true` when a pending Qualcomm GENI UART was activated.
pub(crate) fn activate_after_boot_page_table_switch(
    direct_map: crate::vm::direct_map::DirectMapWindow,
) -> bool {
    #[cfg(feature = "limine")]
    {
        let paddr = PENDING_QCOM_GENI_PADDR.load(Ordering::Acquire);
        if paddr != 0 {
            crate::earlyfb::deactivate();
            publish_uart(
                EarlyUartKind::QcomGeni,
                direct_map
                    .phys_to_virt(crate::mem::address::PhysAddr::new(paddr as u64))
                    .expect("early UART is outside the direct map")
                    .as_usize(),
            );
            BOOT_SELECTED_UART.store(true, Ordering::Release);
            for &byte in b"\x1b[2J\x1b[H" {
                emergency_uart_putc(byte);
            }
            return true;
        }
    }

    false
}

/// Move Qualcomm GENI early output to the runtime driver's ioremap address.
///
/// # Arguments
///
/// * `vaddr` - Device-typed virtual base returned by `ioremap`.
pub(crate) fn register_runtime_qcom_geni(vaddr: usize) {
    if !has_active_uart()
        || EarlyUartKind::from_raw(EARLY_UART_KIND.load(Ordering::Acquire))
            != EarlyUartKind::QcomGeni
    {
        return;
    }
    crate::earlyfb::deactivate();
    publish_uart(EarlyUartKind::QcomGeni, vaddr);
}

/// Initialize the framebuffer fallback when no early UART is active.
pub fn early_console_init() {
    if EarlyUartKind::from_raw(EARLY_UART_KIND.load(Ordering::Acquire)) != EarlyUartKind::None {
        return;
    }
    if crate::earlyfb::is_initialized() {
        return;
    }

    #[cfg(feature = "limine")]
    {
        let Some(response) = crate::boot::limine::FRAMEBUFFER_REQUEST.response() else {
            return;
        };
        let Some(framebuffer) = response.framebuffers().iter().next() else {
            return;
        };

        crate::earlyfb::init(framebuffer);
    }
}

/// Write a string through the architecture early console.
///
/// # Arguments
///
/// * `s` - String to emit.
pub fn early_console_write(s: &str) {
    for byte in s.bytes() {
        early_putc(byte);
    }
}

#[cfg(test)]
mod tests {
    use super::{EarlyUartKind, Ns16550Layout, ns16550_write_byte};

    #[test_case]
    fn unknown_uart_kind_is_safely_disabled() {
        assert_eq!(EarlyUartKind::from_raw(0), EarlyUartKind::None);
        assert_eq!(EarlyUartKind::from_raw(usize::MAX), EarlyUartKind::None);
        assert_eq!(EarlyUartKind::from_raw(1), EarlyUartKind::Pl011);
        assert_eq!(EarlyUartKind::from_raw(2), EarlyUartKind::QcomGeni);
        assert_eq!(EarlyUartKind::from_raw(3), EarlyUartKind::Ns16550);
    }

    #[test_case]
    fn ns16550_geometry_rejects_unsupported_and_unaligned_accesses() {
        assert_eq!(Ns16550Layout::new(0, 1).unwrap().required_size(), 6);
        assert_eq!(Ns16550Layout::new(2, 4).unwrap().required_size(), 24);
        assert!(Ns16550Layout::new(0, 4).is_none());
        assert!(Ns16550Layout::new(2, 2).is_none());
        assert!(Ns16550Layout::new(usize::MAX, 1).is_none());
    }

    #[test_case]
    fn ns16550_uses_the_declared_access_width_and_register_spacing() {
        let mut bytes = [0u8; 8];
        bytes[5] = 0x20;
        assert!(ns16550_write_byte(bytes.as_mut_ptr() as usize, 0, 1, b'A'));
        assert_eq!(bytes, [b'A', 0, 0, 0, 0, 0x20, 0, 0]);

        let mut words = [0u32; 8];
        words[5] = 0x20;
        assert!(ns16550_write_byte(words.as_mut_ptr() as usize, 2, 4, b'B'));
        assert_eq!(words, [b'B' as u32, 0, 0, 0, 0, 0x20, 0, 0]);
    }

    #[test_case]
    fn ns16550_stalled_transmitter_returns_without_writing() {
        let mut bytes = [0u8; 8];
        assert!(!ns16550_write_byte(bytes.as_mut_ptr() as usize, 0, 1, b'A'));
        assert_eq!(bytes, [0; 8]);
    }
}
