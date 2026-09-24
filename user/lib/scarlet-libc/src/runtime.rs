//! Small C runtime facilities without process-global locale or calendar state.
//!
//! Native clocks expose microsecond-quantized nanoseconds. We query them
//! directly because Scarlet std's infallible `SystemTime::now` panics when UTC
//! has not been initialized, whereas the C functions must report an error.
//! Sleep uses Rust std after validating its finite Native u64-nanosecond range.
//! Entropy calls always require a registered source; they never use the Native
//! emergency PRNG. The legacy entropy syscall has only an undifferentiated
//! failure sentinel, so failures after local validation are reported as EIO.

use std::ffi::{c_char, c_int, c_long};
#[cfg(target_os = "scarlet")]
use std::ffi::{c_uint, c_void};
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

#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn sysconf(name: c_int) -> c_long {
    match name {
        30 => 4096, // _SC_PAGESIZE; Scarlet's architecture-independent PAGE_SIZE
        70 => 4096, // _SC_GETPW_R_SIZE_MAX; upper bound for the synthetic user record
        _ => crate::fail(ERRNO_EINVAL) as c_long,
    }
}

#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn sched_yield() -> c_int {
    std::thread::yield_now();
    0
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Timeval {
    pub tv_sec: i64,
    pub tv_usec: c_long,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tm {
    pub tm_sec: c_int,
    pub tm_min: c_int,
    pub tm_hour: c_int,
    pub tm_mday: c_int,
    pub tm_mon: c_int,
    pub tm_year: c_int,
    pub tm_wday: c_int,
    pub tm_yday: c_int,
    pub tm_isdst: c_int,
    pub tm_gmtoff: c_long,
    pub tm_zone: *const c_char,
}

#[cfg(any(test, target_os = "scarlet"))]
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    // Proleptic Gregorian calendar, with the Unix epoch shifted to the civil
    // epoch. Division uses floor semantics for dates before 1970.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month, day)
}

#[cfg(any(test, target_os = "scarlet"))]
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(any(test, target_os = "scarlet"))]
fn utc_calendar(seconds: i64) -> Result<Tm, c_int> {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let tm_year = c_int::try_from(year - 1900).map_err(|_| ERRNO_EOVERFLOW)?;
    Ok(Tm {
        tm_sec: (seconds_of_day % 60) as c_int,
        tm_min: ((seconds_of_day / 60) % 60) as c_int,
        tm_hour: (seconds_of_day / 3_600) as c_int,
        tm_mday: day as c_int,
        tm_mon: (month - 1) as c_int,
        tm_year,
        tm_wday: (days + 4).rem_euclid(7) as c_int,
        tm_yday: (days - days_from_civil(year, 1, 1)) as c_int,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: c"UTC".as_ptr(),
    })
}

#[cfg(any(test, target_os = "scarlet"))]
fn utc_timestamp(input: Tm) -> Result<(i64, Tm), c_int> {
    let year = i64::from(input.tm_year) + 1900 + i64::from(input.tm_mon).div_euclid(12);
    let month = i64::from(input.tm_mon).rem_euclid(12) + 1;
    let days = days_from_civil(year, month, i64::from(input.tm_mday));
    let seconds = days
        .checked_mul(86_400)
        .and_then(|value| value.checked_add(i64::from(input.tm_hour) * 3_600))
        .and_then(|value| value.checked_add(i64::from(input.tm_min) * 60))
        .and_then(|value| value.checked_add(i64::from(input.tm_sec)))
        .ok_or(ERRNO_EOVERFLOW)?;
    Ok((seconds, utc_calendar(seconds)?))
}

/// # Safety
/// `input` points to a writable struct tm.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mktime(input: *mut Tm) -> i64 {
    if input.is_null() {
        crate::fail(scarlet_abi::fs::ERRNO_EFAULT);
        return -1;
    }
    // SAFETY: the C caller supplies a readable/writable struct tm.
    let original = unsafe { *input };
    match utc_timestamp(original) {
        Ok((seconds, normalized)) => {
            // SAFETY: the pointer was checked above.
            unsafe { *input = normalized };
            seconds
        }
        Err(errno) => {
            crate::fail(errno);
            -1
        }
    }
}

#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn difftime(end: i64, beginning: i64) -> f64 {
    (i128::from(end) - i128::from(beginning)) as f64
}

