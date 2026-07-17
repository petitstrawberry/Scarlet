use core::arch::asm;

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extension {
    Base = 0x10,
    SetTimer = 0x00,
    ConsolePutChar = 0x01,
    ConsoleGetChar = 0x02,
    DebugConsole = 0x4442_434e,
    Timer = 0x5449_4d45,
    Ipi = 0x7350_49,
    Rfence = 0x5246_4e43,
    Hsm = 0x4853_4d,
    Srst = 0x5352_5354,
    Pmu = 0x504d_55,
}

/// Result returned by an SBI v0.2-or-newer function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SbiRet {
    /// SBI status code returned in `a0`.
    pub error: isize,
    /// SBI result value returned in `a1`.
    pub value: usize,
}

/// Standard SBI error returned by an SBI v0.2-or-newer function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbiError {
    Failed,
    NotSupported,
    InvalidParam,
    Denied,
    InvalidAddress,
    AlreadyAvailable,
    AlreadyStarted,
    AlreadyStopped,
    Unknown(isize),
}

impl SbiError {
    /// Converts an SBI error status into its kernel representation.
    ///
    /// # Arguments
    ///
    /// * `error` - The signed SBI error status returned in `a0`.
    ///
    /// # Returns
    ///
    /// The matching error, preserving unrecognized status codes explicitly.
    pub fn from_error(error: isize) -> Self {
        match error {
            -1 => Self::Failed,
            -2 => Self::NotSupported,
            -3 => Self::InvalidParam,
            -4 => Self::Denied,
            -5 => Self::InvalidAddress,
            -6 => Self::AlreadyAvailable,
            -7 => Self::AlreadyStarted,
            -8 => Self::AlreadyStopped,
            _ => Self::Unknown(error),
        }
    }
}

const SBI_BASE_PROBE_FID: usize = 3;
const SBI_RFENCE_REMOTE_SFENCE_VMA_FID: usize = 1;
const SBI_RFENCE_REMOTE_SFENCE_VMA_ASID_FID: usize = 2;

/// Invokes an SBI function that accepts up to two arguments.
///
/// # Arguments
///
/// * `extension` - SBI extension identifier.
/// * `function` - Function identifier within `extension`.
/// * `arg0` - Value passed in `a0`.
/// * `arg1` - Value passed in `a1`.
///
/// # Returns
///
/// The value returned in `a1`, or the SBI error returned in `a0`.
#[inline(never)]
#[unsafe(no_mangle)]
pub fn sbi_call(
    extension: Extension,
    function: usize,
    arg0: usize,
    arg1: usize,
) -> Result<usize, SbiError> {
    let ret = sbi_call_v02_raw(extension, function, [arg0, arg1, 0, 0, 0, 0]);
    sbi_result_value(ret)
}

/// Builds the six SBI argument registers for a Base extension probe.
pub(crate) const fn base_probe_request_args(extension: Extension) -> [usize; 6] {
    [extension as usize, 0, 0, 0, 0, 0]
}

/// Builds the six SBI argument registers for `remote_sfence_vma`.
pub(crate) const fn remote_sfence_vma_request_args(
    hart_mask: usize,
    hart_mask_base: usize,
    start: usize,
    size: usize,
) -> [usize; 6] {
    [hart_mask, hart_mask_base, start, size, 0, 0]
}

/// Builds the six SBI argument registers for `remote_sfence_vma_asid`.
pub(crate) const fn remote_sfence_vma_asid_request_args(
    hart_mask: usize,
    hart_mask_base: usize,
    start: usize,
    size: usize,
    asid: usize,
) -> [usize; 6] {
    [hart_mask, hart_mask_base, start, size, asid, 0]
}

/// Returns the SBI hart-list encoding that targets every supervisor-available hart.
pub(crate) const fn all_harts_sentinel() -> (usize, usize) {
    (0, usize::MAX)
}

