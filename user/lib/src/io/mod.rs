//! I/O module for Scarlet user programs
//!
//! This module provides both low-level I/O utilities and high-level
//! Rust standard library-compatible interfaces.

// I/O error handling
use core::fmt;

/// A specialized Result type for I/O operations
pub type Result<T> = core::result::Result<T, Error>;

/// The error type for I/O operations
#[derive(Debug, Clone)]
pub struct Error {
    kind: ErrorKind,
    message: &'static str,
}

impl Error {
    /// Create a new I/O error
    pub fn new(kind: ErrorKind, message: &'static str) -> Self {
        Self { kind, message }
    }
    
    /// Return the kind of this error
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

/// A list specifying general categories of I/O error
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// An entity was not found, often a file
    NotFound,
    /// The operation lacked the necessary privileges to complete
    PermissionDenied,
    /// The connection was refused by the remote server
    ConnectionRefused,
    /// The connection was reset by the remote server
    ConnectionReset,
    /// A non-empty directory was specified where an empty directory was expected
    DirectoryNotEmpty,
    /// The filesystem object is, unexpectedly, a directory
    IsADirectory,
    /// The network operation failed because it was not connected yet
    NotConnected,
    /// An operation could not be completed, because it failed to allocate enough memory
    OutOfMemory,
    /// A parameter was incorrect
    InvalidInput,
    /// Data not valid for the operation were encountered
    InvalidData,
    /// The I/O operation's timeout expired, causing it to be canceled
    TimedOut,
    /// This operation was interrupted
    Interrupted,
    /// This operation is unsupported on this platform
    Unsupported,
    /// An error returned when an operation could not be completed because an "end of file" was reached prematurely
    UnexpectedEof,
    /// Any I/O error not part of this list
    Other,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::NotFound => write!(f, "entity not found"),
            ErrorKind::PermissionDenied => write!(f, "permission denied"),
            ErrorKind::ConnectionRefused => write!(f, "connection refused"),
            ErrorKind::ConnectionReset => write!(f, "connection reset"),
            ErrorKind::DirectoryNotEmpty => write!(f, "directory not empty"),
            ErrorKind::IsADirectory => write!(f, "is a directory"),
            ErrorKind::NotConnected => write!(f, "not connected"),
            ErrorKind::OutOfMemory => write!(f, "out of memory"),
            ErrorKind::InvalidInput => write!(f, "invalid input parameter"),
            ErrorKind::InvalidData => write!(f, "invalid data"),
            ErrorKind::TimedOut => write!(f, "timed out"),
            ErrorKind::Interrupted => write!(f, "operation interrupted"),
            ErrorKind::Unsupported => write!(f, "operation not supported"),
            ErrorKind::UnexpectedEof => write!(f, "unexpected end of file"),
            ErrorKind::Other => write!(f, "other error"),
        }
    }
}

// I/O traits (no_std compatible)

/// The Read trait allows for reading bytes from a source
pub trait Read {
    /// Pull some bytes from this source into the specified buffer
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
}

/// The Write trait represents an object which can write bytes to a sink
pub trait Write {
    /// Write a buffer into this writer, returning how many bytes were written
    fn write(&mut self, buf: &[u8]) -> Result<usize>;
    
    /// Flush this output stream, ensuring that all intermediately buffered data reaches the destination
    fn flush(&mut self) -> Result<()>;
}

/// The Seek trait provides a cursor which can be moved within a stream of bytes
pub trait Seek {
    /// Seek to an offset, in bytes, in a stream
    fn seek(&mut self, pos: SeekFrom) -> Result<u64>;
}

/// Enumeration of possible methods to seek within an I/O object
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekFrom {
    /// Sets the offset to the provided number of bytes from the start of the stream
    Start(u64),
    /// Sets the offset to the provided number of bytes from the end of the stream
    End(i64),
    /// Sets the offset to the provided number of bytes from the current position
    Current(i64),
}

use crate::handle::Handle;

/// Write data to stdout (handle 1)
fn write_to_stdout(data: &[u8]) -> usize {
    // Use handle 1 (stdout) directly
    let stdout = Handle::from_raw(1);
    if let Ok(stream) = stdout.as_stream() {
        match stream.write(data) {
            Ok(bytes_written) => bytes_written,
            Err(_) => 0,
        }
    } else {
        0
    }
}

/// Read data from stdin (handle 0)
fn read_from_stdin(buffer: &mut [u8]) -> usize {
    // Use handle 0 (stdin) directly
    let stdin = Handle::from_raw(0);
    if let Ok(stream) = stdin.as_stream() {
        match stream.read(buffer) {
            Ok(bytes_read) => bytes_read,
            Err(_) => 0,
        }
    } else {
        0
    }
}

/// Outputs a single character to the console
/// 
/// This function uses stdout to output characters.
/// 
/// # Arguments
/// * `c` - The character to output
/// 
/// # Returns
/// The number of bytes written on success, 0 on failure
/// 
pub fn putchar(c: char) -> usize {
    let mut buf = [0u8; 4];
    let char_str = c.encode_utf8(&mut buf);
    write_to_stdout(char_str.as_bytes())
}

/// Reads a single character from the console
/// This function uses stdin to read characters.
/// 
/// # Note
/// This function is blocking and will wait for user input.
/// 
/// # Returns
/// The character read from the console.
/// 
pub fn get_char() -> char {
    let mut buf = [0u8; 1];
    loop {
        let bytes_read = read_from_stdin(&mut buf);
        if bytes_read > 0 {
            return buf[0] as char;
        }
        // If no data available, continue trying
    }
}

/// Outputs a string to the console
/// 
/// # Arguments
/// * `s` - The string to output
/// 
/// # Returns
/// The number of characters output
pub fn puts(s: &str) -> usize {
    write_to_stdout(s.as_bytes())
}

/// Print implementation for Scarlet
pub fn _print(args: fmt::Arguments) {
    use fmt::Write;
    
    let mut writer = StdoutWriter;
    writer.write_fmt(args).unwrap();
}

/// A simple writer that outputs to stdout
struct StdoutWriter;

impl fmt::Write for StdoutWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_to_stdout(s.as_bytes());
        Ok(())
    }
}

/// Macro for printing to stdout
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::io::_print(format_args!($($arg)*)));
}

/// Macro for printing to stdout with a newline
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}
