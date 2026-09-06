//! File system abstraction for Scarlet Native API
//!
//! This module provides a Rust standard library-like file system interface
//! using the OpenOptions builder pattern and high-level convenience functions.
//! It belongs to the legacy `scarlet-std` facade, not Rust `std::fs`; similar
//! method names do not imply identical atomicity or error-reporting guarantees.
//! Examples target legacy `no_std` executables and are compile-only: mounting,
//! removing files, and other system operations must not run as host doctests.
//!
//! ## Core Functions
//!
//! ### File Operations
//! - [`File::open`], [`File::create`]: Open and create files
//! - [`OpenOptions`]: Flexible file opening with various options
//!
//! ### Directory Operations  
//! - [`change_directory`]: Change current working directory
//! - [`File::read_dir`]: Read directory entries from an open directory
//! - [`list_directory`]: List all entries in a directory (convenience function)
//! - [`count_directory_entries`]: Count files and directories (example function)
//!
//! ### Directory Entry Parsing
//! - [`DirectoryEntry`]: High-level directory entry structure
//! - [`DirectoryEntryRaw`]: Low-level raw directory entry structure
//! - [`parse_dir_entry`]: Parse raw directory entry data
//! - [`parse_dir_entry_safe`]: Safe directory entry parsing (backward compatibility)
//!
//! ### Filesystem Operations
//! - [`mount`]: Mount filesystems with various options
//! - [`unmount`]: Unmount filesystems
//! - [`pivot_root`]: Change root filesystem (system initialization)

use crate::handle::Handle;
use crate::handle::capability::SeekFrom as ScarletSeekFrom;
use crate::handle::capability::StreamError;
use crate::io::{Error, ErrorKind, Read, Result, Seek, SeekFrom, Write};
use crate::string::String;

fn stream_error_to_io(error: StreamError, message: &'static str) -> Error {
    let kind = match error {
        StreamError::Interrupted => ErrorKind::Interrupted,
        StreamError::WouldBlock => ErrorKind::WouldBlock,
        StreamError::EndOfStream => ErrorKind::UnexpectedEof,
        StreamError::PermissionDenied => ErrorKind::PermissionDenied,
        StreamError::InvalidParameter => ErrorKind::InvalidInput,
        StreamError::Unsupported => ErrorKind::Unsupported,
        _ => ErrorKind::Other,
    };
    Error::new(kind, message)
}

/// Options and flags which can be used to configure how a file is opened
///
/// This builder exposes the ability to configure how a [`File`] is opened
/// and what operations are permitted on the open file. The [`File::open`]
/// and [`File::create`] methods provide commonly used options
/// using this builder.
///
/// # Examples
///
/// Opening a file to read:
///
/// ```no_run
/// #![no_std]
/// #![no_main]
/// # #[unsafe(no_mangle)]
/// # extern "C" fn main() -> i32 {
/// #     example().expect("filesystem operation failed");
/// #     0
/// # }
/// # fn example() -> scarlet_std::io::Result<()> {
/// use scarlet_std::fs::OpenOptions;
///
/// let file = OpenOptions::new()
///     .read(true)
///     .open("foo.txt")?;
/// # Ok(())
/// # }
/// ```
///
/// Opening a file for both reading and writing, creating it if it doesn't exist:
///
/// ```no_run
/// #![no_std]
/// #![no_main]
/// # #[unsafe(no_mangle)]
/// # extern "C" fn main() -> i32 {
/// #     example().expect("filesystem operation failed");
/// #     0
/// # }
/// # fn example() -> scarlet_std::io::Result<()> {
/// use scarlet_std::fs::OpenOptions;
///
/// let file = OpenOptions::new()
///     .read(true)
///     .write(true)
///     .create(true)
///     .open("foo.txt")?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
}

impl OpenOptions {
    /// Creates a blank new set of options ready for configuration
    ///
    /// All options are initially set to `false`.
    /// The legacy flag encoder treats an unset access mode as read-only, unlike
    /// Rust `std::fs::OpenOptions`, which requires an explicit access mode.
    ///
    /// # Returns
    /// An options builder; no file is opened yet.
    pub fn new() -> Self {
        Self {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
        }
    }

    /// Sets the option for read access
    ///
    /// This option, when true, will indicate that the file should be
    /// readable if opened.
    /// When combining reading with append in this legacy implementation, also
    /// set `.write(true)`; `.read(true).append(true)` alone encodes write-only access.
    ///
    /// # Arguments
    /// * `read` - Whether to request read access.
    ///
    /// # Returns
    /// The same builder for chaining; no I/O is performed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// #![no_std]
    /// #![no_main]
    /// use scarlet_std::fs::OpenOptions;
    ///
    /// # #[unsafe(no_mangle)]
    /// # extern "C" fn main() -> i32 {
    /// let file = OpenOptions::new().read(true).open("foo.txt");
    /// # 0
    /// # }
    /// ```
    pub fn read(&mut self, read: bool) -> &mut Self {
        self.read = read;
        self
    }

    /// Sets the option for write access
    ///
    /// This option, when true, will indicate that the file should be
    /// writable if opened.
    ///
    /// Without append mode, writes overwrite bytes at the current cursor. Setting
    /// this flag alone does not truncate an existing file.
    ///
    /// # Arguments
    /// * `write` - Whether to request write access.
    ///
    /// # Returns
    /// The same builder for chaining; no I/O is performed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// #![no_std]
    /// #![no_main]
    /// # #[unsafe(no_mangle)]
    /// # extern "C" fn main() -> i32 {
    /// #     example().expect("filesystem operation failed");
    /// #     0
    /// # }
    /// # fn example() -> scarlet_std::io::Result<()> {
    /// use scarlet_std::fs::OpenOptions;
    ///
    /// let file = OpenOptions::new().write(true).open("foo.txt");
    /// # Ok(())
    /// # }
    /// ```
    pub fn write(&mut self, write: bool) -> &mut Self {
        self.write = write;
        self
    }

