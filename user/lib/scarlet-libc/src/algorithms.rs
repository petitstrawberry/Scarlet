//! Allocation-free C sorting, searching and byte-string algorithms.
//!
//! Character operations implement the ASCII C locale. `strtok` uses per-thread
//! state (POSIX permits process-global state); `strtok_r` uses caller state.

use std::cell::Cell;
use std::ffi::{c_char, c_int, c_long, c_longlong, c_void};
use std::mem::MaybeUninit;
use std::ptr;

pub type Comparator = unsafe extern "C" fn(*const c_void, *const c_void) -> c_int;

fn valid_extent(count: usize, size: usize) -> bool {
    count
        .checked_mul(size)
        .is_some_and(|bytes| bytes <= isize::MAX as usize)
}

// No slice or reference may span an element while the C comparator runs: the
// callback may inspect the array through its own pointers. Byte-aligned
// MaybeUninit preserves uninitialized C struct padding and supports odd sizes.
unsafe fn swap_elements(base: *mut u8, a: usize, b: usize, size: usize) {
    // SAFETY: callers validate the extent and pass distinct element indices,
    // making the ranges nonoverlapping. MaybeUninit<u8> has alignment one and
    // allows both initialized fields and uninitialized padding to be moved.
    unsafe {
        let a = base.add(a * size).cast::<MaybeUninit<u8>>();
        let b = base.add(b * size).cast::<MaybeUninit<u8>>();
        ptr::swap_nonoverlapping(a, b, size);
    }
}

unsafe fn sift_down(
    base: *mut u8,
    mut root: usize,
    count: usize,
    size: usize,
    compare: Comparator,
) {
    // A root below count/2 has a child; this test also bounds 2*root+1.
    while root < count / 2 {
        let mut child = root * 2 + 1;
        // SAFETY: all comparator pointers address actual elements in the array,
        // and the callback obeys qsort's read-only comparison contract.
        unsafe {
            if child < count - 1
                && compare(
                    base.add(child * size).cast(),
                    base.add((child + 1) * size).cast(),
                ) < 0
            {
                child += 1;
            }
            if compare(base.add(root * size).cast(), base.add(child * size).cast()) >= 0 {
                break;
            }
            swap_elements(base, root, child, size);
        }
        root = child;
    }
}

/// Sort an array with allocation-free heapsort (worst-case O(n log n) comparisons).
///
/// # Safety
/// For nonzero count and size, base must address count writable elements of size
/// bytes, and compare must implement a consistent total ordering without modifying
/// the elements. As in C, their complete extent must belong to one allocation.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn qsort(
    base: *mut c_void,
    count: usize,
    size: usize,
    compare: Option<Comparator>,
) {
    if count < 2 || size == 0 || !valid_extent(count, size) {
        return;
    }
    let Some(compare) = compare else { return };
    let base = base.cast::<u8>();
    // SAFETY: the C contract supplies the allocation and callback; checked extent
    // ensures every index multiplication below is representable.
    unsafe {
        for root in (0..count / 2).rev() {
            sift_down(base, root, count, size, compare);
        }
        for end in (1..count).rev() {
            swap_elements(base, 0, end, size);
            sift_down(base, 0, end, size, compare);
        }
    }
}

/// Find an equal element in an ascending sorted array; duplicate choice is unspecified.
///
/// # Safety
/// key and each of count elements at base must be valid inputs to compare. The
/// array must occupy one allocation and be sorted using that comparison order.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn bsearch(
    key: *const c_void,
    base: *const c_void,
    count: usize,
    size: usize,
    compare: Option<Comparator>,
) -> *mut c_void {
    if count == 0 || size == 0 || !valid_extent(count, size) {
        return ptr::null_mut();
    }
    let Some(compare) = compare else {
        return ptr::null_mut();
    };
    let mut low = 0;
    let mut high = count;
    while low < high {
        let middle = low + (high - low) / 2;
        // SAFETY: middle is an element index in the checked input extent.
        let element = unsafe { base.cast::<u8>().add(middle * size) }.cast::<c_void>();
        // SAFETY: caller supplies readable key/element and a valid callback.
        let order = unsafe { compare(key, element) };
        if order < 0 {
            high = middle;
        } else if order > 0 {
            low = middle + 1;
        } else {
            return element.cast_mut();
        }
    }
    ptr::null_mut()
}

/// C leaves the result undefined when the absolute value is unrepresentable.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn abs(value: c_int) -> c_int {
    value.wrapping_abs()
}