/// # Safety
/// Both pointers must be valid for reading/writing a time_t/struct tm.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gmtime_r(input: *const i64, output: *mut Tm) -> *mut Tm {
    if input.is_null() || output.is_null() {
        crate::fail(ERRNO_EFAULT);
        return std::ptr::null_mut();
    }
    // SAFETY: The caller provides a readable time_t.
    let value = unsafe { *input };
    let calendar = match utc_calendar(value) {
        Ok(calendar) => calendar,
        Err(error) => {
            crate::fail(error);
            return std::ptr::null_mut();
        }
    };
    // SAFETY: The caller provides a writable struct tm.
    unsafe { *output = calendar };
    output
}

/// Scarlet's local clock is UTC until timezone configuration is available.
/// # Safety
/// Both pointers must be valid for reading/writing a time_t/struct tm.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn localtime_r(input: *const i64, output: *mut Tm) -> *mut Tm {
    // SAFETY: The contracts are identical while local time equals UTC.
    unsafe { gmtime_r(input, output) }
}

#[cfg(target_os = "scarlet")]
thread_local! {
    static CALENDAR_BUFFER: std::cell::UnsafeCell<Tm> = const {
        std::cell::UnsafeCell::new(Tm {
            tm_sec: 0, tm_min: 0, tm_hour: 0, tm_mday: 0, tm_mon: 0,
            tm_year: 0, tm_wday: 0, tm_yday: 0, tm_isdst: 0,
            tm_gmtoff: 0, tm_zone: std::ptr::null(),
        })
    };
}

/// # Safety
/// `input` must point to a readable time_t. The returned thread-local buffer
/// is replaced by the next gmtime or localtime call from this thread.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gmtime(input: *const i64) -> *mut Tm {
    CALENDAR_BUFFER.with(|buffer| {
        // SAFETY: this thread owns its TLS calendar buffer.
        unsafe { gmtime_r(input, buffer.get()) }
    })
}

/// # Safety
/// `input` must point to a readable time_t. Scarlet local time is UTC.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn localtime(input: *const i64) -> *mut Tm {
    // SAFETY: localtime and gmtime share the same contract and UTC backend.
    unsafe { gmtime(input) }
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
    fn utc_calendar_handles_epoch_leap_years_and_range() {
        for (seconds, year, month, day, weekday, yearday) in [
            (0, 1970, 1, 1, 4, 0),
            (-1, 1969, 12, 31, 3, 364),
            (-2_203_891_200, 1900, 3, 1, 4, 59),
            (951_782_400, 2000, 2, 29, 2, 59),
            (4_107_542_400, 2100, 3, 1, 1, 59),
        ] {
            let calendar = utc_calendar(seconds).unwrap();
            assert_eq!(calendar.tm_year, year - 1900);
            assert_eq!(calendar.tm_mon, month - 1);
            assert_eq!(calendar.tm_mday, day);
            assert_eq!(calendar.tm_wday, weekday);
            assert_eq!(calendar.tm_yday, yearday);
            assert_eq!(calendar.tm_isdst, 0);
            assert_eq!(calendar.tm_gmtoff, 0);
            assert_eq!(
                days_from_civil(year.into(), month.into(), day.into()),
                seconds.div_euclid(86_400)
            );
        }
        assert_eq!(utc_calendar(i64::MAX).unwrap_err(), ERRNO_EOVERFLOW);
        assert_eq!(utc_calendar(i64::MIN).unwrap_err(), ERRNO_EOVERFLOW);
    }

    #[test]
    fn utc_timestamp_round_trips_and_normalizes_fields() {
        for seconds in [-2_203_891_200, -1, 0, 951_782_400, 4_107_542_400] {
            let calendar = utc_calendar(seconds).unwrap();
            assert_eq!(utc_timestamp(calendar).unwrap().0, seconds);
        }
        let mut input = utc_calendar(0).unwrap();
        input.tm_mon = 13;
        input.tm_mday = 0;
        input.tm_hour = 24;
        let (seconds, normalized) = utc_timestamp(input).unwrap();
        assert_eq!(seconds, days_from_civil(1971, 2, 1) * 86_400);
        assert_eq!(
            (normalized.tm_year, normalized.tm_mon, normalized.tm_mday),
            (71, 1, 1)
        );
    }

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