    /// Sets the option for the append mode
    ///
    /// This option requests `O_APPEND`, so supporting file implementations place
    /// writes at the end instead of overwriting bytes at the current cursor.
    /// For append-only access, `.append(true)` is sufficient. To read as well,
    /// this legacy flag encoder requires `.read(true).write(true).append(true)`.
    ///
    /// This wrapper does not make a sequence of writes atomic or prevent readers
    /// from observing partial content. Individual writes may be short; concurrent
    /// append behavior depends on the kernel file implementation.
    ///
    /// # Arguments
    /// * `append` - Whether to request append mode.
    ///
    /// # Returns
    /// The same builder for chaining; no I/O is performed.
    ///
    /// ## Note
    ///
    /// This function doesn't create the file if it doesn't exist. Use the
    /// [`OpenOptions::create`] method to do so.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// #![no_std]
    /// #![no_main]
    /// # #[unsafe(no_mangle)]
    /// # extern "C" fn main() -> i32 {
    /// #     example().expect("filesystem operation failed");
    /// #     0
    /// # }
    /// # fn example() -> scarlet_std::io::Result<()> {
    /// use scarlet_std::fs::OpenOptions;
    ///
    /// let file = OpenOptions::new().append(true).open("foo.txt");
    /// # Ok(())
    /// # }
    /// ```
    pub fn append(&mut self, append: bool) -> &mut Self {
        self.append = append;
        self
    }

    /// Sets the option for truncating a previous file
    ///
    /// If a file is successfully opened with this option set it will truncate
    /// the file to 0 length if it already exists.
    ///
    /// The file must be opened with write access for truncate to work.
    /// This builder passes `O_TRUNC` through and does not validate its combination
    /// with the access mode; the kernel file implementation decides the outcome.
    ///
    /// # Arguments
    /// * `truncate` - Whether to request truncation when opening the file.
    ///
    /// # Returns
    /// The same builder for chaining; truncation is deferred until open.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// #![no_std]
    /// #![no_main]
    /// # #[unsafe(no_mangle)]
    /// # extern "C" fn main() -> i32 {
    /// #     example().expect("filesystem operation failed");
    /// #     0
    /// # }
    /// # fn example() -> scarlet_std::io::Result<()> {
    /// use scarlet_std::fs::OpenOptions;
    ///
    /// let file = OpenOptions::new().write(true).truncate(true).open("foo.txt");
    /// # Ok(())
    /// # }
    /// ```
    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.truncate = truncate;
        self
    }

    /// Sets the option to create a new file, or open it if it already exists
    ///
    /// In order for the file to be created, [`OpenOptions::write`] or
    /// [`OpenOptions::append`] access must be used.
    /// Creation is attempted before a separate open operation. A creation error
    /// is ignored for this option so that an existing file can still be opened;
    /// this is not an atomic create-and-open operation.
    ///
    /// # Arguments
    /// * `create` - Whether to attempt creation before opening.
    ///
    /// # Returns
    /// The same builder for chaining; no file is created yet.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// #![no_std]
    /// #![no_main]
    /// # #[unsafe(no_mangle)]
    /// # extern "C" fn main() -> i32 {
    /// #     example().expect("filesystem operation failed");
    /// #     0
    /// # }
    /// # fn example() -> scarlet_std::io::Result<()> {
    /// use scarlet_std::fs::OpenOptions;
    ///
    /// let file = OpenOptions::new().write(true).create(true).open("foo.txt");
    /// # Ok(())
    /// # }
    /// ```
    pub fn create(&mut self, create: bool) -> &mut Self {
        self.create = create;
        self
    }

    /// Sets the option to create a new file, failing if it already exists
    ///
    /// The legacy implementation first calls `VfsCreateFile` and treats a creation
    /// failure as an error, then opens the path in a separate operation. Creation
    /// errors are reported as `ErrorKind::Other` with a generic "File already exists"
    /// message even when the cause is not an existing file.
    ///
    /// Unlike Rust `std::fs::OpenOptions::create_new`, this is not an atomic
    /// create-and-open operation. Another task can replace the path between those
    /// calls, so the returned handle is not guaranteed to refer to the newly
    /// created file. Do not use it as a TOCTOU-safe exclusive-creation primitive.
    ///
    /// With `.create_new(true)`, [`.create()`] is redundant, but [`.truncate()`]
    /// is still passed to the subsequent open; it is not ignored.
    ///
    /// The file must be opened with write or append access in order to create
    /// a new file.
    ///
    /// # Arguments
    /// * `create_new` - Whether failure of the preliminary creation should abort open.
    ///
    /// # Returns
    /// The same builder for chaining; no file is created yet.
    ///
    /// [`.create()`]: OpenOptions::create
    /// [`.truncate()`]: OpenOptions::truncate
    ///
    /// # Examples
    ///
    /// ```no_run
    /// #![no_std]
    /// #![no_main]
    /// # #[unsafe(no_mangle)]
    /// # extern "C" fn main() -> i32 {
    /// #     example().expect("filesystem operation failed");
    /// #     0
    /// # }
    /// # fn example() -> scarlet_std::io::Result<()> {
    /// use scarlet_std::fs::OpenOptions;
    ///
    /// let file = OpenOptions::new().write(true).create_new(true).open("foo.txt");
    /// # Ok(())
    /// # }
    /// ```
    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.create_new = create_new;
        self
    }

    /// Opens a file at `path` with the options specified by `self`
    ///
    /// # Arguments
    /// * `path` - File path in the current task's VFS namespace.
    ///
    /// # Returns
    /// An owning file wrapper, or an I/O error. A preliminary creation can leave
    /// a file behind even if the later open fails; there is no rollback.
    ///
    /// # Errors
    ///
    /// This function will return an error under a number of different
    /// circumstances. Some of these error conditions are listed here, together
    /// with their [`ErrorKind`]. This legacy wrapper does not preserve detailed
    /// kernel errors and must not be used to distinguish all path/permission failures.
    ///
    /// * [`InvalidInput`]: Creation was requested without write/append access,
    ///   or a creation path contains an interior NUL byte.
    /// * [`Other`]: The preliminary `create_new` operation or subsequent handle
    ///   open failed. Missing files or directory components and denied access
    ///   are not distinguished as [`NotFound`] or [`PermissionDenied`] here.
    /// * [`Unsupported`]: The opened handle does not expose file operations.
    ///
    /// An unset access mode is encoded as read-only, and truncate/access-mode
    /// combinations are passed to the kernel rather than rejected by this builder.
    ///
    /// [`ErrorKind`]: crate::io::ErrorKind
    /// [`InvalidInput`]: ErrorKind::InvalidInput
    /// [`Other`]: ErrorKind::Other
    /// [`Unsupported`]: ErrorKind::Unsupported
    /// [`NotFound`]: ErrorKind::NotFound
    /// [`PermissionDenied`]: ErrorKind::PermissionDenied
    ///
    /// # Examples
    ///
    /// ```no_run
    /// #![no_std]
    /// #![no_main]
    /// # #[unsafe(no_mangle)]
    /// # extern "C" fn main() -> i32 {
    /// #     example().expect("filesystem operation failed");
    /// #     0
    /// # }
    /// # fn example() -> scarlet_std::io::Result<()> {
    /// use scarlet_std::fs::OpenOptions;
    ///
    /// let file = OpenOptions::new().read(true).open("foo.txt");
    /// # Ok(())
    /// # }
    /// ```
    pub fn open<P: AsRef<str>>(&self, path: P) -> Result<File> {
        use crate::ffi::str_to_cstr_bytes;
        use crate::syscall::{Syscall, syscall2};

        // If we need to create the file, use VfsCreateFile first
        if self.create || self.create_new {
            // Check if we have write access
            if !self.write && !self.append {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "Cannot create file without write access",
                ));
            }

            // Convert path to null-terminated C string
            let path_bytes = str_to_cstr_bytes(path.as_ref())
                .map_err(|_| Error::new(ErrorKind::InvalidInput, "path contains null byte"))?;

            // For create_new, we should check if file exists first
            // For now, just attempt to create and handle errors
            // SAFETY: The NUL-terminated path remains readable until return; mode is a scalar.
            let result = unsafe {
                syscall2(
                    Syscall::VfsCreateFile,
                    path_bytes.as_ptr() as usize,
                    0, // mode (unused for now)
                )
            };

            // For create_new, creation failure is an error
            // For create, we continue even if creation fails (file might already exist)
            if self.create_new && result == usize::MAX {
                return Err(Error::new(ErrorKind::Other, "File already exists"));
            }
        }

        // Construct open flags from options
        // Flag values match POSIX-style constants used by the kernel:
        //   O_RDONLY = 0x0, O_WRONLY = 0x1, O_RDWR = 0x2
        //   O_APPEND = 0x400, O_TRUNC = 0x200
        let flags = if self.read && self.write {
            0x2 // O_RDWR
        } else if self.write || self.append {
            0x1 // O_WRONLY
        } else {
            0x0 // O_RDONLY
        };

        let flags = if self.append { flags | 0x400 } else { flags };
        let flags = if self.truncate { flags | 0x200 } else { flags };

        // Use Handle::open and wrap in File
        let handle = Handle::open(path.as_ref(), flags)
            .map_err(|_| Error::new(ErrorKind::Other, "Failed to open file"))?;

        File::from_handle(handle)
    }
}

