//! Unbuffered C streams layered on the Native descriptor adapters.
//!
//! Every operation locks its stream for the complete operation. No output is
//! deferred to process exit. A single pushback byte supplies the C guarantee.
use crate::descriptor::{
    O_APPEND, O_CLOEXEC, O_CREAT, O_EXCL, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY,
};
use scarlet_abi::ERRNO_EINVAL;
use std::ffi::c_int;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mode {
    read: bool,
    write: bool,
    append: bool,
    flags: c_int,
}

fn parse_mode(mode: &[u8]) -> Result<Mode, c_int> {
    let first = *mode.first().ok_or(ERRNO_EINVAL)?;
    if !matches!(first, b'r' | b'w' | b'a') {
        return Err(ERRNO_EINVAL);
    }
    let (mut plus, mut binary, mut exclusive, mut cloexec) = (false, false, false, false);
    for &byte in &mode[1..] {
        let value = match byte {
            b'+' => &mut plus,
            b'b' => &mut binary,
            b'x' if first == b'w' => &mut exclusive,
            b'e' => &mut cloexec,
            _ => return Err(ERRNO_EINVAL),
        };
        if *value {
            return Err(ERRNO_EINVAL);
        }
        *value = true;
    }
    Ok(Mode {
        read: first == b'r' || plus,
        write: first != b'r' || plus,
        append: first == b'a',
        flags: (if plus {
            O_RDWR
        } else if first == b'r' {
            O_RDONLY
        } else {
            O_WRONLY
        }) | if first == b'w' { O_CREAT | O_TRUNC } else { 0 }
            | if first == b'a' { O_CREAT | O_APPEND } else { 0 }
            | if exclusive { O_EXCL } else { 0 }
            | if cloexec { O_CLOEXEC } else { 0 },
    })
}

#[cfg(target_os = "scarlet")]
mod native {
    use super::*;
    use crate::descriptor::O_ACCMODE;
    use crate::{allocation, descriptor, fail, formatting};
    use scarlet_abi::fs::{ERRNO_EFAULT, ERRNO_EOVERFLOW};
    use scarlet_abi::{ERRNO_EBADF, ERRNO_EIO};
    use std::ffi::{CStr, VaList, c_char, c_long, c_void};
    use std::ptr;
    use std::sync::{Mutex, MutexGuard};

    const EOF: c_int = -1;
    const ESPIPE: c_int = 29;
    struct State {
        fd: c_int,
        read: bool,
        write: bool,
        eof: bool,
        error: bool,
        pushed: Option<u8>,
        closed: bool,
    }
    #[repr(C)]
    pub struct File {
        state: Mutex<State>,
        heap: bool,
    }
    impl File {
        const fn new(fd: c_int, read: bool, write: bool, heap: bool) -> Self {
            Self {
                state: Mutex::new(State {
                    fd,
                    read,
                    write,
                    eof: false,
                    error: false,
                    pushed: None,
                    closed: false,
                }),
                heap,
            }
        }
    }
    static STDIN: File = File::new(0, true, false, false);
    static STDOUT: File = File::new(1, false, true, false);
    static STDERR: File = File::new(2, false, true, false);
    #[unsafe(no_mangle)]
    pub static mut stdin: *mut File = ptr::addr_of!(STDIN).cast_mut();
    #[unsafe(no_mangle)]
    pub static mut stdout: *mut File = ptr::addr_of!(STDOUT).cast_mut();
    #[unsafe(no_mangle)]
    pub static mut stderr: *mut File = ptr::addr_of!(STDERR).cast_mut();

