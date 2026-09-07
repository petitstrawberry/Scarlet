use core::sync::atomic::{AtomicUsize, Ordering};

#[repr(usize)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EarlyUartKind {
    None = 0,
    Pl011 = 1,
    QcomGeni = 2,
}

impl EarlyUartKind {
    fn from_raw(value: usize) -> Self {
        match value {
            1 => Self::Pl011,
            2 => Self::QcomGeni,
            _ => Self::None,
        }
    }
}

static EARLY_UART_KIND: AtomicUsize = AtomicUsize::new(EarlyUartKind::None as usize);
static EARLY_UART_VADDR: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "limine")]
static PENDING_QCOM_GENI_PADDR: AtomicUsize = AtomicUsize::new(0);

fn publish_uart(kind: EarlyUartKind, vaddr: usize) {
    EARLY_UART_VADDR.store(vaddr, Ordering::Relaxed);
    EARLY_UART_KIND.store(kind as usize, Ordering::Release);
    crate::log::register_emergency_putc(emergency_uart_putc);
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
        EarlyUartKind::Pl011 => {
            let _ = try_uart_putc(c);
        }
        EarlyUartKind::QcomGeni => {
            let _ = crate::drivers::uart::qcom_geni::try_emergency_write_byte(uart, c);
        }
    }
}

/// Write one byte to the active early UART or framebuffer fallback.
///
/// # Arguments
///
/// * `c` - Byte to emit.
pub fn early_putc(c: u8) {
    if try_uart_putc(c) {
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
pub(crate) fn register_linux_boot_pl011(paddr: usize) {
    publish_uart(
        EarlyUartKind::Pl011,
        crate::environment::SCARLET_HHDM_BASE + paddr,
    );
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
pub(crate) fn activate_after_boot_page_table_switch() -> bool {
    #[cfg(feature = "limine")]
    {
        let paddr = PENDING_QCOM_GENI_PADDR.load(Ordering::Acquire);
        if paddr != 0 {
            crate::earlyfb::deactivate();
            publish_uart(
                EarlyUartKind::QcomGeni,
                crate::environment::SCARLET_HHDM_BASE + paddr,
            );
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
    use super::EarlyUartKind;

    #[test_case]
    fn unknown_uart_kind_is_safely_disabled() {
        assert_eq!(EarlyUartKind::from_raw(0), EarlyUartKind::None);
        assert_eq!(EarlyUartKind::from_raw(usize::MAX), EarlyUartKind::None);
        assert_eq!(EarlyUartKind::from_raw(1), EarlyUartKind::Pl011);
        assert_eq!(EarlyUartKind::from_raw(2), EarlyUartKind::QcomGeni);
    }
}