impl Default for OpenOptions {
    /// Creates a blank new set of options ready for configuration
    ///
    /// This is equivalent to [`OpenOptions::new()`].
    fn default() -> Self {
        Self::new()
    }
}

// File system types and structures

/// High-level File wrapper with automatic resource management
///
/// This provides a Rust standard library-like interface while using
/// Scarlet Native capabilities under the hood. The file is automatically
/// closed when the File instance is dropped.
///
/// Files are not cloneable to ensure clear ownership semantics.
pub struct File {
    handle: Handle,
}

impl File {
    /// Create a File from an existing Handle
    ///
    /// This is used internally by OpenOptions and other high-level APIs.
    ///
    /// # Arguments
    /// * `handle` - Owned handle to consume, including when validation fails.
    ///
    /// # Returns
    /// A `File` on success.
    ///
    /// This performs a type check using the handle's cached kernel object info.
    /// If the handle does not represent a file-like object, this returns
    /// `ErrorKind::Unsupported` and drops the consumed handle, closing it.
    pub fn from_handle(handle: Handle) -> Result<Self> {
        handle.as_file().map_err(|_| {
            Error::new(
                ErrorKind::Unsupported,
                "Object does not support file operations",
            )
        })?;
        Ok(File { handle })
    }

    /// Open a file with automatic resource management
    ///
    /// This is a convenience method. For more control over file opening options,
    /// use OpenOptions.
    ///
    /// # Arguments
    /// * `path` - Path to the file
    ///
    /// # Returns
    /// File instance or error
    pub fn open<P: AsRef<str>>(path: P) -> Result<Self> {
        // Open for read-only
        let handle = Handle::open(path.as_ref(), 0x0) // O_RDONLY
            .map_err(|_| Error::new(ErrorKind::Other, "Failed to open file"))?;
        File::from_handle(handle)
    }

    /// Create a new file (equivalent to open with create, write, truncate)
    ///
    /// This is a convenience method. For more control over file creation options,
    /// use OpenOptions.
    ///
    /// # Arguments
    /// * `path` - Path to the file to create
    ///
    /// # Returns
    /// File instance or error
    pub fn create<P: AsRef<str>>(path: P) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        options.open(path)
    }

    /// Open a file with specific flags (low-level interface)
    ///
    /// This method provides direct access to system-level flags.
    /// Prefer using [`File::open`], [`File::create`], or [`OpenOptions`]
    /// for most use cases.
    ///
    /// # Arguments
    /// * `path` - Path to the file
    /// * `flags` - Open flags (implementation-specific)
    ///
    /// # Returns
    /// File instance or error
    pub fn open_with_flags<P: AsRef<str>>(path: P, flags: usize) -> Result<Self> {
        let handle = Handle::open(path.as_ref(), flags)
            .map_err(|_| Error::new(ErrorKind::Other, "Failed to open file"))?;
        File::from_handle(handle)
    }

    /// Get the underlying handle (for advanced usage)
    ///
    /// This allows access to the low-level Handle and its capabilities
    /// when you need more control than the high-level File interface provides.
    pub fn as_handle(&self) -> &Handle {
        &self.handle
    }

    /// Convert the File into a Handle
    ///
    /// This consumes the File and returns the underlying Handle.
    ///
    /// # Returns
    /// Handle instance
    pub fn into_handle(self) -> Handle {
        self.handle
    }

    /// Clone the underlying handle via duplication
    ///
    /// This requires a syscall and creates an independently closeable handle to
    /// the same kernel object. It does not copy the file contents or guarantee an
    /// independent seek cursor or open-file state.
    ///
    /// # Returns
    /// Cloned Handle instance or error
    pub fn clone_handle(&self) -> Result<Handle> {
        self.handle
            .duplicate()
            .map_err(|_| Error::new(ErrorKind::Other, "Failed to duplicate handle"))
    }

    /// Get the raw handle ID
    pub fn as_raw(&self) -> i32 {
        self.handle.as_raw()
    }

    pub fn set_nonblocking(&self, enabled: bool) -> Result<()> {
        const HCTL_SET_NONBLOCKING: u32 = 0x5353_0007;
        // SAFETY: This fixed socket control takes a scalar argument and borrows the live handle; it carries no raw pointer.
        let result = unsafe {
            crate::syscall::syscall3(
                crate::syscall::Syscall::HandleControl,
                self.handle.as_raw() as usize,
                HCTL_SET_NONBLOCKING as usize,
                if enabled { 1 } else { 0 },
            )
        };
        if result == usize::MAX {
            return Err(Error::new(ErrorKind::Other, "set_nonblocking failed"));
        }
        Ok(())
    }
}

