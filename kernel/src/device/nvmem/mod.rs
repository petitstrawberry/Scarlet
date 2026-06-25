//! NVMEM (non-volatile memory) subsystem abstractions.
//!
//! Provides consumer-side access to device-specific calibration data, fuse values,
//! factory-programmed identifiers, and similar persistent storage. Providers are
//! registered in [`crate::device::manager::DeviceManager`] by firmware phandle;
//! the manager resolves firmware cell specifiers into [`NvmemCell`] handles so
//! cells can retain an owned `Arc<dyn NvmemProvider>`.

extern crate alloc;

use alloc::sync::Arc;

/// NVMEM operation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvmemError {
    /// Requested provider or cell was not found.
    NotFound,
    /// Requested offset, size, or buffer length is outside the provider range.
    OutOfRange,
    /// Read operation failed.
    ReadFailed,
    /// Write operation failed.
    WriteFailed,
    /// Operation is not supported by this provider.
    NotSupported,
    /// Provider is busy and cannot satisfy the operation.
    Busy,
    /// Hardware access failed.
    HardwareError,
}

/// A read/write accessor for an NVMEM cell at an offset and size within a provider.
pub struct NvmemCell {
    provider: Arc<dyn NvmemProvider>,
    offset: usize,
    size: usize,
    name: &'static str,
}

impl NvmemCell {
    /// Create a new NVMEM cell handle.
    ///
    /// # Arguments
    ///
    /// * `provider` - Provider that backs the cell.
    /// * `offset` - Byte offset within the provider.
    /// * `size` - Cell size in bytes.
    /// * `name` - Static cell name used for diagnostics.
    ///
    /// # Returns
    ///
    /// A cell handle that reads and writes through `provider`.
    pub fn new(
        provider: Arc<dyn NvmemProvider>,
        offset: usize,
        size: usize,
        name: &'static str,
    ) -> Self {
        Self {
            provider,
            offset,
            size,
            name,
        }
    }

    /// Return the cell name.
    ///
    /// # Returns
    ///
    /// Static cell name supplied by firmware or the resolver fallback.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Return the cell size in bytes.
    ///
    /// # Returns
    ///
    /// Number of bytes exposed by this cell.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Read the entire cell into a caller-provided buffer.
    ///
    /// # Arguments
    ///
    /// * `buf` - Buffer whose length must exactly match [`Self::size`].
    ///
    /// # Returns
    ///
    /// `Ok(())` when the provider read succeeds.
    pub fn read(&self, buf: &mut [u8]) -> Result<(), NvmemError> {
        if buf.len() != self.size {
            return Err(NvmemError::OutOfRange);
        }

        self.provider.read(self.offset, buf)
    }

    /// Write the entire cell from a caller-provided buffer.
    ///
    /// # Arguments
    ///
    /// * `buf` - Buffer whose length must exactly match [`Self::size`].
    ///
    /// # Returns
    ///
    /// `Ok(())` when the provider write succeeds.
    pub fn write(&self, buf: &[u8]) -> Result<(), NvmemError> {
        if buf.len() != self.size {
            return Err(NvmemError::OutOfRange);
        }

        self.provider.write(self.offset, buf)
    }
}

/// NVMEM provider that exposes read/write access to its backing storage.
pub trait NvmemProvider: Send + Sync {
    /// Return the provider name.
    ///
    /// # Returns
    ///
    /// Static name used for logging and diagnostics.
    fn name(&self) -> &'static str;

    /// Return the provider size in bytes.
    ///
    /// # Returns
    ///
    /// Byte length of the readable provider storage.
    fn size(&self) -> usize;

    /// Read bytes from provider storage.
    ///
    /// # Arguments
    ///
    /// * `offset` - Byte offset within provider storage.
    /// * `buf` - Destination buffer to fill.
    ///
    /// # Returns
    ///
    /// `Ok(())` when all requested bytes were read.
    fn read(&self, offset: usize, buf: &mut [u8]) -> Result<(), NvmemError>;

    /// Write bytes to provider storage.
    ///
    /// # Arguments
    ///
    /// * `offset` - Byte offset within provider storage.
    /// * `buf` - Source buffer to write.
    ///
    /// # Returns
    ///
    /// `Ok(())` when all requested bytes were written.
    fn write(&self, offset: usize, buf: &[u8]) -> Result<(), NvmemError>;

    /// Return the number of cells in `#nvmem-cell-cells`.
    ///
    /// # Returns
    ///
    /// Number of specifier cells after the provider phandle. The default is 2
    /// for `(offset, size)`.
    fn cell_cells(&self) -> usize {
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use spin::Mutex;

    struct MemoryNvmemProvider {
        data: Mutex<[u8; 4]>,
    }

    impl NvmemProvider for MemoryNvmemProvider {
        fn name(&self) -> &'static str {
            "memory-nvmem"
        }

        fn size(&self) -> usize {
            4
        }

        fn read(&self, offset: usize, buf: &mut [u8]) -> Result<(), NvmemError> {
            let end = offset
                .checked_add(buf.len())
                .ok_or(NvmemError::OutOfRange)?;
            if end > self.size() {
                return Err(NvmemError::OutOfRange);
            }

            buf.copy_from_slice(&self.data.lock()[offset..end]);
            Ok(())
        }

        fn write(&self, offset: usize, buf: &[u8]) -> Result<(), NvmemError> {
            let end = offset
                .checked_add(buf.len())
                .ok_or(NvmemError::OutOfRange)?;
            if end > self.size() {
                return Err(NvmemError::OutOfRange);
            }

            self.data.lock()[offset..end].copy_from_slice(buf);
            Ok(())
        }
    }

    #[test_case]
    fn test_nvmem_cell_read_write_validates_size() {
        let provider = Arc::new(MemoryNvmemProvider {
            data: Mutex::new([1, 2, 3, 4]),
        });
        let cell = NvmemCell::new(provider, 1, 2, "cell");

        let mut buf = [0u8; 2];
        cell.read(&mut buf).unwrap();
        assert_eq!(buf, [2, 3]);

        cell.write(&[8, 9]).unwrap();
        cell.read(&mut buf).unwrap();
        assert_eq!(buf, [8, 9]);
        assert_eq!(cell.read(&mut [0u8; 1]), Err(NvmemError::OutOfRange));
        assert_eq!(cell.write(&[1]), Err(NvmemError::OutOfRange));
        assert_eq!(
            vec![cell.name(), provider_name(&cell)],
            vec!["cell", "memory-nvmem"]
        );
    }

    fn provider_name(cell: &NvmemCell) -> &'static str {
        cell.provider.name()
    }
}
