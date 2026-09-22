//! Small C runtime facilities without process-global locale or calendar state.
//!
//! Native clocks expose microsecond-quantized nanoseconds. We query them
//! directly because Scarlet std's infallible `SystemTime::now` panics when UTC
//! has not been initialized, whereas the C functions must report an error.
//! Sleep uses Rust std after validating its finite Native u64-nanosecond range.
//! Entropy calls always require a registered source; they never use the Native
//! emergency PRNG. The legacy entropy syscall has only an undifferentiated
//! failure sentinel, so failures after local validation are reported as EIO.

#[cfg(target_os = "scarlet")]
use std::ffi::{c_char, c_uint, c_void};
use std::ffi::{c_int, c_long};
#[cfg(any(test, target_os = "scarlet"))]
use std::time::Duration;

#[cfg(any(test, target_os = "scarlet"))]
use scarlet_abi::{ERRNO_EINVAL, ERRNO_EIO, fs::ERRNO_EOVERFLOW};
#[cfg(target_os = "scarlet")]
use scarlet_abi::{GET_RANDOM_FLAG_REQUIRE_ENTROPY, Syscall, fs::ERRNO_EFAULT};

#[cfg(any(test, target_os = "scarlet"))]
use crate::Timespec;

pub const CLOCK_REALTIME: c_int = 0;
pub const CLOCK_MONOTONIC: c_int = 1;
pub const GRND_NONBLOCK: u32 = 1;
pub const GRND_RANDOM: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Timeval {
    pub tv_sec: i64,
    pub tv_usec: c_long,
}

/// Clear only the IEEE sign bit, including for signed zeros and NaN payloads.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn fabs(value: f64) -> f64 {
    f64::from_bits(value.to_bits() & 0x7fff_ffff_ffff_ffff)
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn fabsf(value: f32) -> f32 {
    f32::from_bits(value.to_bits() & 0x7fff_ffff)
}

#[cfg(any(test, target_os = "scarlet"))]
fn checked_clock(clock: c_int) -> Result<(), c_int> {
    match clock {
        CLOCK_REALTIME | CLOCK_MONOTONIC => Ok(()),
        _ => Err(ERRNO_EINVAL),
    }
}

#[cfg(any(test, target_os = "scarlet"))]
fn clock_result(nanoseconds: u64) -> Result<Duration, c_int> {
    if nanoseconds == u64::MAX {
        Err(ERRNO_EIO)
    } else {
        Ok(Duration::from_nanos(nanoseconds))
    }
}

#[cfg(any(test, target_os = "scarlet"))]
fn duration_timespec(duration: Duration) -> Result<Timespec, c_int> {
    Ok(Timespec {
        tv_sec: i64::try_from(duration.as_secs()).map_err(|_| ERRNO_EOVERFLOW)?,
        tv_nsec: c_long::from(duration.subsec_nanos()),
    })
}

#[cfg(any(test, target_os = "scarlet"))]
fn sleep_duration(time: Timespec) -> Result<Duration, c_int> {
    if time.tv_sec < 0 || !(0..1_000_000_000).contains(&time.tv_nsec) {
        return Err(ERRNO_EINVAL);
    }
    let duration = Duration::new(time.tv_sec as u64, time.tv_nsec as u32);
    // Scarlet std saturates larger durations; rejecting them preserves the C
    // request instead of silently shortening a sleep.
    if duration.as_nanos() > u128::from(u64::MAX) {
        Err(ERRNO_EOVERFLOW)
    } else {
        Ok(duration)
    }
}

#[cfg(any(test, target_os = "scarlet"))]
fn random_arguments(null_buffer: bool, count: usize, flags: u32) -> Result<(), c_int> {
    if flags & !(GRND_NONBLOCK | GRND_RANDOM) != 0 {
        Err(ERRNO_EINVAL)
    } else if flags != 0 {
        // Native entropy providers may block, and there is no separate
        // /dev/random pool. Neither Linux flag can be implemented faithfully.
        Err(scarlet_abi::ERRNO_EOPNOTSUPP)
    } else if count > isize::MAX as usize {
        Err(ERRNO_EINVAL)
    } else if null_buffer && count != 0 {
        Err(scarlet_abi::fs::ERRNO_EFAULT)
    } else {
        Ok(())
    }
}