// Implement Rust standard library-like methods
impl File {
    /// Read data from the file
    ///
    /// # Arguments
    /// * `buf` - Buffer to read data into
    ///
    /// # Returns
    /// Number of bytes read, which can be shorter than `buf.len()`, or an I/O
    /// error. A zero-byte read can indicate EOF or an empty destination buffer.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let stream = self.handle.as_stream().map_err(|_| {
            Error::new(
                ErrorKind::Unsupported,
                "Object does not support stream operations",
            )
        })?;

        stream
            .read(buf)
            .map_err(|error| stream_error_to_io(error, "Read operation failed"))
    }

    /// Read directory entries from a directory file
    ///
    /// This method reads one serialized entry from the directory stream. Use
    /// [`list_directory`] to collect all entries into a vector.
    ///
    /// # Arguments
    /// * `self` - An open directory file; the stream position advances on a read.
    ///
    /// # Returns
    /// * `Ok(Some(entry))` - One parsed directory entry.
    /// * `Ok(None)` - End of the directory stream.
    /// * `Err(error)` - I/O failure or an invalid serialized entry, not a raw errno.
    ///
    /// # Panics
    /// Panics if the underlying handle does not expose stream operations; this
    /// legacy implementation does not turn that capability failure into an I/O error.
    pub fn read_dir(&mut self) -> Result<Option<DirectoryEntry>> {
        // let file_handle = self.handle.as_file()
        //     .map_err(|_| Error::new(ErrorKind::Unsupported, "Object does not support file operations"))?;
        // let metadata = file_handle.metadata()
        //     .map_err(|_| Error::new(ErrorKind::Other, "Failed to get file metadata"))?;

        // crate::println!("metadata: {:?}", metadata);

        // if !metadata.is_directory() {
        //     return Err(Error::new(ErrorKind::InvalidInput, "Handle is not a directory"));
        // }

        let mut buf = [0u8; core::mem::size_of::<DirectoryEntryRaw>()];
        let bytes_read = self.handle.as_stream().unwrap().read(&mut buf);

        if bytes_read.is_err() {
            return Err(Error::new(
                ErrorKind::Other,
                "Failed to read directory entry",
            ));
        }
        let bytes_read = bytes_read.unwrap();

        if bytes_read == 0 {
            return Ok(None); // EOF - no more entries
        }

        // Parse the directory entry
        if let Some(entry) = parse_dir_entry(&buf[..bytes_read]) {
            Ok(Some(DirectoryEntry::from_raw(entry)))
        } else {
            Err(Error::new(
                ErrorKind::InvalidData,
                "Failed to parse directory entry",
            ))
        }
    }

    /// Write data to the file
    ///
    /// # Arguments
    /// * `buf` - Data to write
    ///
    /// # Returns
    /// Number of bytes written, which can be shorter than `buf.len()`, or an I/O
    /// error. Success does not imply that the bytes have reached persistent storage.
    pub fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let stream = self.handle.as_stream().map_err(|_| {
            Error::new(
                ErrorKind::Unsupported,
                "Object does not support stream operations",
            )
        })?;

        stream
            .write(buf)
            .map_err(|error| stream_error_to_io(error, "Write operation failed"))
    }

    /// Write all data to the file
    ///
    /// This convenience method retries short writes until all bytes have been
    /// accepted or an error occurs. On error, an already written prefix remains;
    /// it does not roll back partial writes or guarantee persistence.
    ///
    /// # Arguments
    /// * `buf` - Data to write
    ///
    /// # Returns
    /// Success or error
    pub fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        let stream = self.handle.as_stream().map_err(|_| {
            Error::new(
                ErrorKind::Unsupported,
                "Object does not support stream operations",
            )
        })?;

        stream
            .write_all(buf)
            .map_err(|error| stream_error_to_io(error, "Write all operation failed"))
    }

    /// Seek to a position in the file
    ///
    /// # Arguments
    /// * `pos` - Position to seek to
    ///
    /// # Returns
    /// New absolute position or error
    pub fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let file_obj = self.handle.as_file().map_err(|_| {
            Error::new(
                ErrorKind::Unsupported,
                "Object does not support file operations",
            )
        })?;

        let scarlet_pos = match pos {
            SeekFrom::Start(offset) => ScarletSeekFrom::Start(offset),
            SeekFrom::Current(offset) => ScarletSeekFrom::Current(offset),
            SeekFrom::End(offset) => ScarletSeekFrom::End(offset),
        };

        file_obj
            .seek(scarlet_pos)
            .map_err(|_| Error::new(ErrorKind::Other, "Seek operation failed"))
    }

    /// Truncate the file to the specified size
    ///
    /// # Arguments
    /// * `size` - New size of the file in bytes
    ///
    /// # Returns
    /// Success or error
    pub fn set_len(&mut self, size: u64) -> Result<()> {
        let file_obj = self.handle.as_file().map_err(|_| {
            Error::new(
                ErrorKind::Unsupported,
                "Object does not support file operations",
            )
        })?;

        file_obj
            .truncate(size)
            .map_err(|_| Error::new(ErrorKind::Other, "Truncate operation failed"))
    }

    // /// Get file metadata
    // ///
    // /// # Returns
    // /// File metadata or error
    // pub fn metadata(&self) -> Result<FileMetadata> {
    //     let file_obj = self.handle.as_file()
    //         .map_err(|_| Error::new(ErrorKind::Unsupported, "Object does not support file operations"))?;

    //     file_obj.metadata()
    //         .map_err(|_| Error::new(ErrorKind::Other, "Metadata operation failed"))
    // }

    /// Get the current position in the file
    ///
    /// # Returns
    /// Current position or error
    pub fn stream_position(&mut self) -> Result<u64> {
        self.seek(SeekFrom::Current(0))
    }
}

// Standard library-like traits for compatibility
impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        File::read(self, buf)
    }
}