/// C leaves the result undefined when the absolute value is unrepresentable.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn labs(value: c_long) -> c_long {
    value.wrapping_abs()
}

/// C leaves the result undefined when the absolute value is unrepresentable.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn llabs(value: c_longlong) -> c_longlong {
    value.wrapping_abs()
}

struct ByteSet([u64; 4]);

impl ByteSet {
    unsafe fn from_c_string(mut bytes: *const c_char) -> Self {
        let mut set = Self([0; 4]);
        // SAFETY: caller supplies a readable NUL-terminated byte string.
        unsafe {
            loop {
                let byte = bytes.read() as u8;
                if byte == 0 {
                    return set;
                }
                set.0[(byte / 64) as usize] |= 1u64 << (byte % 64);
                bytes = bytes.add(1);
            }
        }
    }

    fn contains(&self, byte: u8) -> bool {
        self.0[(byte / 64) as usize] & (1u64 << (byte % 64)) != 0
    }
}

/// # Safety
/// text and accepted must be readable NUL-terminated strings.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strspn(text: *const c_char, accepted: *const c_char) -> usize {
    // SAFETY: caller supplies both strings; NUL is never in the byte set.
    unsafe {
        let set = ByteSet::from_c_string(accepted);
        let mut count = 0;
        while set.contains(text.add(count).read() as u8) {
            count += 1;
        }
        count
    }
}

/// # Safety
/// text and rejected must be readable NUL-terminated strings.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strcspn(text: *const c_char, rejected: *const c_char) -> usize {
    // SAFETY: caller supplies both strings; scanning stops at the text's NUL.
    unsafe {
        let set = ByteSet::from_c_string(rejected);
        let mut count = 0;
        loop {
            let byte = text.add(count).read() as u8;
            if byte == 0 || set.contains(byte) {
                return count;
            }
            count += 1;
        }
    }
}

/// # Safety
/// text and accepted must be readable NUL-terminated strings.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strpbrk(text: *const c_char, accepted: *const c_char) -> *mut c_char {
    // SAFETY: strcspn stops inside the text, including its terminating NUL.
    unsafe {
        let found = text.add(strcspn(text, accepted));
        if found.read() == 0 {
            ptr::null_mut()
        } else {
            found.cast_mut()
        }
    }
}

/// # Safety
/// A non-null text must be writable and NUL-terminated; delimiters must be a
/// readable NUL-terminated string. state must be writable, and when text is null
/// its stored pointer must be null or a still-live tokenizer continuation.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strtok_r(
    text: *mut c_char,
    delimiters: *const c_char,
    state: *mut *mut c_char,
) -> *mut c_char {
    // SAFETY: state is caller-owned; text or its saved continuation is writable.
    unsafe {
        let mut cursor = if text.is_null() { state.read() } else { text };
        if cursor.is_null() {
            return ptr::null_mut();
        }
        let set = ByteSet::from_c_string(delimiters);
        while set.contains(cursor.read() as u8) {
            cursor = cursor.add(1);
        }
        if cursor.read() == 0 {
            state.write(ptr::null_mut());
            return ptr::null_mut();
        }
        let token = cursor;
        loop {
            let byte = cursor.read() as u8;
            if byte == 0 {
                state.write(ptr::null_mut());
                return token;
            }
            if set.contains(byte) {
                cursor.write(0);
                state.write(cursor.add(1));
                return token;
            }
            cursor = cursor.add(1);
        }
    }
}

thread_local! {
    static TOKENIZER: Cell<*mut c_char> = const { Cell::new(ptr::null_mut()) };
}

/// # Safety
/// text/delimiters must meet strtok_r's string requirements. A null text continues
/// this thread's previous sequence; that sequence's storage must remain live.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strtok(text: *mut c_char, delimiters: *const c_char) -> *mut c_char {
    TOKENIZER.with(|slot| {
        let mut state = slot.get();
        // SAFETY: local state is live and string requirements are inherited.
        let token = unsafe { strtok_r(text, delimiters, &mut state) };
        slot.set(state);
        token
    })
}

fn ascii_lower(byte: u8) -> u8 {
    if byte.is_ascii_uppercase() {
        byte + (b'a' - b'A')
    } else {
        byte
    }
}

/// # Safety
/// left and right must be readable NUL-terminated strings.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strcasecmp(left: *const c_char, right: *const c_char) -> c_int {
    // SAFETY: string requirements are forwarded; scanning stops at either NUL.
    unsafe { strncasecmp(left, right, usize::MAX) }
}