/// Probes whether an SBI extension is available.
///
/// # Arguments
///
/// * `extension` - Extension to probe through the SBI Base extension.
///
/// # Returns
///
/// `Ok(true)` when the extension is available, `Ok(false)` when it is absent,
/// or the SBI failure reported by the Base extension.
pub(crate) fn sbi_probe_extension(extension: Extension) -> Result<bool, SbiError> {
    let ret = sbi_call_v02_raw(
        Extension::Base,
        SBI_BASE_PROBE_FID,
        base_probe_request_args(extension),
    );
    sbi_result_value(ret).map(|value| value != 0)
}

/// Sends a synchronous RFENCE request to flush an address range for all ASIDs.
///
/// # Arguments
///
/// * `hart_mask` - SBI hart bit-vector.
/// * `hart_mask_base` - Base hart ID for `hart_mask`.
/// * `start` - Start virtual address, or zero for the full address space.
/// * `size` - Range size, or zero for the full address space.
///
/// # Returns
///
/// `Ok(())` only after SBI reports that every target hart completed the request.
pub fn remote_sfence_vma(
    hart_mask: usize,
    hart_mask_base: usize,
    start: usize,
    size: usize,
) -> Result<(), SbiError> {
    require_rfence()?;
    sbi_result_unit(sbi_call_v02_raw(
        Extension::Rfence,
        SBI_RFENCE_REMOTE_SFENCE_VMA_FID,
        remote_sfence_vma_request_args(hart_mask, hart_mask_base, start, size),
    ))
}

/// Sends a synchronous RFENCE request to flush an address range for one ASID.
///
/// # Arguments
///
/// * `hart_mask` - SBI hart bit-vector.
/// * `hart_mask_base` - Base hart ID for `hart_mask`.
/// * `start` - Start virtual address, or zero for the full address space.
/// * `size` - Range size, or zero for the full address space.
/// * `asid` - Address-space identifier whose translations must be invalidated.
///
/// # Returns
///
/// `Ok(())` only after SBI reports that every target hart completed the request.
pub(crate) fn remote_sfence_vma_asid(
    hart_mask: usize,
    hart_mask_base: usize,
    start: usize,
    size: usize,
    asid: usize,
) -> Result<(), SbiError> {
    require_rfence()?;
    sbi_result_unit(sbi_call_v02_raw(
        Extension::Rfence,
        SBI_RFENCE_REMOTE_SFENCE_VMA_ASID_FID,
        remote_sfence_vma_asid_request_args(hart_mask, hart_mask_base, start, size, asid),
    ))
}

/// Sends a synchronous full-address-space RFENCE request to all available harts.
///
/// # Arguments
///
/// * `start` - Start virtual address, or zero for the full address space.
/// * `size` - Range size, or zero for the full address space.
///
/// # Returns
///
/// `Ok(())` only after SBI reports that every available hart completed the request.
pub fn remote_sfence_vma_all_harts(start: usize, size: usize) -> Result<(), SbiError> {
    let (hart_mask, hart_mask_base) = all_harts_sentinel();
    remote_sfence_vma(hart_mask, hart_mask_base, start, size)
}

/// Sends a synchronous full-address-space RFENCE request for one ASID to all harts.
///
/// # Arguments
///
/// * `start` - Start virtual address, or zero for the full address space.
/// * `size` - Range size, or zero for the full address space.
/// * `asid` - Address-space identifier whose translations must be invalidated.
///
/// # Returns
///
/// `Ok(())` only after SBI reports that every available hart completed the request.
pub(crate) fn remote_sfence_vma_asid_all_harts(
    start: usize,
    size: usize,
    asid: usize,
) -> Result<(), SbiError> {
    let (hart_mask, hart_mask_base) = all_harts_sentinel();
    remote_sfence_vma_asid(hart_mask, hart_mask_base, start, size, asid)
}

/// Writes one character through the legacy SBI console extension.
pub fn sbi_console_putchar(c: char) {
    let _ = sbi_call(Extension::ConsolePutChar, 0, c as usize, 0);
}