impl Write for File {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        File::write(self, buf)
    }

    /// Complete a flush request without issuing a kernel storage operation.
    ///
    /// # Returns
    /// `Ok(())`. This wrapper has no userspace write buffer, but the no-op does
    /// not flush kernel or device caches and is not a durability guarantee.
    fn flush(&mut self) -> Result<()> {
        // For now, we don't have explicit flush capability
        // This could be added as a future enhancement
        Ok(())
    }
}

impl Seek for File {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        File::seek(self, pos)
    }
}

//
// Mount Operations
//

/// Mount flags for mount operations
///
/// These flags are passed to the mount() system call to control mount behavior.
pub mod mount_flags {
    /// Mount filesystem read-only
    pub const MS_RDONLY: u32 = 0x01;
    /// Ignore suid and sgid bits
    pub const MS_NOSUID: u32 = 0x02;
    /// Disallow access to device special files
    pub const MS_NODEV: u32 = 0x04;
    /// Disallow program execution
    pub const MS_NOEXEC: u32 = 0x08;
    /// Writes are synced at once
    pub const MS_SYNCHRONOUS: u32 = 0x10;
    /// Bind mount
    pub const MS_BIND: u32 = 0x1000;
}

//
// File system operations
//

/// Mount a filesystem
///
/// # Arguments
///
/// * `source` - Source device or filesystem name (e.g., "/dev/sda1", "tmpfs")
/// * `target` - Target mount point (e.g., "/mnt/data")
/// * `fstype` - Filesystem type (e.g., "ext4", "tmpfs", "bind")
/// * `flags` - Mount flags (see `mount_flags` module)
/// * `data` - Optional filesystem-specific data
///
/// # Examples
///
/// Mount a tmpfs:
/// ```no_run
/// #![no_std]
/// #![no_main]
/// # #[unsafe(no_mangle)]
/// # extern "C" fn main() -> i32 {
/// #     example().expect("filesystem operation failed");
/// #     0
/// # }
/// # fn example() -> scarlet_std::io::Result<()> {
/// use scarlet_std::fs;
///
/// fs::mount("tmpfs", "/tmp", "tmpfs", 0, Some("size=100M"))?;
/// # Ok(())
/// # }
/// ```
///
/// Bind mount:
/// ```no_run
/// #![no_std]
/// #![no_main]
/// # #[unsafe(no_mangle)]
/// # extern "C" fn main() -> i32 {
/// #     example().expect("filesystem operation failed");
/// #     0
/// # }
/// # fn example() -> scarlet_std::io::Result<()> {
/// use scarlet_std::fs::{mount, mount_flags};
///
/// mount("/source/dir", "/target/dir", "bind", mount_flags::MS_BIND, None)?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns `Err` if the mount operation fails, such as:
/// - Invalid mount point
/// - Filesystem type not supported
/// - Permission denied
/// - Mount point already mounted
pub fn mount(
    source: &str,
    target: &str,
    fstype: &str,
    flags: u32,
    data: Option<&str>,
) -> Result<()> {
    use crate::ffi::str_to_cstr_bytes;
    use crate::syscall::{Syscall, syscall5};

    let source_c = str_to_cstr_bytes(source)
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "source contains null byte"))?;
    let target_c = str_to_cstr_bytes(target)
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "target contains null byte"))?;
    let fstype_c = str_to_cstr_bytes(fstype)
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "fstype contains null byte"))?;

    let data_c;
    let data_ptr = if let Some(data_str) = data {
        data_c = str_to_cstr_bytes(data_str)
            .map_err(|_| Error::new(ErrorKind::InvalidInput, "data contains null byte"))?;
        data_c.as_ptr() as usize
    } else {
        0
    };

    // SAFETY: All paths, filesystem type and optional mount data are live NUL-terminated strings borrowed until return.
    let result = unsafe {
        syscall5(
            Syscall::FsMount,
            source_c.as_ptr() as usize,
            target_c.as_ptr() as usize,
            fstype_c.as_ptr() as usize,
            flags as usize,
            data_ptr,
        )
    };

    if result == usize::MAX {
        Err(Error::new(ErrorKind::Other, "mount failed"))
    } else {
        Ok(())
    }
}

/// Unmount a filesystem
///
/// # Arguments
///
/// * `target` - Mount point to unmount (e.g., "/mnt/data")
/// * `flags` - Unmount flags (reserved for future use, pass 0)
///
/// # Examples
///
/// ```no_run
/// #![no_std]
/// #![no_main]
/// # #[unsafe(no_mangle)]
/// # extern "C" fn main() -> i32 {
/// #     example().expect("filesystem operation failed");
/// #     0
/// # }
/// # fn example() -> scarlet_std::io::Result<()> {
/// use scarlet_std::fs::unmount;
///
/// unmount("/mnt/data", 0)?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns `Err` if the unmount operation fails, such as:
/// - Mount point not found
/// - Filesystem busy (files still open)
/// - Permission denied
pub fn unmount(target: &str, flags: u32) -> Result<()> {
    use crate::ffi::str_to_cstr_bytes;
    use crate::syscall::{Syscall, syscall2};

    let target_c = str_to_cstr_bytes(target)
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "target contains null byte"))?;

    // SAFETY: The NUL-terminated mount path remains readable until return; flags is a scalar.
    let result = unsafe {
        syscall2(
            Syscall::FsUmount,
            target_c.as_ptr() as usize,
            flags as usize,
        )
    };

    if result == usize::MAX {
        Err(Error::new(ErrorKind::Other, "unmount failed"))
    } else {
        Ok(())
    }
}

/// Change the root filesystem (pivot_root)
///
/// This system call moves the old root filesystem to `old_root` and makes
/// `new_root` the new root filesystem. This is typically used during system
/// initialization to switch from an initramfs to the real root filesystem.
///
/// # Arguments
///
/// * `new_root` - Path to the new root filesystem
/// * `old_root` - Path where the old root filesystem will be moved
///
/// # Examples
///
/// ```no_run
/// #![no_std]
/// #![no_main]
/// # #[unsafe(no_mangle)]
/// # extern "C" fn main() -> i32 {
/// #     example().expect("filesystem operation failed");
/// #     0
/// # }
/// # fn example() -> scarlet_std::io::Result<()> {
/// use scarlet_std::fs::pivot_root;
///
/// // Switch to new root, moving old root to /old_root
/// pivot_root("/mnt/newroot", "/mnt/newroot/old_root")?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns `Err` if the pivot_root operation fails, such as:
/// - New root path does not exist or is not a mount point
/// - Old root path is invalid
/// - Permission denied
/// - Operation not supported in current namespace
pub fn pivot_root(new_root: &str, old_root: &str) -> Result<()> {
    use crate::ffi::str_to_cstr_bytes;
    use crate::syscall::{Syscall, syscall2};

    let new_root_c = str_to_cstr_bytes(new_root)
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "new_root contains null byte"))?;
    let old_root_c = str_to_cstr_bytes(old_root)
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "old_root contains null byte"))?;

    // SAFETY: Both NUL-terminated paths remain readable until this synchronous operation returns.
    let result = unsafe {
        syscall2(
            Syscall::FsPivotRoot,
            new_root_c.as_ptr() as usize,
            old_root_c.as_ptr() as usize,
        )
    };

    if result == usize::MAX {
        Err(Error::new(ErrorKind::Other, "pivot_root failed"))
    } else {
        Ok(())
    }
}