#[cfg(any(test, target_os = "scarlet"))]
fn random_result(value: usize, requested: usize) -> Result<usize, c_int> {
    if value > requested || value == usize::MAX {
        Err(ERRNO_EIO)
    } else {
        Ok(value)
    }
}

#[cfg(target_os = "scarlet")]
fn clock_duration(clock: c_int) -> Result<Duration, c_int> {
    checked_clock(clock)?;
    let syscall = if clock == CLOCK_REALTIME {
        Syscall::SystemTime
    } else {
        Syscall::MonotonicTime
    };
    // SAFETY: both clocks take no pointers and use the full-width scalar ABI.
    clock_result(unsafe { scarlet_sys::syscall_u64(syscall, [0; 6]) })
}

/// # Safety
/// A non-null output must address a writable time_t.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn time(output: *mut i64) -> i64 {
    let value = match clock_duration(CLOCK_REALTIME).and_then(duration_timespec) {
        Ok(time) => time.tv_sec,
        Err(error) => return crate::fail(error) as i64,
    };
    if !output.is_null() {
        // SAFETY: the caller supplies a live writable time_t.
        unsafe { *output = value };
    }
    value
}

/// # Safety
/// output must address a writable timespec.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clock_gettime(clock: c_int, output: *mut Timespec) -> c_int {
    if let Err(error) = checked_clock(clock) {
        return crate::fail(error);
    }
    if output.is_null() {
        return crate::fail(ERRNO_EFAULT);
    }
    match clock_duration(clock).and_then(duration_timespec) {
        Ok(value) => {
            // SAFETY: the caller supplies a live writable timespec.
            unsafe { *output = value };
            0
        }
        Err(error) => crate::fail(error),
    }
}

/// # Safety
/// A non-null output must address a writable timespec.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clock_getres(clock: c_int, output: *mut Timespec) -> c_int {
    if let Err(error) = checked_clock(clock) {
        return crate::fail(error);
    }
    if !output.is_null() {
        // Native time::current_time_ns multiplies whole microseconds by 1000;
        // this is clock quantization, not a claim about RTC accuracy.
        unsafe {
            *output = Timespec {
                tv_sec: 0,
                tv_nsec: 1000,
            }
        };
    }
    0
}

/// # Safety
/// output must address a writable timeval. timezone must be null.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gettimeofday(output: *mut Timeval, timezone: *mut c_void) -> c_int {
    if !timezone.is_null() {
        return crate::fail(ERRNO_EINVAL);
    }
    if output.is_null() {
        return crate::fail(ERRNO_EFAULT);
    }
    match clock_duration(CLOCK_REALTIME).and_then(duration_timespec) {
        Ok(value) => {
            // SAFETY: the caller supplies a live writable timeval.
            unsafe {
                *output = Timeval {
                    tv_sec: value.tv_sec,
                    tv_usec: value.tv_nsec / 1000,
                }
            };
            0
        }
        Err(error) => crate::fail(error),
    }
}

/// # Safety
/// requested must be readable and aligned. A non-null remaining pointer must
/// be writable; Native Sleep currently completes the interval without EINTR,
/// and remaining is left unchanged on success or a validation error.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nanosleep(requested: *const Timespec, _remaining: *mut Timespec) -> c_int {
    if requested.is_null() {
        return crate::fail(ERRNO_EFAULT);
    }
    // SAFETY: requested is a valid timespec supplied by the caller.
    let duration = match sleep_duration(unsafe { *requested }) {
        Ok(value) => value,
        Err(error) => return crate::fail(error),
    };
    // Check the kernel's absolute timer range too. Native Sleep adds current
    // uptime to the relative duration with saturation.
    let now = match clock_duration(CLOCK_MONOTONIC) {
        Ok(value) => value,
        Err(error) => return crate::fail(error),
    };
    if (now.as_nanos() + duration.as_nanos()) > u128::from(u64::MAX) {
        return crate::fail(ERRNO_EOVERFLOW);
    }
    std::thread::sleep(duration);
    0
}

