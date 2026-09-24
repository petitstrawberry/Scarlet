//! Integer conversion in the C locale, using the C17/POSIX prefix grammar.
//!
//! Base zero recognizes decimal, octal and hexadecimal. The C23 `0b`/`0B`
//! extension is deliberately not enabled: at base zero or two, conversion stops
//! at `b`/`B`. No locale state, allocation, or Rust integer parsing is involved.

use std::ffi::{c_char, c_int, c_long, c_longlong, c_ulong, c_ulonglong};

use scarlet_abi::{ERRNO_EINVAL, fs::ERRNO_ERANGE};

struct Number {
    magnitude: u64,
    negative: bool,
    overflow: bool,
}

fn digit(byte: u8) -> u32 {
    match byte {
        b'0'..=b'9' => u32::from(byte - b'0'),
        b'a'..=b'z' => u32::from(byte - b'a') + 10,
        b'A'..=b'Z' => u32::from(byte - b'A') + 10,
        _ => u32::MAX,
    }
}

/// Parse a magnitude bounded by the range of the requested signed/unsigned type.
///
/// # Safety
/// `text` is a readable NUL-terminated string; a non-null `end` is writable.
unsafe fn number(
    text: *const c_char,
    end: *mut *mut c_char,
    mut base: c_int,
    positive_limit: u64,
    negative_limit: u64,
) -> Number {
    if !end.is_null() {
        // SAFETY: the caller supplies a writable pointer object.
        unsafe { *end = text.cast_mut() };
    }
    if base != 0 && !(2..=36).contains(&base) {
        crate::fail(ERRNO_EINVAL);
        return Number {
            magnitude: 0,
            negative: false,
            overflow: false,
        };
    }

    let mut cursor = text.cast::<u8>();
    // SAFETY: each advance below follows a non-NUL byte within the string.
    while matches!(unsafe { *cursor }, b' ' | b'\t'..=b'\r') {
        cursor = unsafe { cursor.add(1) };
    }
    let negative = unsafe { *cursor } == b'-';
    if matches!(unsafe { *cursor }, b'+' | b'-') {
        cursor = unsafe { cursor.add(1) };
    }

    if unsafe { *cursor } == b'0' {
        // Only accept the prefix if a hexadecimal digit follows it. For "0x"
        // or "0xg", the leading zero is itself an octal/hexadecimal digit.
        if (base == 0 || base == 16)
            && matches!(unsafe { *cursor.add(1) }, b'x' | b'X')
            && digit(unsafe { *cursor.add(2) }) < 16
        {
            cursor = unsafe { cursor.add(2) };
            base = 16;
        } else if base == 0 {
            base = 8;
        }
    } else if base == 0 {
        base = 10;
    }

    let first_digit = cursor;
    let limit = if negative {
        negative_limit
    } else {
        positive_limit
    };
    let radix = base as u64;
    let cutoff = limit / radix;
    let remainder = limit % radix;
    let mut magnitude = 0;
    let mut overflow = false;
    loop {
        // SAFETY: cursor is within the NUL-terminated string. NUL is not a digit.
        let value = u64::from(digit(unsafe { *cursor }));
        if value >= radix {
            break;
        }
        if magnitude > cutoff || (magnitude == cutoff && value > remainder) {
            overflow = true;
        } else if !overflow {
            magnitude = magnitude * radix + value;
        }
        // Overflow still consumes every valid digit, preserving endptr semantics.
        cursor = unsafe { cursor.add(1) };
    }
    if cursor != first_digit && !end.is_null() {
        // SAFETY: end is writable, and cursor still points into the input.
        unsafe { *end = cursor.cast::<c_char>().cast_mut() };
    }
    if overflow {
        crate::fail(ERRNO_ERANGE);
        magnitude = limit;
    }
    Number {
        magnitude,
        negative,
        overflow,
    }
}

/// Convert a C string to a signed long, saturating with ERANGE on overflow.
///
/// # Safety
/// `text` is a readable NUL-terminated string. If non-null, `end` is writable.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strtol(text: *const c_char, end: *mut *mut c_char, base: c_int) -> c_long {
    // SAFETY: the C API caller supplies the input string and optional end pointer.
    let number = unsafe { number(text, end, base, c_long::MAX as u64, c_long::MAX as u64 + 1) };
    if number.negative {
        (number.magnitude as c_long).wrapping_neg()
    } else {
        number.magnitude as c_long
    }
}