/// Create a new directory
///
/// This function creates a new directory at the specified path.
///
/// # Arguments
/// * `path` - Path to the new directory
///
pub fn create_directory<P: AsRef<str>>(path: P) -> Result<()> {
    use crate::ffi::str_to_cstr_bytes;
    use crate::syscall::{Syscall, syscall1};

    let path_c = str_to_cstr_bytes(path.as_ref())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "path contains null byte"))?;

    // SAFETY: The NUL-terminated path remains readable until the synchronous directory operation returns.
    let result = unsafe { syscall1(Syscall::VfsCreateDirectory, path_c.as_ptr() as usize) };

    if result == usize::MAX {
        Err(Error::new(ErrorKind::Other, "create directory failed"))
    } else {
        Ok(())
    }
}

/// Change the current working directory
///
/// # Arguments
///
/// * `path` - Path to the new working directory
///
/// # Examples
///
/// ```no_run
/// #![no_std]
/// #![no_main]
/// # #[unsafe(no_mangle)]
/// # extern "C" fn main() -> i32 {
/// #     example().expect("filesystem operation failed");
/// #     0
/// # }
/// # fn example() -> scarlet_std::io::Result<()> {
/// use scarlet_std::fs::change_directory;
///
/// change_directory("/tmp")?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns `Err` if the directory change fails, such as:
/// - Directory does not exist
/// - Permission denied
/// - Invalid path
pub fn change_directory<P: AsRef<str>>(path: P) -> Result<()> {
    use crate::ffi::str_to_cstr_bytes;
    use crate::syscall::{Syscall, syscall1};

    let path_c = str_to_cstr_bytes(path.as_ref())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "path contains null byte"))?;

    // SAFETY: The NUL-terminated path remains readable until the synchronous directory operation returns.
    let result = unsafe { syscall1(Syscall::VfsChangeDirectory, path_c.as_ptr() as usize) };

    if result == usize::MAX {
        Err(Error::new(ErrorKind::Other, "change directory failed"))
    } else {
        Ok(())
    }
}

/// Remove a file
///
/// This function removes a file at the specified path.
///
/// # Arguments
/// * `path` - Path to the file to remove
///
/// # Examples
///
/// ```no_run
/// #![no_std]
/// #![no_main]
/// # #[unsafe(no_mangle)]
/// # extern "C" fn main() -> i32 {
/// #     example().expect("filesystem operation failed");
/// #     0
/// # }
/// # fn example() -> scarlet_std::io::Result<()> {
/// use scarlet_std::fs::remove_file;
///
/// remove_file("old_file.txt")?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns `Err` if the remove operation fails, such as:
/// - File not found
/// - Permission denied
/// - Filesystem is read-only
pub fn remove_file<P: AsRef<str>>(path: P) -> Result<()> {
    use crate::ffi::str_to_cstr_bytes;
    use crate::syscall::{Syscall, syscall1};

    let path_c = str_to_cstr_bytes(path.as_ref())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "path contains null byte"))?;

    // SAFETY: The NUL-terminated path remains readable until the synchronous removal returns.
    let result = unsafe { syscall1(Syscall::VfsRemove, path_c.as_ptr() as usize) };

    if result == usize::MAX {
        Err(Error::new(ErrorKind::Other, "remove file failed"))
    } else {
        Ok(())
    }
}

/// Remove a directory
///
/// This function removes a directory at the specified path.
/// The directory must be empty to be removed successfully.
///
/// # Arguments
/// * `path` - Path to the directory to remove
///
/// # Examples
///
/// ```no_run
/// #![no_std]
/// #![no_main]
/// # #[unsafe(no_mangle)]
/// # extern "C" fn main() -> i32 {
/// #     example().expect("filesystem operation failed");
/// #     0
/// # }
/// # fn example() -> scarlet_std::io::Result<()> {
/// use scarlet_std::fs::remove_directory;
///
/// remove_directory("empty_dir")?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns `Err` if the remove operation fails, such as:
/// - Directory not found
/// - Directory not empty
/// - Permission denied
/// - Filesystem is read-only
pub fn remove_directory<P: AsRef<str>>(path: P) -> Result<()> {
    use crate::ffi::str_to_cstr_bytes;
    use crate::syscall::{Syscall, syscall1};

    let path_c = str_to_cstr_bytes(path.as_ref())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "path contains null byte"))?;

    // SAFETY: The NUL-terminated path remains readable until the synchronous removal returns.
    let result = unsafe { syscall1(Syscall::VfsRemove, path_c.as_ptr() as usize) };

    if result == usize::MAX {
        Err(Error::new(ErrorKind::Other, "remove directory failed"))
    } else {
        Ok(())
    }
}

/// Raw Directory entry structure (must match kernel definition)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DirectoryEntryRaw {
    /// Unique file identifier
    pub file_id: u64,
    /// File size in bytes
    pub size: u64,
    /// File type as a byte value
    pub file_type: u8,
    /// Length of the file name
    pub name_len: u8,
    /// Reserved bytes for alignment
    pub _reserved: [u8; 6],
    /// File name (null-terminated, max 255 characters)
    pub name: [u8; 256],
}

impl DirectoryEntryRaw {
    /// Get the name as a string
    pub fn name_str(&self) -> core::result::Result<&str, core::str::Utf8Error> {
        let name_bytes = &self.name[..self.name_len as usize];
        core::str::from_utf8(name_bytes)
    }

    /// Get the name as an owned String
    pub fn name_string(&self) -> core::result::Result<crate::string::String, core::str::Utf8Error> {
        let name_str = self.name_str()?;
        let mut owned_name = crate::string::String::new();
        for c in name_str.chars() {
            owned_name.push(c);
        }
        Ok(owned_name)
    }

