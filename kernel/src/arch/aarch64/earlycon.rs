use core::sync::atomic::{AtomicUsize, Ordering};

static EARLY_UART_PADDR: AtomicUsize = AtomicUsize::new(0);

pub fn early_putc(c: u8) {
    let uart_paddr = EARLY_UART_PADDR.load(Ordering::Acquire);
    if uart_paddr != 0 {
        const UART_DR: usize = 0x000;
        const UART_FR: usize = 0x018;
        const UART_FR_TXFF: u32 = 1 << 5;
        let uart = crate::environment::SCARLET_HHDM_BASE + uart_paddr;

        // SAFETY: linux-boot publishes an FDT-described PL011 MMIO page only
        // after installing its Device mapping in the temporary HHDM.
        unsafe {
            while ((uart + UART_FR) as *const u32).read_volatile() & UART_FR_TXFF != 0 {
                core::hint::spin_loop();
            }
            ((uart + UART_DR) as *mut u32).write_volatile(c as u32);
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
pub(crate) fn register_linux_boot_pl011(paddr: usize) {
    EARLY_UART_PADDR.store(paddr, Ordering::Release);
}

pub fn early_console_init() {
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

pub fn early_console_write(s: &str) {
    crate::earlyfb::write_str(s);
}