/// Reads one character through the legacy SBI console extension.
pub fn sbi_console_getchar() -> char {
    match sbi_call(Extension::ConsoleGetChar, 0, 0, 0) {
        Ok(c) => c as u8 as char,
        Err(_) => '\0',
    }
}

/// Writes one byte through the SBI debug-console extension.
pub fn sbi_debug_console_write_byte(c: char) {
    let _ = sbi_call(Extension::DebugConsole, 0x2, c as usize, 0);
}

/// Programs the SBI timer.
pub fn sbi_set_timer(stime_value: u64) {
    let _ = sbi_call(Extension::Timer, 0, stime_value as usize, 0);
}

/// Sends an SBI software interrupt to the selected harts.
pub fn sbi_send_ipi(hart_mask: usize, hart_mask_base: usize) {
    let _ = sbi_call(Extension::Ipi, 0, hart_mask, hart_mask_base);
}

/// Requests an SBI system reset and does not return.
pub fn sbi_system_reset(reset_type: u32, reset_reason: u32) -> ! {
    let _ = sbi_call(
        Extension::Srst,
        0,
        reset_type as usize,
        reset_reason as usize,
    );
    loop {}
}

fn require_rfence() -> Result<(), SbiError> {
    if sbi_probe_extension(Extension::Rfence)? {
        Ok(())
    } else {
        Err(SbiError::NotSupported)
    }
}

fn sbi_result_value(ret: SbiRet) -> Result<usize, SbiError> {
    match ret.error {
        0 => Ok(ret.value),
        error => Err(SbiError::from_error(error)),
    }
}

fn sbi_result_unit(ret: SbiRet) -> Result<(), SbiError> {
    sbi_result_value(ret).map(|_| ())
}

/// Executes an SBI v0.2-or-newer ECALL and returns its `sbiret` registers.
fn sbi_call_v02_raw(extension: Extension, function: usize, args: [usize; 6]) -> SbiRet {
    let mut a0 = args[0];
    let mut a1 = args[1];

    // SAFETY: the register assignment follows the SBI v0.2 calling convention;
    // `clobber_abi("C")` declares all caller-saved registers that firmware may clobber.
    unsafe {
        asm!(
            "ecall",
            inout("a0") a0,
            inout("a1") a1,
            in("a2") args[2],
            in("a3") args[3],
            in("a4") args[4],
            in("a5") args[5],
            in("a6") function,
            in("a7") extension as usize,
            clobber_abi("C"),
            options(nostack),
        );
    }

    SbiRet {
        error: a0 as isize,
        value: a1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_unknown_error_maps_without_panic() {
        assert_eq!(SbiError::from_error(-9), SbiError::Unknown(-9));
        assert_eq!(SbiError::from_error(123), SbiError::Unknown(123));
    }

    #[test_case]
    fn test_sbi_result_conversion_surfaces_errors() {
        assert_eq!(
            sbi_result_value(SbiRet {
                error: -2,
                value: usize::MAX,
            }),
            Err(SbiError::NotSupported)
        );
        assert_eq!(
            sbi_result_unit(SbiRet {
                error: 0,
                value: 123,
            }),
            Ok(())
        );
    }

    #[test_case]
    fn test_base_probe_request_args() {
        assert_eq!(
            base_probe_request_args(Extension::Rfence),
            [Extension::Rfence as usize, 0, 0, 0, 0, 0]
        );
    }

    #[test_case]
    fn test_remote_sfence_vma_request_args() {
        assert_eq!(
            remote_sfence_vma_request_args(0b101, 4, 0x1000, 0x2000),
            [0b101, 4, 0x1000, 0x2000, 0, 0]
        );
    }

    #[test_case]
    fn test_remote_sfence_vma_asid_request_args() {
        assert_eq!(
            remote_sfence_vma_asid_request_args(0b11, 2, 0x3000, 0x4000, 7),
            [0b11, 2, 0x3000, 0x4000, 7, 0]
        );
    }

    #[test_case]
    fn test_all_harts_sentinel() {
        assert_eq!(all_harts_sentinel(), (0, usize::MAX));
    }
}