    /// Check if this entry is a directory
    pub fn is_directory(&self) -> bool {
        self.file_type == 1 // FileType::Directory as u8
    }

    /// Check if this entry is a regular file
    pub fn is_file(&self) -> bool {
        self.file_type == 0 // FileType::RegularFile as u8
    }

    /// Check if this entry is a symbolic link
    pub fn is_symlink(&self) -> bool {
        self.file_type == 2 // FileType::SymbolicLink as u8
    }

    /// Get file type as a human-readable string
    pub fn file_type_str(&self) -> &'static str {
        match self.file_type {
            0 => "file",
            1 => "directory",
            2 => "symlink",
            3 => "device",
            4 => "pipe",
            5 => "socket",
            _ => "unknown",
        }
    }
}

/// Directory entry structure for user space
/// This structure is a higher-level representation of a directory entry
/// that can be used in user space

#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    /// Unique file identifier
    pub file_id: u64,
    /// File size in bytes
    pub size: u64,
    /// File type as a byte value
    pub file_type: u8,
    /// File name
    pub name: String,
}

impl DirectoryEntry {
    /// Create a new DirectoryEntry from raw data
    pub fn from_raw(entry: DirectoryEntryRaw) -> Self {
        Self {
            file_id: entry.file_id,
            size: entry.size,
            file_type: entry.file_type,
            name: entry.name_string().unwrap_or_else(|_| String::new()),
        }
    }

    /// Get the name as a string slice
    pub fn name_str(&self) -> &str {
        &self.name
    }

    /// Check if this entry is a directory
    pub fn is_directory(&self) -> bool {
        self.file_type == 1 // FileType::Directory as u8
    }

    /// Check if this entry is a regular file
    pub fn is_file(&self) -> bool {
        self.file_type == 0 // FileType::RegularFile as u8
    }

    /// Check if this entry is a symbolic link
    pub fn is_symlink(&self) -> bool {
        self.file_type == 2 // FileType::SymbolicLink as u8
    }
}

/// Helper function to parse directory entries from readdir buffer (backward compatibility)
///
/// This function is kept for backward compatibility with older code that manually
/// handles directory entry parsing. Consider using [`File::read_dir`] or
/// [`list_directory`] for new code, which handle parsing automatically.
///
/// # Arguments
/// * `buf` - Buffer containing directory entry from readdir
/// * `bytes_read` - Number of bytes actually read
///
/// # Returns
/// * `Some((name, file_type, file_id, size))` - Parsed directory entry data
/// * `None` - If parsing failed or EOF reached
///
pub fn parse_dir_entry_safe(
    buf: &[u8],
    bytes_read: usize,
) -> Option<(crate::string::String, u8, u64, u64)> {
    if bytes_read == 0 {
        return None; // EOF
    }

    if let Some(entry) = parse_dir_entry(&buf[..bytes_read])
        && let Ok(owned_name) = entry.name_string()
    {
        return Some((owned_name, entry.file_type, entry.file_id, entry.size));
    }

    None
}

/// Parse a single directory entry from buffer (low-level function)
pub fn parse_dir_entry(buf: &[u8]) -> Option<DirectoryEntryRaw> {
    if buf.len() < core::mem::size_of::<DirectoryEntryRaw>() {
        return None;
    }

    unsafe {
        Some(core::ptr::read_unaligned(
            buf.as_ptr() as *const DirectoryEntryRaw
        ))
    }
}

/// List all files and directories in a directory
///
/// This is a convenience function that opens a directory and reads all entries.
/// It demonstrates how to use the new directory reading API.
///
/// # Arguments
/// * `path` - Path to the directory to list
///
/// # Returns
/// * `Ok(entries)` - Vector of directory entries on success
/// * `Err(error)` - I/O error on failure
///
/// # Examples
///
/// ```no_run
/// #![no_std]
/// #![no_main]
/// # #[unsafe(no_mangle)]
/// # extern "C" fn main() -> i32 {
/// #     example().expect("filesystem operation failed");
/// #     0
/// # }
/// # fn example() -> scarlet_std::io::Result<()> {
/// use scarlet_std::println;
/// use scarlet_std::fs;
///
/// let entries = fs::list_directory("/tmp")?;
/// for entry in entries {
///     println!("{}: {} bytes", entry.name, entry.size);
/// }
/// # Ok(())
/// # }
/// ```
///
pub fn list_directory(path: &str) -> Result<crate::vec::Vec<DirectoryEntry>> {
    use crate::vec::Vec;

    let dir_file = File::open(path);
    if dir_file.is_err() {
        return Err(dir_file.err().unwrap());
    }

    let mut entries = Vec::new();

    let mut file = dir_file.unwrap();

    loop {
        match file.read_dir() {
            Ok(Some(entry)) => {
                entries.push(entry);
            }
            Ok(None) => break, // EOF
            Err(errno) => {
                return Err(errno);
            }
        }
    }

    Ok(entries)
}

/// Count files and directories in a directory
///
/// This is an example function that demonstrates using the directory listing API
/// to analyze directory contents.
///
/// # Arguments
/// * `path` - Path to the directory to analyze
///
/// # Returns
/// * `Ok((file_count, dir_count))` - Tuple of (number of files, number of directories)
/// * `Err(error)` - I/O error on failure
///
/// # Examples
///
/// ```no_run
/// #![no_std]
/// #![no_main]
/// # #[unsafe(no_mangle)]
/// # extern "C" fn main() -> i32 {
/// #     example().expect("filesystem operation failed");
/// #     0
/// # }
/// # fn example() -> scarlet_std::io::Result<()> {
/// use scarlet_std::println;
/// use scarlet_std::fs;
///
/// let (files, dirs) = fs::count_directory_entries("/home")?;
/// println!("Found {} files and {} directories", files, dirs);
/// # Ok(())
/// # }
/// ```
///
pub fn count_directory_entries(path: &str) -> Result<(usize, usize)> {
    let entries = list_directory(path)?;

    let mut file_count = 0;
    let mut dir_count = 0;

    for entry in entries {
        if entry.is_file() {
            file_count += 1;
        } else if entry.is_directory() {
            dir_count += 1;
        }
    }

    Ok((file_count, dir_count))
}