/// Convert a C string to an unsigned long, saturating with ERANGE on overflow.
///
/// A minus sign negates an in-range magnitude in the unsigned result type.
///
/// # Safety
/// `text` is a readable NUL-terminated string. If non-null, `end` is writable.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strtoul(
    text: *const c_char,
    end: *mut *mut c_char,
    base: c_int,
) -> c_ulong {
    // SAFETY: the C API caller supplies the input string and optional end pointer.
    let number = unsafe { number(text, end, base, c_ulong::MAX as u64, c_ulong::MAX as u64) };
    // On range error the result is ULONG_MAX, even when the input is negative.
    if number.negative && !number.overflow {
        (number.magnitude as c_ulong).wrapping_neg()
    } else {
        number.magnitude as c_ulong
    }
}

/// Convert a C string to a signed long long, saturating with ERANGE on overflow.
///
/// # Safety
/// `text` is a readable NUL-terminated string. If non-null, `end` is writable.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strtoll(
    text: *const c_char,
    end: *mut *mut c_char,
    base: c_int,
) -> c_longlong {
    // SAFETY: the C API caller supplies the input string and optional end pointer.
    let number = unsafe {
        number(
            text,
            end,
            base,
            c_longlong::MAX as u64,
            c_longlong::MAX as u64 + 1,
        )
    };
    if number.negative {
        (number.magnitude as c_longlong).wrapping_neg()
    } else {
        number.magnitude as c_longlong
    }
}

/// Convert a C string to an unsigned long long, saturating with ERANGE on overflow.
///
/// # Safety
/// `text` is a readable NUL-terminated string. If non-null, `end` is writable.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strtoull(
    text: *const c_char,
    end: *mut *mut c_char,
    base: c_int,
) -> c_ulonglong {
    // SAFETY: the C API caller supplies the input string and optional end pointer.
    let number = unsafe { number(text, end, base, c_ulonglong::MAX, c_ulonglong::MAX) };
    // On range error the result is ULLONG_MAX, even when the input is negative.
    if number.negative && !number.overflow {
        number.magnitude.wrapping_neg()
    } else {
        number.magnitude
    }
}

#[repr(C)]
pub struct Imaxdiv {
    pub quot: i64,
    pub rem: i64,
}

/// Return the absolute value of a C intmax_t; the minimum value is undefined
/// by C, and wraps rather than panicking at the Rust FFI boundary.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn imaxabs(value: i64) -> i64 {
    value.wrapping_abs()
}

/// Divide two C intmax_t values and return quotient and remainder.
/// Division by zero and the MIN/-1 pair are undefined by C.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn imaxdiv(numerator: i64, denominator: i64) -> Imaxdiv {
    Imaxdiv {
        quot: numerator / denominator,
        rem: numerator % denominator,
    }
}

/// # Safety
/// `text` is a readable NUL-terminated string and `end`, if non-null, is writable.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strtoimax(text: *const c_char, end: *mut *mut c_char, base: c_int) -> i64 {
    // SAFETY: intmax_t and long long are both signed 64-bit on Scarlet.
    unsafe { strtoll(text, end, base) }
}

/// # Safety
/// `text` is a readable NUL-terminated string and `end`, if non-null, is writable.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strtoumax(text: *const c_char, end: *mut *mut c_char, base: c_int) -> u64 {
    // SAFETY: uintmax_t and unsigned long long are both 64-bit on Scarlet.
    unsafe { strtoull(text, end, base) }
}

/// Convert a decimal C string to int. Use strtol when range checking is needed.
///
/// # Safety
/// `text` is a readable NUL-terminated string with a value representable as int.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn atoi(text: *const c_char) -> c_int {
    // SAFETY: the caller supplies the string and a representable decimal value.
    unsafe { strtol(text, std::ptr::null_mut(), 10) as c_int }
}

/// Convert a decimal C string to long. Use strtol when range checking is needed.
///
/// # Safety
/// `text` is a readable NUL-terminated string with a value representable as long.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn atol(text: *const c_char) -> c_long {
    // SAFETY: the caller supplies the string and a representable decimal value.
    unsafe { strtol(text, std::ptr::null_mut(), 10) }
}

