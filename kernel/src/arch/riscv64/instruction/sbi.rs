use core::arch::asm;

pub enum Extension {
    Base = 0x10,
    SetTimer = 0x00,
    ConsolePutChar = 0x01,
    ConsoleGetChar = 0x02,
    DebugConsole = 0x4442434e,
    Timer = 0x54494d45,
    Ipi = 0x735049,
    Rfence = 0x52464e43,
    Hsm = 0x48534d,
    Srst = 0x53525354,
    Pmu = 0x504d55,
}

pub struct SbiRet {
    pub error: usize,
    pub value: usize,
}

pub enum SbiError {
    Failed = -1,
    NotSupported = -2,
    InvalidParam = -3,
    Denied = -4,
    InvalidAddress = -5,
    AlreadyAvailable = -6,
    AlreadyStarted = -7,
    AlreadyStopped = -8,
}

impl SbiError {
    pub fn from_error(error: usize) -> SbiError {
        let error = error as isize;
        match error {
            -1 => SbiError::Failed,
            -2 => SbiError::NotSupported,
            -3 => SbiError::InvalidParam,
            -4 => SbiError::Denied,
            -5 => SbiError::InvalidAddress,
            -6 => SbiError::AlreadyAvailable,
            -7 => SbiError::AlreadyStarted,
            -8 => SbiError::AlreadyStopped,
            _ => panic!("Invalid SBI error code"),
        }
    }
}

/// More robust SBI call implementation with additional safety measures
#[inline(never)]
#[unsafe(no_mangle)]
pub fn sbi_call(
    extension: Extension,
    function: usize,
    arg0: usize,
    arg1: usize,
) -> Result<usize, SbiError> {
    let error: usize;
    let ret: usize;

    unsafe {
        asm!(
            "ecall",
            inout("a0") arg0 => error,
            inout("a1") arg1 => ret,
            inout("a2") 0 => _,
            inout("a3") 0 => _,
            inout("a4") 0 => _,
            inout("a5") 0 => _,
            inout("a6") function => _,
            inout("a7") extension as usize => _,
            clobber_abi("C"),
            options(nostack),
        );
    }

    match error {
        0 => Ok(ret),
        error_code if error_code <= 8 => Err(SbiError::from_error(error_code)),
        _ => Err(SbiError::Failed),
    }
}

/// SBI call with three arguments (a0, a1, a2).
///
/// Required by extensions like HSM `hart_start` which takes hartid, start_addr,
/// and opaque as three separate arguments.
#[inline(never)]
pub fn sbi_call3(
    extension: Extension,
    function: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
) -> Result<usize, SbiError> {
    let error: usize;
    let ret: usize;

    unsafe {
        asm!(
            "ecall",
            inout("a0") arg0 => error,
            inout("a1") arg1 => ret,
            inout("a2") arg2 => _,
            inout("a3") 0 => _,
            inout("a4") 0 => _,
            inout("a5") 0 => _,
            inout("a6") function => _,
            inout("a7") extension as usize => _,
            clobber_abi("C"),
            options(nostack),
        );
    }

    match error {
        0 => Ok(ret),
        error_code if error_code <= 8 => Err(SbiError::from_error(error_code)),
        _ => Err(SbiError::Failed),
    }
}

pub fn sbi_console_putchar(c: char) {
    let _ = sbi_call(Extension::ConsolePutChar, 0, c as usize, 0);
}

pub fn sbi_console_getchar() -> char {
    let ret = sbi_call(Extension::ConsoleGetChar, 0, 0, 0);
    match ret {
        Ok(c) => c as u8 as char,
        Err(_) => '\0',
    }
}

pub fn sbi_debug_console_write_byte(c: char) {
    let _ = sbi_call(Extension::DebugConsole, 0x2, c as usize, 0);
}

pub fn sbi_set_timer(stime_value: u64) {
    let _ = sbi_call(Extension::Timer, 0, stime_value as usize, 0);
}

pub fn sbi_system_reset(reset_type: u32, reset_reason: u32) -> ! {
    let _ = sbi_call(
        Extension::Srst,
        0,
        reset_type as usize,
        reset_reason as usize,
    );
    loop {}
}

/// Start a hart using the SBI HSM (Hart State Management) extension.
///
/// # Arguments
///
/// * `hartid` - The hart to start
/// * `start_addr` - Physical address of the entry point for the hart
/// * `opaque` - An opaque value passed to the hart in register `a1`
///
/// # Returns
///
/// `Ok(0)` on success, or an `SbiError` on failure
pub fn sbi_hart_start(hartid: usize, start_addr: usize, opaque: usize) -> Result<usize, SbiError> {
    sbi_call3(Extension::Hsm, 0, hartid, start_addr, opaque)
}

/// Send an inter-processor interrupt (IPI) to a set of harts.
///
/// # Arguments
///
/// * `hart_mask` - Bitmask of harts to receive the IPI (relative to `hart_mask_base`)
/// * `hart_mask_base` - The starting hart ID for the mask
///
/// # Returns
///
/// `Ok(0)` on success, or an `SbiError` on failure
pub fn sbi_send_ipi(hart_mask: usize, hart_mask_base: usize) -> Result<usize, SbiError> {
    sbi_call(Extension::Ipi, 0, hart_mask, hart_mask_base)
}

/// Clear the supervisor software interrupt pending bit (SSIP) in `sip`.
///
/// This must be called after handling a software interrupt (IPI) to acknowledge it.
pub fn sbi_clear_ipi() {
    unsafe {
        asm!("csrc sip, {0}", in(reg) 0x2); // Clear SSIP (bit 1)
    }
}