    unsafe fn lock<'a>(stream: *mut File) -> Result<MutexGuard<'a, State>, c_int> {
        if stream.is_null() {
            return Err(ERRNO_EINVAL);
        }
        // SAFETY: the caller supplies a live stream for the entire operation.
        let state = unsafe { &(*stream).state };
        let guard = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.closed {
            Err(ERRNO_EBADF)
        } else {
            Ok(guard)
        }
    }
    fn errno() -> c_int {
        // SAFETY: errno is private to this thread.
        unsafe { *crate::__errno_location() }
    }
    fn error(state: &mut State, code: c_int) {
        state.error = true;
        fail(code);
    }
    unsafe fn mode_from_ptr(mode: *const c_char) -> Result<Mode, c_int> {
        if mode.is_null() {
            return Err(ERRNO_EINVAL);
        }
        // SAFETY: the caller supplies a NUL-terminated mode.
        parse_mode(unsafe { CStr::from_ptr(mode) }.to_bytes())
    }
    fn new_stream(fd: c_int, mode: Mode) -> *mut File {
        let result = allocation::malloc(size_of::<File>()).cast::<File>();
        if !result.is_null() {
            // SAFETY: the malloc allocation is correctly aligned and exclusive.
            unsafe {
                result.write(File::new(fd, mode.read, mode.write, true));
            }
        }
        result
    }