/// # Safety
/// Each input must be readable through its first NUL or count bytes, whichever
/// comes first. Comparisons use unsigned-byte ordering in the ASCII C locale.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn strncasecmp(
    mut left: *const c_char,
    mut right: *const c_char,
    count: usize,
) -> c_int {
    // SAFETY: caller supplies the readable extent for both input strings.
    unsafe {
        for _ in 0..count {
            let a = ascii_lower(left.read() as u8);
            let b = ascii_lower(right.read() as u8);
            if a != b || a == 0 {
                return c_int::from(a) - c_int::from(b);
            }
            left = left.add(1);
            right = right.add(1);
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::sync::{Arc, Barrier};

    unsafe extern "C" fn integer_order(a: *const c_void, b: *const c_void) -> c_int {
        // SAFETY: each callback input is an i32 element or search key.
        let (a, b) = unsafe {
            (
                a.cast::<i32>().read_unaligned(),
                b.cast::<i32>().read_unaligned(),
            )
        };
        (a > b) as c_int - (a < b) as c_int
    }

    #[test]
    fn heapsort_and_binary_search_many_shapes() {
        for count in 0..257 {
            for shape in 0..4 {
                let mut actual: Vec<i32> = (0..count)
                    .map(|i| match shape {
                        0 => i,
                        1 => -i,
                        2 => (i * 167) % 31,
                        _ => 7,
                    })
                    .collect();
                let mut expected = actual.clone();
                expected.sort();
                // SAFETY: Vec owns the complete array and callback reads i32.
                unsafe {
                    qsort(
                        actual.as_mut_ptr().cast(),
                        actual.len(),
                        4,
                        Some(integer_order),
                    );
                    assert_eq!(actual, expected);
                    for key in [-258i32, -1, 0, 7, 19, 30, 256, 258] {
                        let found = bsearch(
                            (&key as *const i32).cast(),
                            actual.as_ptr().cast(),
                            actual.len(),
                            4,
                            Some(integer_order),
                        );
                        assert_eq!(!found.is_null(), actual.contains(&key));
                        if !found.is_null() {
                            assert_eq!(found.cast::<i32>().read(), key);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn sort_preserves_fields_with_uninitialized_struct_padding() {
        #[repr(C)]
        struct Record {
            tag: u8,
            key: u32,
        }

        unsafe extern "C" fn compare(a: *const c_void, b: *const c_void) -> c_int {
            // SAFETY: records have initialized key fields; padding is never read.
            let (a, b) = unsafe {
                (
                    ptr::addr_of!((*a.cast::<Record>()).key).read(),
                    ptr::addr_of!((*b.cast::<Record>()).key).read(),
                )
            };
            (a > b) as c_int - (a < b) as c_int
        }

        let keys = [4, 0, 2, 2, u32::MAX, 1, 10];
        let mut records = [const { MaybeUninit::<Record>::uninit() }; 7];
        // Initialize only fields: copying whole Record values could accidentally
        // mask the bug by initializing padding. Miri diagnoses byte reads from
        // the still-uninitialized gaps in the original swap implementation.
        for (index, record) in records.iter_mut().enumerate() {
            // SAFETY: each field belongs to this writable, aligned array slot.
            unsafe {
                ptr::addr_of_mut!((*record.as_mut_ptr()).tag).write(index as u8);
                ptr::addr_of_mut!((*record.as_mut_ptr()).key).write(keys[index]);
            }
        }
        // SAFETY: the complete array is writable; comparator reads only fields.
        unsafe {
            qsort(
                records.as_mut_ptr().cast(),
                records.len(),
                size_of::<Record>(),
                Some(compare),
            );
        }
        let mut seen = 0u8;
        let mut previous = 0;
        for record in &records {
            // SAFETY: qsort moves complete records, preserving initialized fields.
            let (tag, key) = unsafe {
                (
                    ptr::addr_of!((*record.as_ptr()).tag).read(),
                    ptr::addr_of!((*record.as_ptr()).key).read(),
                )
            };
            assert!(tag < 7);
            assert_eq!(key, keys[tag as usize]);
            assert_eq!(seen & (1 << tag), 0);
            seen |= 1 << tag;
            assert!(key >= previous);
            previous = key;
        }
        assert_eq!(seen, 0x7f);
    }

    #[test]
    fn zero_and_invalid_extents_do_not_call_or_dereference() {
        unsafe extern "C" fn forbidden(_: *const c_void, _: *const c_void) -> c_int {
            panic!("comparator called for an empty or invalid extent")
        }
        for (count, size) in [(0, 4), (4, 0), (usize::MAX, 2), (2, isize::MAX as usize)] {
            // SAFETY: these edge cases return before dereferencing pointers.
            unsafe {
                qsort(ptr::null_mut(), count, size, Some(forbidden));
                assert!(bsearch(ptr::null(), ptr::null(), count, size, Some(forbidden)).is_null());
            }
        }
    }

    #[test]
    fn spans_and_casefold_cover_all_nonzero_bytes() {
        for byte in 1..=255u8 {
            let text = [byte, byte, b'!', 0];
            let set = [byte, 0];
            // SAFETY: arrays contain their terminating NUL.
            unsafe {
                assert_eq!(
                    strspn(text.as_ptr().cast(), set.as_ptr().cast()),
                    if byte == b'!' { 3 } else { 2 }
                );
                assert_eq!(strcspn(text.as_ptr().cast(), set.as_ptr().cast()), 0);
                assert_eq!(
                    strpbrk(text.as_ptr().cast(), set.as_ptr().cast()),
                    text.as_ptr().cast_mut().cast()
                );
                let folded = [ascii_lower(byte), 0];
                assert_eq!(strcasecmp(set.as_ptr().cast(), folded.as_ptr().cast()), 0);
            }
        }
    }

    #[test]
    fn tokenizer_reentrant_and_changing_delimiters() {
        let mut input = b",,one:two;three,,\0".to_vec();
        let mut other = b"x/y\0".to_vec();
        let mut state = ptr::null_mut();
        let mut second = ptr::null_mut();
        // SAFETY: buffers remain live and writable throughout both sequences.
        unsafe {
            let first = strtok_r(input.as_mut_ptr().cast(), c",:".as_ptr(), &mut state);
            assert_eq!(CStr::from_ptr(first), c"one");
            assert_eq!(
                CStr::from_ptr(strtok_r(
                    other.as_mut_ptr().cast(),
                    c"/".as_ptr(),
                    &mut second
                )),
                c"x"
            );
            assert_eq!(
                CStr::from_ptr(strtok_r(ptr::null_mut(), c";".as_ptr(), &mut state)),
                c"two"
            );
            assert_eq!(
                CStr::from_ptr(strtok_r(ptr::null_mut(), c",".as_ptr(), &mut state)),
                c"three"
            );
            assert!(strtok_r(ptr::null_mut(), c",".as_ptr(), &mut state).is_null());
            assert!(strtok_r(ptr::null_mut(), c",".as_ptr(), &mut state).is_null());
            assert_eq!(
                CStr::from_ptr(strtok_r(ptr::null_mut(), c"".as_ptr(), &mut second)),
                c"y"
            );
        }
    }

    #[test]
    fn strtok_state_is_per_thread() {
        let barrier = Arc::new(Barrier::new(2));
        let workers: Vec<_> = [b"a,b\0", b"x,y\0"]
            .into_iter()
            .map(|text| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut input = *text;
                    // SAFETY: input stays live until its tokenizer is exhausted.
                    unsafe {
                        let first = strtok(input.as_mut_ptr().cast(), c",".as_ptr());
                        assert_eq!(first, input.as_mut_ptr().cast());
                        barrier.wait();
                        let second = strtok(ptr::null_mut(), c",".as_ptr());
                        assert_eq!(second, input.as_mut_ptr().add(2).cast());
                        assert!(strtok(ptr::null_mut(), c",".as_ptr()).is_null());
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn absolute_values_and_errno_preservation() {
        // SAFETY: errno belongs to the current thread.
        unsafe { *crate::__errno_location() = 73 };
        assert_eq!(abs(-i32::MAX), i32::MAX);
        assert_eq!(labs(-c_long::MAX), c_long::MAX);
        assert_eq!(llabs(-c_longlong::MAX), c_longlong::MAX);
        assert_eq!(abs(0), 0);
        assert_eq!(abs(1), 1);
        // SAFETY: literal strings are terminated, zero length reads no bytes.
        unsafe {
            assert_eq!(strspn(c"aa!".as_ptr(), c"a".as_ptr()), 2);
            assert_eq!(strncasecmp(ptr::null(), ptr::null(), 0), 0);
            assert_eq!(*crate::__errno_location(), 73);
        }
    }
}
