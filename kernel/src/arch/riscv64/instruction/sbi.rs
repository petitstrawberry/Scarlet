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

/// Hart State Management (HSM) extension functions
///
/// The HSM extension allows the supervisor OS to start and stop harts,
/// and query their state.

/// HSM hart states
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HartState {
    Started = 0,
    Stopped = 1,
    StartPending = 2,
    StopPending = 3,
    Suspended = 4,
    ResumePending = 5,
    Unknown = 6,
}

impl HartState {
    pub fn from_value(value: usize) -> Self {
        match value {
            0 => HartState::Started,
            1 => HartState::Stopped,
            2 => HartState::StartPending,
            3 => HartState::StopPending,
            4 => HartState::Suspended,
            5 => HartState::ResumePending,
            _ => HartState::Unknown,
        }
    }
}

/// Get the current state of a hart
///
/// # Arguments
/// * `hartid` - The target hart ID
///
/// # Returns
/// * `Ok(HartState)` - The current state of the hart
/// * `Err(SbiError)` - SBI call error
pub fn sbi_hsm_hart_get_status(hartid: usize) -> Result<HartState, SbiError> {
    match sbi_call(Extension::Hsm, 0, hartid, 0) {
        Ok(state) => Ok(HartState::from_value(state)),
        Err(e) => Err(e),
    }
}

/// Start a halted hart at a given physical address
///
/// # Arguments
/// * `hartid` - The target hart ID
/// * `start_addr` - Physical address where the hart should start execution
/// * `opaque` - A value that will be passed in register a1 to the hart
///
/// # Returns
/// * `Ok(())` - Hart start request was successful
/// * `Err(SbiError)` - SBI call error
pub fn sbi_hsm_hart_start(
    hartid: usize,
    start_addr: usize,
    opaque: usize,
) -> Result<(), SbiError> {
    // sbi_call expects arg0 in a0, arg1 in a1
    // We need to pass hartid in a0, start_addr in a1, and opaque in a2
    // Our current sbi_call only takes 2 args, so we need to call it directly
    let error: usize;
    let _value: usize;

    unsafe {
        asm!(
            "ecall",
            inout("a0") hartid => error,
            inout("a1") start_addr => _value,
            inout("a2") opaque => _,
            inout("a3") 0 => _,
            inout("a4") 0 => _,
            inout("a5") 0 => _,
            inout("a6") 0usize => _,
            inout("a7") Extension::Hsm as usize => _,
            clobber_abi("C"),
            options(nostack),
        );
    }

    match error {
        0 => Ok(()),
        error_code if error_code as isize >= -8 && error_code as isize <= -1 => Err(SbiError::from_error(error_code)),
        _ => Err(SbiError::Failed),
    }
}

/// Stop the calling hart
///
/// This function does not return
///
/// # Arguments
/// * `opaque` - A value to be retrieved by a subsequent sbi_hsm_hart_get_status call
pub fn sbi_hsm_hart_stop(opaque: usize) -> ! {
    let _ = sbi_call(Extension::Hsm, 1, opaque, 0);
    loop {}
}

/// Request that a hart suspend itself
///
/// # Arguments
/// * `suspend_type` - The type of suspension
/// * `resume_addr` - Physical address where the hart should resume
/// * `opaque` - A value to be retrieved by a subsequent sbi_hsm_hart_get_status call
///
/// # Returns
/// * `Ok(())` - Suspension was successful
/// * `Err(SbiError)` - SBI call error
pub fn sbi_hsm_hart_suspend(
    suspend_type: usize,
    resume_addr: usize,
    opaque: usize,
) -> Result<(), SbiError> {
    let error: usize;
    let _value: usize;

    unsafe {
        asm!(
            "ecall",
            inout("a0") suspend_type => error,
            inout("a1") resume_addr => _value,
            inout("a2") opaque => _,
            inout("a3") 0 => _,
            inout("a4") 0 => _,
            inout("a5") 0 => _,
            inout("a6") 1usize => _,
            inout("a7") Extension::Hsm as usize => _,
            clobber_abi("C"),
            options(nostack),
        );
    }

    match error {
        0 => Ok(()),
        error_code if error_code as isize >= -8 && error_code as isize <= -1 => Err(SbiError::from_error(error_code)),
        _ => Err(SbiError::Failed),
    }
}