/// Create a symbolic link at the specified path pointing to the target
///
/// # Arguments
/// * `symlink_path` - The path where the symbolic link will be created
/// * `target_path` - The path that the symbolic link will point to
///
/// # Returns
/// * `Ok(())` - If the symbolic link was created successfully
/// * `Err(Error)` - If the symbolic link could not be created
///
/// # Example
/// ```no_run
/// #![no_std]
/// #![no_main]
/// # #[unsafe(no_mangle)]
/// # extern "C" fn main() -> i32 {
/// #     example().expect("filesystem operation failed");
/// #     0
/// # }
/// # fn example() -> scarlet_std::io::Result<()> {
/// use scarlet_std::fs::create_symlink;
///
/// create_symlink("/path/to/symlink", "/path/to/target")?;
/// # Ok(())
/// # }
/// ```
pub fn create_symlink(symlink_path: &str, target_path: &str) -> Result<()> {
    use crate::ffi::str_to_cstr_bytes;
    use crate::syscall::{Syscall, syscall4};

    let symlink_path_c = str_to_cstr_bytes(symlink_path)
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "symlink_path contains null byte"))?;
    let target_path_c = str_to_cstr_bytes(target_path)
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "target_path contains null byte"))?;

    // SAFETY: Both NUL-terminated paths remain readable until the synchronous link operation returns.
    let result = unsafe {
        syscall4(
            Syscall::VfsCreateSymlink,
            symlink_path_c.as_ptr() as usize,
            target_path_c.as_ptr() as usize,
            0,
            0,
        )
    };

    if result == usize::MAX {
        Err(Error::new(
            ErrorKind::Other,
            "Failed to create symbolic link",
        ))
    } else {
        Ok(())
    }
}

/// Read the target of a symbolic link
///
/// # Arguments
/// * `symlink_path` - The path to the symbolic link
///
/// # Returns
/// * `Ok(String)` - The target path that the symbolic link points to
/// * `Err(Error)` - If the symbolic link could not be read
///
/// # Example
/// ```no_run
/// #![no_std]
/// #![no_main]
/// # #[unsafe(no_mangle)]
/// # extern "C" fn main() -> i32 {
/// #     example().expect("filesystem operation failed");
/// #     0
/// # }
/// # fn example() -> scarlet_std::io::Result<()> {
/// use scarlet_std::println;
/// use scarlet_std::fs::read_link;
///
/// let target = read_link("/path/to/symlink")?;
/// println!("Symbolic link points to: {}", target);
/// # Ok(())
/// # }
/// ```
pub fn read_link(symlink_path: &str) -> Result<String> {
    use crate::ffi::str_to_cstr_bytes;
    use crate::syscall::{Syscall, syscall3};

    let symlink_path_c = str_to_cstr_bytes(symlink_path)
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "symlink_path contains null byte"))?;

    // Allocate buffer for target path (PATH_MAX = 4096)
    let mut buffer = [0u8; 4096];

    // SAFETY: The NUL-terminated path and disjoint exclusive output buffer remain valid for the supplied output length.
    let result = unsafe {
        syscall3(
            Syscall::VfsReadlink,
            symlink_path_c.as_ptr() as usize,
            buffer.as_mut_ptr() as usize,
            buffer.len(),
        )
    };

    if result == usize::MAX {
        Err(Error::new(ErrorKind::Other, "Failed to read symbolic link"))
    } else if result == 0 {
        Err(Error::new(ErrorKind::Other, "Empty symbolic link target"))
    } else {
        // Convert bytes to string (assuming UTF-8)
        let target_bytes = &buffer[..result];
        match core::str::from_utf8(target_bytes) {
            Ok(target_str) => Ok(String::from(target_str)),
            Err(_) => Err(Error::new(
                ErrorKind::Other,
                "Invalid UTF-8 in symbolic link target",
            )),
        }
    }
}

/// Rename or move a file or directory
///
/// This function renames a file or directory, moving it to a new path if the
/// paths reside in the same filesystem.
///
/// # Arguments
/// * `old_path` - Current path of the file or directory
/// * `new_path` - New path after the rename/move
///
/// # Examples
///
/// ```no_run
/// #![no_std]
/// #![no_main]
/// # #[unsafe(no_mangle)]
/// # extern "C" fn main() -> i32 {
/// #     example().expect("filesystem operation failed");
/// #     0
/// # }
/// # fn example() -> scarlet_std::io::Result<()> {
/// use scarlet_std::fs::rename;
///
/// rename("old_name.txt", "new_name.txt")?;
/// rename("src/file.txt", "dst/file.txt")?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns `Err` if the rename operation fails, such as:
/// - Source path not found
/// - Destination directory not found
/// - Cross-filesystem move (not supported)
/// - Destination is a non-empty directory
/// - Permission denied
pub fn rename<P: AsRef<str>>(old_path: P, new_path: P) -> Result<()> {
    use crate::ffi::str_to_cstr_bytes;
    use crate::syscall::{Syscall, syscall2};

    let old_path_c = str_to_cstr_bytes(old_path.as_ref())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "old_path contains null byte"))?;
    let new_path_c = str_to_cstr_bytes(new_path.as_ref())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "new_path contains null byte"))?;

    // SAFETY: Both NUL-terminated paths remain readable until the synchronous rename returns.
    let result = unsafe {
        syscall2(
            Syscall::VfsRename,
            old_path_c.as_ptr() as usize,
            new_path_c.as_ptr() as usize,
        )
    };

    if result == usize::MAX {
        Err(Error::new(
            ErrorKind::Other,
            "rename failed: source not found, cross-filesystem move, or permission denied",
        ))
    } else {
        Ok(())
    }
}

/// Get current working directory as a path string
///
/// # Returns
/// * `Ok(String)` - Current working directory path
/// * `Err(Error)` - If the path could not be retrieved
pub fn get_cwd_path() -> Result<String> {
    use crate::syscall::{Syscall, syscall2};

    // Allocate buffer for path (PATH_MAX = 4096)
    let mut buffer = [0u8; 4096];

    // SAFETY: buffer is exclusive output storage for the advertised byte length until return.
    let result = unsafe {
        syscall2(
            Syscall::VfsGetCwdPath,
            buffer.as_mut_ptr() as usize,
            buffer.len(),
        )
    };

    if result == usize::MAX {
        Err(Error::new(
            ErrorKind::Other,
            "Failed to get current working directory",
        ))
    } else {
        let path_bytes = &buffer[..result];
        match core::str::from_utf8(path_bytes) {
            Ok(path_str) => Ok(String::from(path_str)),
            Err(_) => Err(Error::new(
                ErrorKind::Other,
                "Invalid UTF-8 in current working directory",
            )),
        }
    }
}