    /// # Safety
    /// Both input strings must be NUL-terminated.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fopen(path: *const c_char, mode: *const c_char) -> *mut File {
        let mode = match unsafe { mode_from_ptr(mode) } {
            Ok(mode) => mode,
            Err(code) => {
                fail(code);
                return ptr::null_mut();
            }
        };
        let fd = unsafe { descriptor::open_impl(path, mode.flags, 0o666) };
        if fd < 0 {
            return ptr::null_mut();
        }
        let stream = new_stream(fd, mode);
        if stream.is_null() {
            let code = errno();
            descriptor::close(fd);
            fail(code);
        }
        stream
    }
    /// # Safety
    /// mode is NUL-terminated; ownership of fd passes to the stream on success.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fdopen(fd: c_int, mode: *const c_char) -> *mut File {
        let mode = match unsafe { mode_from_ptr(mode) } {
            Ok(mode) => mode,
            Err(code) => {
                fail(code);
                return ptr::null_mut();
            }
        };
        if mode.flags & (O_EXCL | O_CLOEXEC) != 0 {
            fail(ERRNO_EINVAL);
            return ptr::null_mut();
        }
        let flags = match descriptor::descriptor_access(fd) {
            Ok(flags) => flags,
            Err(code) => {
                fail(code);
                return ptr::null_mut();
            }
        };
        if (mode.read && flags & O_ACCMODE == O_WRONLY)
            || (mode.write && flags & O_ACCMODE == O_RDONLY)
        {
            fail(ERRNO_EINVAL);
            return ptr::null_mut();
        }
        // Allocate before changing the descriptor, preserving it on ENOMEM.
        let stream = new_stream(fd, mode);
        if stream.is_null() {
            return stream;
        }
        if mode.append {
            if let Err(code) = descriptor::set_append(fd) {
                unsafe {
                    ptr::drop_in_place(stream);
                    allocation::free(stream.cast());
                }
                fail(code);
                return ptr::null_mut();
            }
        }
        stream
    }
    /// # Safety
    /// stream is live and no other thread can continue using it after this call.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fclose(stream: *mut File) -> c_int {
        let mut state = match unsafe { lock(stream) } {
            Ok(state) => state,
            Err(code) => return fail(code),
        };
        let result = descriptor::close(state.fd);
        state.closed = true;
        drop(state);
        if unsafe { (*stream).heap } {
            unsafe {
                ptr::drop_in_place(stream);
                allocation::free(stream.cast());
            }
        }
        result
    }
    /// # Safety
    /// A non-null stream must be live. NULL flushes all output streams.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fflush(stream: *mut File) -> c_int {
        // All output is already submitted; fflush does not promise fsync.
        if stream.is_null() {
            return 0;
        }
        let mut state = match unsafe { lock(stream) } {
            Ok(state) => state,
            Err(code) => return fail(code),
        };
        if state.pushed.is_some() {
            let saved = errno();
            if descriptor::lseek(state.fd, -1, 1) < 0 {
                let code = errno();
                if code != ESPIPE {
                    error(&mut state, code);
                    return EOF;
                }
            }
            // Seekable input must expose the logical (pushback-adjusted)
            // position. Nonseekable input has no offset to synchronize.
            state.pushed = None;
            unsafe {
                *crate::__errno_location() = saved;
            }
        }
        0
    }

    unsafe fn read_bytes(state: &mut State, output: *mut u8, count: usize) -> usize {
        if !state.read {
            error(state, ERRNO_EBADF);
            return 0;
        }
        let mut done = 0;
        if let Some(byte) = state.pushed.take() {
            unsafe {
                output.write(byte);
            }
            done = 1;
        }
        while done < count && !state.eof {
            let result =
                unsafe { descriptor::read(state.fd, output.add(done).cast(), count - done) };
            if result < 0 {
                state.error = true;
                break;
            }
            if result == 0 {
                state.eof = true;
                break;
            }
            done += result as usize;
        }
        done
    }
    unsafe fn write_bytes(state: &mut State, input: *const u8, count: usize) -> usize {
        if !state.write {
            error(state, ERRNO_EBADF);
            return 0;
        }
        let mut done = 0;
        while done < count {
            let result =
                unsafe { descriptor::write(state.fd, input.add(done).cast(), count - done) };
            if result < 0 {
                state.error = true;
                break;
            }
            if result == 0 {
                error(state, ERRNO_EIO);
                break;
            }
            done += result as usize;
        }
        done
    }
    /// # Safety
    /// output holds size*count writable bytes; stream is live.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fread(
        output: *mut c_void,
        size: usize,
        count: usize,
        stream: *mut File,
    ) -> usize {
        if size == 0 || count == 0 {
            return 0;
        }
        let mut state = match unsafe { lock(stream) } {
            Ok(state) => state,
            Err(code) => {
                fail(code);
                return 0;
            }
        };
        let Some(bytes) = size
            .checked_mul(count)
            .filter(|n| *n <= isize::MAX as usize)
        else {
            error(&mut state, ERRNO_EOVERFLOW);
            return 0;
        };
        if output.is_null() {
            error(&mut state, ERRNO_EFAULT);
            return 0;
        }
        (unsafe { read_bytes(&mut state, output.cast(), bytes) }) / size
    }
    /// # Safety
    /// input holds size*count readable bytes; stream is live.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fwrite(
        input: *const c_void,
        size: usize,
        count: usize,
        stream: *mut File,
    ) -> usize {
        if size == 0 || count == 0 {
            return 0;
        }
        let mut state = match unsafe { lock(stream) } {
            Ok(state) => state,
            Err(code) => {
                fail(code);
                return 0;
            }
        };
        let Some(bytes) = size
            .checked_mul(count)
            .filter(|n| *n <= isize::MAX as usize)
        else {
            error(&mut state, ERRNO_EOVERFLOW);
            return 0;
        };
        if input.is_null() {
            error(&mut state, ERRNO_EFAULT);
            return 0;
        }
        (unsafe { write_bytes(&mut state, input.cast(), bytes) }) / size
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fseeko(stream: *mut File, offset: i64, whence: c_int) -> c_int {
        let mut state = match unsafe { lock(stream) } {
            Ok(state) => state,
            Err(code) => return fail(code),
        };
        seek(&mut state, offset, whence)
    }
    fn seek(state: &mut State, offset: i64, whence: c_int) -> c_int {
        let offset = if whence == 1 && state.pushed.is_some() {
            match offset.checked_sub(1) {
                Some(offset) => offset,
                None => return fail(ERRNO_EOVERFLOW),
            }
        } else {
            offset
        };
        if descriptor::lseek(state.fd, offset, whence) < 0 {
            return EOF;
        }
        state.pushed = None;
        state.eof = false;
        0
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fseek(stream: *mut File, offset: c_long, whence: c_int) -> c_int {
        unsafe { fseeko(stream, offset, whence) }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ftello(stream: *mut File) -> i64 {
        let state = match unsafe { lock(stream) } {
            Ok(state) => state,
            Err(code) => {
                fail(code);
                return -1;
            }
        };
        let position = descriptor::lseek(state.fd, 0, 1);
        if position < 0 {
            return -1;
        }
        if state.pushed.is_some() {
            position - 1
        } else {
            position
        }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ftell(stream: *mut File) -> c_long {
        unsafe { ftello(stream) }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn rewind(stream: *mut File) {
        let mut state = match unsafe { lock(stream) } {
            Ok(state) => state,
            Err(code) => {
                fail(code);
                return;
            }
        };
        seek(&mut state, 0, 0);
        // As with fseek, EOF and pushback change only when repositioning
        // succeeds; rewind additionally clears the error indicator.
        state.error = false;
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn feof(stream: *mut File) -> c_int {
        match unsafe { lock(stream) } {
            Ok(state) => state.eof as c_int,
            Err(code) => {
                fail(code);
                0
            }
        }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ferror(stream: *mut File) -> c_int {
        match unsafe { lock(stream) } {
            Ok(state) => state.error as c_int,
            Err(code) => {
                fail(code);
                1
            }
        }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn clearerr(stream: *mut File) {
        match unsafe { lock(stream) } {
            Ok(mut state) => {
                state.error = false;
                state.eof = false;
            }
            Err(code) => {
                fail(code);
            }
        }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fileno(stream: *mut File) -> c_int {
        match unsafe { lock(stream) } {
            Ok(state) => state.fd,
            Err(code) => fail(code),
        }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fgetc(stream: *mut File) -> c_int {
        let mut state = match unsafe { lock(stream) } {
            Ok(state) => state,
            Err(code) => return fail(code),
        };
        let mut byte = 0;
        if unsafe { read_bytes(&mut state, &mut byte, 1) } == 1 {
            byte as c_int
        } else {
            EOF
        }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn getc(stream: *mut File) -> c_int {
        unsafe { fgetc(stream) }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn getchar() -> c_int {
        unsafe { fgetc(stdin) }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fputc(byte: c_int, stream: *mut File) -> c_int {
        let mut state = match unsafe { lock(stream) } {
            Ok(state) => state,
            Err(code) => return fail(code),
        };
        let byte = byte as u8;
        if unsafe { write_bytes(&mut state, &byte, 1) } == 1 {
            byte as c_int
        } else {
            EOF
        }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn putc(byte: c_int, stream: *mut File) -> c_int {
        unsafe { fputc(byte, stream) }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn putchar(byte: c_int) -> c_int {
        unsafe { fputc(byte, stdout) }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ungetc(byte: c_int, stream: *mut File) -> c_int {
        if byte == EOF {
            return EOF;
        }
        let mut state = match unsafe { lock(stream) } {
            Ok(state) => state,
            Err(code) => return fail(code),
        };
        if !state.read {
            return fail(ERRNO_EBADF);
        }
        if state.pushed.is_some() {
            return EOF;
        }
        state.pushed = Some(byte as u8);
        state.eof = false;
        byte as u8 as c_int
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fgets(
        output: *mut c_char,
        size: c_int,
        stream: *mut File,
    ) -> *mut c_char {
        if size <= 0 {
            fail(ERRNO_EINVAL);
            return ptr::null_mut();
        }
        let mut state = match unsafe { lock(stream) } {
            Ok(state) => state,
            Err(code) => {
                fail(code);
                return ptr::null_mut();
            }
        };
        if output.is_null() {
            error(&mut state, ERRNO_EFAULT);
            return ptr::null_mut();
        }
        let mut done = 0;
        while done < size as usize - 1 {
            let mut byte = 0;
            if unsafe { read_bytes(&mut state, &mut byte, 1) } == 0 {
                if !state.eof || done == 0 {
                    return ptr::null_mut();
                }
                break;
            }
            unsafe {
                output.add(done).write(byte as c_char);
            }
            done += 1;
            if byte == b'\n' {
                break;
            }
        }
        unsafe {
            output.add(done).write(0);
        }
        output
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fputs(input: *const c_char, stream: *mut File) -> c_int {
        let mut state = match unsafe { lock(stream) } {
            Ok(state) => state,
            Err(code) => return fail(code),
        };
        if !state.write {
            error(&mut state, ERRNO_EBADF);
            return EOF;
        }
        if input.is_null() {
            error(&mut state, ERRNO_EFAULT);
            return EOF;
        }
        let input = unsafe { CStr::from_ptr(input) }.to_bytes();
        if unsafe { write_bytes(&mut state, input.as_ptr(), input.len()) } == input.len() {
            0
        } else {
            EOF
        }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn puts(input: *const c_char) -> c_int {
        let mut state = match unsafe { lock(stdout) } {
            Ok(state) => state,
            Err(code) => return fail(code),
        };
        if input.is_null() {
            error(&mut state, ERRNO_EFAULT);
            return EOF;
        }
        let input = unsafe { CStr::from_ptr(input) }.to_bytes();
        if unsafe { write_bytes(&mut state, input.as_ptr(), input.len()) } != input.len() {
            return EOF;
        }
        if unsafe { write_bytes(&mut state, b"\n".as_ptr(), 1) } == 1 {
            0
        } else {
            EOF
        }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn vfprintf(
        stream: *mut File,
        format: *const c_char,
        args: VaList<'_>,
    ) -> c_int {
        let mut state = match unsafe { lock(stream) } {
            Ok(state) => state,
            Err(code) => return fail(code),
        };
        if !state.write {
            error(&mut state, ERRNO_EBADF);
            return EOF;
        }
        let result = unsafe {
            formatting::format(format, args, |chunk| {
                match chunk {
                    formatting::Chunk::Bytes(bytes) => {
                        if write_bytes(&mut state, bytes.as_ptr(), bytes.len()) != bytes.len() {
                            return Err(errno());
                        }
                    }
                    formatting::Chunk::Repeat(byte, mut count) => {
                        let bytes = [byte; 256];
                        while count != 0 {
                            let size = count.min(bytes.len());
                            if write_bytes(&mut state, bytes.as_ptr(), size) != size {
                                return Err(errno());
                            }
                            count -= size;
                        }
                    }
                }
                Ok(())
            })
        };
        match result {
            Ok(count) => count as c_int,
            Err(code) => {
                error(&mut state, code);
                EOF
            }
        }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fprintf(stream: *mut File, format: *const c_char, args: ...) -> c_int {
        unsafe { vfprintf(stream, format, args) }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn vprintf(format: *const c_char, args: VaList<'_>) -> c_int {
        unsafe { vfprintf(stdout, format, args) }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn printf(format: *const c_char, args: ...) -> c_int {
        unsafe { vfprintf(stdout, format, args) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modes_accept_ordered_binary_update_and_exclusive_create() {
        assert_eq!(parse_mode(b"rb+"), parse_mode(b"r+b"));
        assert_eq!(
            parse_mode(b"wb+x").unwrap().flags,
            2 | O_CREAT | O_TRUNC | 0x80
        );
        let append = parse_mode(b"a+e").unwrap();
        assert!(append.read && append.write && append.append);
        assert_eq!(append.flags, 2 | O_CREAT | O_APPEND | 0x80000);
    }
    #[test]
    fn malformed_modes_are_rejected() {
        for mode in [
            b"".as_slice(),
            b"rr",
            b"r++",
            b"rbb",
            b"rx",
            b"wxx",
            b"a,x",
            b"rbjunk",
        ] {
            assert_eq!(parse_mode(mode), Err(ERRNO_EINVAL));
        }
    }
}