/// # Safety
/// buffer must be writable for count bytes, unless count is zero.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getrandom(buffer: *mut c_void, count: usize, flags: c_uint) -> isize {
    if let Err(error) = random_arguments(buffer.is_null(), count, flags) {
        return crate::fail(error) as isize;
    }
    if count == 0 {
        return 0;
    }
    // SAFETY: the caller supplies the buffer. Requiring entropy prevents the
    // kernel's emergency XorShift generator from satisfying this C API.
    let value = unsafe {
        scarlet_sys::syscall3(
            Syscall::GetRandom,
            buffer as usize,
            count,
            GET_RANDOM_FLAG_REQUIRE_ENTROPY,
        )
    };
    match random_result(value, count) {
        Ok(value) => value as isize,
        Err(error) => crate::fail(error) as isize,
    }
}

/// # Safety
/// buffer must be writable for count bytes, unless count is zero.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getentropy(buffer: *mut c_void, count: usize) -> c_int {
    if count > 256 {
        return crate::fail(ERRNO_EIO);
    }
    if let Err(error) = random_arguments(buffer.is_null(), count, 0) {
        return crate::fail(error);
    }
    let mut written = 0;
    while written < count {
        // SAFETY: each successful partial count advances within the caller's
        // original buffer. count is at most 256 and checked above.
        let value =
            unsafe { getrandom(buffer.cast::<u8>().add(written).cast(), count - written, 0) };
        if value < 0 {
            return -1;
        }
        if value == 0 {
            return crate::fail(ERRNO_EIO);
        }
        written += value as usize;
    }
    0
}

#[cfg(target_os = "scarlet")]
fn diagnostic(mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: bytes is live and readable; fd 2 is borrowed, never closed.
        let written = unsafe {
            scarlet_sys::syscall3(
                Syscall::StreamWriteWithStatus,
                2,
                bytes.as_ptr() as usize,
                bytes.len(),
            )
        };
        if written == 0 || written > bytes.len() {
            break;
        }
        bytes = &bytes[written..];
    }
}

/// Abnormal process termination. Native ABI has no POSIX SIGABRT delivery;
/// exit status 134 terminates the thread group without flushing or destructors.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn abort() -> ! {
    // SAFETY: process-wide termination takes only a scalar exit status. This
    // avoids std abort's intrinsic potentially lowering back to this symbol.
    unsafe { scarlet_sys::syscall1(Syscall::ExitGroup, 134) };
    loop {
        // Retry termination if a future kernel ever returns from ExitGroup.
        unsafe { scarlet_sys::syscall1(Syscall::ExitGroup, 134) };
        std::hint::spin_loop();
    }
}