/// Convert a decimal C string to long long. Use strtoll for range checking.
///
/// # Safety
/// `text` is a readable NUL-terminated string with a representable decimal value.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn atoll(text: *const c_char) -> c_longlong {
    // SAFETY: the caller supplies the string and a representable decimal value.
    unsafe { strtoll(text, std::ptr::null_mut(), 10) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn signed(text: &str, base: c_int) -> (c_longlong, usize, c_int) {
        let text = CString::new(text).unwrap();
        let mut end = std::ptr::null_mut();
        // SAFETY: the CString and end pointer remain live during conversion.
        unsafe {
            *crate::__errno_location() = 73;
            let result = strtoll(text.as_ptr(), &mut end, base);
            (
                result,
                end.offset_from(text.as_ptr()) as usize,
                *crate::__errno_location(),
            )
        }
    }

    fn unsigned(text: &str, base: c_int) -> (c_ulonglong, usize, c_int) {
        let text = CString::new(text).unwrap();
        let mut end = std::ptr::null_mut();
        // SAFETY: the CString and end pointer remain live during conversion.
        unsafe {
            *crate::__errno_location() = 73;
            let result = strtoull(text.as_ptr(), &mut end, base);
            (
                result,
                end.offset_from(text.as_ptr()) as usize,
                *crate::__errno_location(),
            )
        }
    }

    #[test]
    fn whitespace_sign_prefix_and_end_pointer() {
        assert_eq!(signed(" \t\n\r\x0b\x0c-0X7f trailing", 0), (-127, 11, 73));
        assert_eq!(signed("+0759", 0), (61, 4, 73));
        assert_eq!(signed("09", 0), (0, 1, 73));
        assert_eq!(signed("123xyz", 10), (123, 3, 73));
        assert_eq!(signed("Zz!", 36), (1295, 2, 73));
        assert_eq!(signed("1012", 2), (5, 3, 73));
        assert_eq!(signed("0xfg", 16), (15, 3, 73));
    }

    #[test]
    fn incomplete_prefix_and_absent_digits() {
        for input in ["", " \t", "+", "-", " +x", "--1", "\u{a0}12"] {
            assert_eq!(signed(input, 0), (0, 0, 73), "{input:?}");
        }
        for base in [0, 16] {
            assert_eq!(signed("0x", base), (0, 1, 73));
            assert_eq!(signed("-0Xg", base), (0, 2, 73));
        }
        assert_eq!(signed("0b11", 0), (0, 1, 73));
        assert_eq!(signed("0B11", 2), (0, 1, 73));
        assert_eq!(signed("0b", 16), (11, 2, 73));
    }

    #[test]
    fn invalid_base_sets_errno_and_original_end_pointer() {
        for base in [c_int::MIN, -1, 1, 37, c_int::MAX] {
            assert_eq!(signed(" \t123", base), (0, 0, ERRNO_EINVAL));
            assert_eq!(unsigned("-123", base), (0, 0, ERRNO_EINVAL));
        }
    }

    #[test]
    fn signed_boundaries_and_overflow_consume_all_digits() {
        assert_eq!(signed("9223372036854775807", 10), (i64::MAX, 19, 73));
        assert_eq!(signed("-9223372036854775808", 10), (i64::MIN, 20, 73));
        assert_eq!(
            signed("9223372036854775808!", 10),
            (i64::MAX, 19, ERRNO_ERANGE)
        );
        assert_eq!(
            signed("-9223372036854775809!", 10),
            (i64::MIN, 20, ERRNO_ERANGE)
        );
        let input = format!("{}!", "9".repeat(2048));
        assert_eq!(signed(&input, 10), (i64::MAX, 2048, ERRNO_ERANGE));
        assert_eq!(signed("-0x8000000000000000", 0), (i64::MIN, 19, 73));
        assert_eq!(
            signed("0x8000000000000000", 0),
            (i64::MAX, 18, ERRNO_ERANGE)
        );
    }

    #[test]
    fn unsigned_negative_and_overflow_are_distinct() {
        assert_eq!(unsigned("18446744073709551615", 10), (u64::MAX, 20, 73));
        assert_eq!(unsigned("-1", 10), (u64::MAX, 2, 73));
        assert_eq!(unsigned("-18446744073709551615", 10), (1, 21, 73));
        assert_eq!(unsigned("-0", 10), (0, 2, 73));
        assert_eq!(
            unsigned("18446744073709551616;", 10),
            (u64::MAX, 20, ERRNO_ERANGE)
        );
        assert_eq!(
            unsigned("-18446744073709551616;", 10),
            (u64::MAX, 21, ERRNO_ERANGE)
        );
        assert_eq!(unsigned("0xffffffffffffffff", 0), (u64::MAX, 18, 73));
    }

    #[test]
    fn all_radices_round_trip_limits_and_reject_next_magnitude() {
        fn representation(mut value: u128, base: u32) -> String {
            let mut digits = Vec::new();
            while value != 0 {
                digits.push(
                    b"0123456789abcdefghijklmnopqrstuvwxyz"[(value % u128::from(base)) as usize],
                );
                value /= u128::from(base);
            }
            digits.reverse();
            String::from_utf8(digits).unwrap()
        }
        for base in 2..=36 {
            let signed_max = representation(i64::MAX as u128, base);
            assert_eq!(
                signed(&signed_max, base as c_int),
                (i64::MAX, signed_max.len(), 73)
            );
            let signed_min = format!("-{}", representation(i64::MAX as u128 + 1, base));
            assert_eq!(
                signed(&signed_min, base as c_int),
                (i64::MIN, signed_min.len(), 73)
            );
            let negative_overflow = format!("-{}", representation(i64::MAX as u128 + 2, base));
            assert_eq!(
                signed(&negative_overflow, base as c_int),
                (i64::MIN, negative_overflow.len(), ERRNO_ERANGE)
            );
            let limit = representation(u128::from(u64::MAX), base);
            assert_eq!(unsigned(&limit, base as c_int), (u64::MAX, limit.len(), 73));
            let overflow = representation(u128::from(u64::MAX) + 1, base);
            assert_eq!(
                unsigned(&overflow, base as c_int),
                (u64::MAX, overflow.len(), ERRNO_ERANGE)
            );
        }
    }

    #[test]
    fn long_width_and_decimal_convenience_functions() {
        let max = CString::new(c_long::MAX.to_string()).unwrap();
        let min = CString::new(c_long::MIN.to_string()).unwrap();
        let unsigned_max = CString::new(c_ulong::MAX.to_string()).unwrap();
        let overflow = CString::new((c_long::MAX as u128 + 1).to_string()).unwrap();
        let underflow = CString::new((c_long::MIN as i128 - 1).to_string()).unwrap();
        let unsigned_overflow = CString::new((c_ulong::MAX as u128 + 1).to_string()).unwrap();
        // SAFETY: each input is NUL-terminated and every null endptr is permitted.
        unsafe {
            assert_eq!(strtol(max.as_ptr(), std::ptr::null_mut(), 10), c_long::MAX);
            assert_eq!(strtol(min.as_ptr(), std::ptr::null_mut(), 10), c_long::MIN);
            assert_eq!(
                strtoul(unsigned_max.as_ptr(), std::ptr::null_mut(), 10),
                c_ulong::MAX
            );
            *crate::__errno_location() = 0;
            assert_eq!(
                strtol(overflow.as_ptr(), std::ptr::null_mut(), 10),
                c_long::MAX
            );
            assert_eq!(*crate::__errno_location(), ERRNO_ERANGE);
            *crate::__errno_location() = 0;
            assert_eq!(
                strtol(underflow.as_ptr(), std::ptr::null_mut(), 10),
                c_long::MIN
            );
            assert_eq!(*crate::__errno_location(), ERRNO_ERANGE);
            *crate::__errno_location() = 0;
            assert_eq!(
                strtoul(unsigned_overflow.as_ptr(), std::ptr::null_mut(), 10),
                c_ulong::MAX
            );
            assert_eq!(*crate::__errno_location(), ERRNO_ERANGE);
            assert_eq!(atoi(c" -2147483648!".as_ptr()), c_int::MIN);
            assert_eq!(atoi(c"2147483647".as_ptr()), c_int::MAX);
            assert_eq!(atol(max.as_ptr()), c_long::MAX);
            assert_eq!(atoll(c"-9223372036854775808".as_ptr()), c_longlong::MIN);
            assert_eq!(atoi(c"010".as_ptr()), 10);
            assert_eq!(atoi(c"0x10".as_ptr()), 0);
            assert_eq!(atoi(c"n/a".as_ptr()), 0);
        }
    }
}