/// # Safety
/// expression, file, and function must each be readable NUL-terminated strings.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __assert_fail(
    expression: *const c_char,
    file: *const c_char,
    line: c_uint,
    function: *const c_char,
) -> ! {
    diagnostic(b"Assertion failed: ");
    // SAFETY: these NUL-terminated strings come from the C assert macro.
    unsafe {
        diagnostic(std::ffi::CStr::from_ptr(expression).to_bytes());
        diagnostic(b" (");
        diagnostic(std::ffi::CStr::from_ptr(file).to_bytes());
        diagnostic(b":");
    }
    let mut digits = [0u8; 10];
    let mut position = digits.len();
    let mut value = line;
    loop {
        position -= 1;
        digits[position] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    diagnostic(&digits[position..]);
    diagnostic(b", ");
    // SAFETY: function is a readable C string supplied by assert.
    unsafe { diagnostic(std::ffi::CStr::from_ptr(function).to_bytes()) };
    diagnostic(b")\n");
    abort()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_value_preserves_all_non_sign_bits() {
        for bits in [
            0,
            1,
            0x3ff0_0000_0000_0000,
            0x7ff0_0000_0000_0000,
            0x7ff0_0000_0000_0042,
            0x7ff8_0000_0000_0042,
        ] {
            assert_eq!(fabs(f64::from_bits(bits)).to_bits(), bits);
            assert_eq!(fabs(f64::from_bits(bits | 1 << 63)).to_bits(), bits);
        }
        for bits in [0, 1, 0x3f80_0000, 0x7f80_0000, 0x7f80_0042, 0x7fc0_0042] {
            assert_eq!(fabsf(f32::from_bits(bits)).to_bits(), bits);
            assert_eq!(fabsf(f32::from_bits(bits | 1 << 31)).to_bits(), bits);
        }
    }

    #[test]
    fn clock_ids_and_unavailable_sentinel_are_checked() {
        assert_eq!(checked_clock(CLOCK_REALTIME), Ok(()));
        assert_eq!(checked_clock(CLOCK_MONOTONIC), Ok(()));
        for clock in [-1, 2, 3, c_int::MAX] {
            assert_eq!(checked_clock(clock), Err(ERRNO_EINVAL));
        }
        assert_eq!(clock_result(u64::MAX), Err(ERRNO_EIO));
        assert_eq!(clock_result(0), Ok(Duration::ZERO));
        assert_eq!(clock_result(1_000_000_001), Ok(Duration::new(1, 1)));
    }

    #[test]
    fn duration_conversion_rejects_invalid_times_and_native_overflow() {
        for time in [
            Timespec {
                tv_sec: -1,
                tv_nsec: 0,
            },
            Timespec {
                tv_sec: 0,
                tv_nsec: -1,
            },
            Timespec {
                tv_sec: 0,
                tv_nsec: 1_000_000_000,
            },
        ] {
            assert_eq!(sleep_duration(time), Err(ERRNO_EINVAL));
        }
        let maximum = Duration::from_nanos(u64::MAX);
        assert_eq!(
            sleep_duration(duration_timespec(maximum).unwrap()),
            Ok(maximum)
        );
        assert_eq!(
            sleep_duration(duration_timespec(maximum + Duration::from_nanos(1)).unwrap()),
            Err(ERRNO_EOVERFLOW)
        );
        assert_eq!(
            sleep_duration(Timespec {
                tv_sec: i64::MAX,
                tv_nsec: 0
            }),
            Err(ERRNO_EOVERFLOW)
        );
        assert_eq!(
            duration_timespec(Duration::from_secs(i64::MAX as u64))
                .unwrap()
                .tv_sec,
            i64::MAX
        );
        assert!(matches!(
            duration_timespec(Duration::from_secs(i64::MAX as u64 + 1)),
            Err(ERRNO_EOVERFLOW)
        ));
    }

    #[test]
    fn random_validation_never_converts_bad_results_into_success() {
        assert_eq!(random_arguments(true, 0, 0), Ok(()));
        assert_eq!(
            random_arguments(true, 1, 0),
            Err(scarlet_abi::fs::ERRNO_EFAULT)
        );
        assert_eq!(
            random_arguments(false, isize::MAX as usize + 1, 0),
            Err(ERRNO_EINVAL)
        );
        assert_eq!(random_arguments(false, 1, 4), Err(ERRNO_EINVAL));
        for flag in [GRND_NONBLOCK, GRND_RANDOM, GRND_NONBLOCK | GRND_RANDOM] {
            assert_eq!(
                random_arguments(false, 1, flag),
                Err(scarlet_abi::ERRNO_EOPNOTSUPP)
            );
        }
        for count in [0, 1, 99, 100] {
            assert_eq!(random_result(count, 100), Ok(count));
        }
        assert_eq!(random_result(101, 100), Err(ERRNO_EIO));
        assert_eq!(random_result(usize::MAX, 100), Err(ERRNO_EIO));
    }
}
