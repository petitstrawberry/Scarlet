//! Positional host I/O for the full distribution's root block image.
//!
//! These methods run on the host I/O side of the device transport. They also
//! serve synchronous boot probes before the Scarlet scheduler is running.

use std::{
    fs::{File, OpenOptions},
    io,
    os::unix::fs::FileExt,
    path::Path,
};

pub const SECTOR_SIZE: usize = 512;

pub struct RootImage {
    file: File,
    size: usize,
}

impl RootImage {
    /// Open an existing writable image without creating or truncating it.
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::open_with_write_access(path, true)
    }

    pub fn open_read_only(path: &Path) -> io::Result<Self> {
        Self::open_with_write_access(path, false)
    }

    fn open_with_write_access(path: &Path, writable: bool) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(writable).open(path)?;
        let metadata = file.metadata()?;
        let size = usize::try_from(metadata.len())
            .ok()
            .filter(|&size| size != 0 && size.is_multiple_of(SECTOR_SIZE))
            .filter(|_| metadata.is_file())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Root image must be a nonempty regular file with complete sectors",
                )
            })?;
        Ok(Self { file, size })
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Fill the entire sector range or return a host I/O error.
    pub fn read_sectors(&self, sector: usize, buffer: &mut [u8]) -> io::Result<()> {
        let offset = self.transfer_offset(sector, buffer.len())?;
        self.file.read_exact_at(buffer, offset)
    }

    /// Commit the entire sector range and sync the file before acknowledgement.
    pub fn write_sectors(&self, sector: usize, buffer: &[u8]) -> io::Result<()> {
        self.write_sectors_unsynced(sector, buffer)?;
        self.sync()
    }

    pub(crate) fn write_sectors_unsynced(&self, sector: usize, buffer: &[u8]) -> io::Result<()> {
        let offset = self.transfer_offset(sector, buffer.len())?;
        self.file.write_all_at(buffer, offset)
    }

    pub(crate) fn sync(&self) -> io::Result<()> {
        self.file.sync_all()
    }

    fn transfer_offset(&self, sector: usize, length: usize) -> io::Result<u64> {
        if length != 0
            && length.is_multiple_of(SECTOR_SIZE)
            && let Some(offset) = sector.checked_mul(SECTOR_SIZE)
            && offset
                .checked_add(length)
                .is_some_and(|end| end <= self.size)
        {
            return Ok(offset as u64);
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Sector transfer must fit within the root image",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_IMAGE: AtomicU64 = AtomicU64::new(0);

    struct TestImage(PathBuf);

    impl TestImage {
        fn new() -> Self {
            loop {
                let id = NEXT_IMAGE.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir()
                    .join(format!("scarlet-root-image-{}-{id}", std::process::id()));
                match File::create_new(&path) {
                    Ok(file) => {
                        let image = Self(path);
                        file.set_len((3 * SECTOR_SIZE) as u64).unwrap();
                        return image;
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("cannot create root image fixture: {error}"),
                }
            }
        }
    }

    impl Drop for TestImage {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn sector_writes_persist_and_out_of_bounds_io_fails() {
        let fixture = TestImage::new();
        let bytes = [0x73; SECTOR_SIZE];
        {
            let image = RootImage::open(&fixture.0).unwrap();
            assert_eq!(image.size(), 3 * SECTOR_SIZE);
            image.write_sectors(1, &bytes).unwrap();
            assert!(image.write_sectors(3, &bytes).is_err());
            assert!(image.write_sectors(0, &bytes[..4]).is_err());
            assert!(image.write_sectors(usize::MAX, &bytes).is_err());
        }
        let image = RootImage::open(&fixture.0).unwrap();
        let mut readback = [0; 3 * SECTOR_SIZE];
        image.read_sectors(0, &mut readback).unwrap();
        assert_eq!(&readback[..SECTOR_SIZE], &[0; SECTOR_SIZE]);
        assert_eq!(&readback[SECTOR_SIZE..2 * SECTOR_SIZE], &bytes);
        assert_eq!(&readback[2 * SECTOR_SIZE..], &[0; SECTOR_SIZE]);
        // Simulate the backing becoming shorter while a read is pending.
        OpenOptions::new()
            .write(true)
            .open(&fixture.0)
            .unwrap()
            .set_len((2 * SECTOR_SIZE) as u64)
            .unwrap();
        assert_eq!(
            image.read_sectors(0, &mut readback).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn root_block_returns_only_the_submitting_callers_batch_in_order() {
        use crate::block::RootBlock;
        use scarlet::device::block::{
            BlockDevice,
            request::{BlockIORequest, BlockIORequestType},
        };

        let _platform = scarlet::hosted::prepare(2, 16 * 1024 * 1024).unwrap();
        let fixture = TestImage::new();
        let disk = RootBlock::new(RootImage::open(&fixture.0).unwrap()).unwrap();
        let request = |request_type, sector, byte| {
            Box::new(BlockIORequest {
                request_type,
                sector,
                sector_count: 1,
                head: 0,
                cylinder: 0,
                buffer: vec![byte; SECTOR_SIZE],
            })
        };
        disk.enqueue_request(request(BlockIORequestType::Read, 2, 0));
        let results = disk.submit_requests(vec![
            request(BlockIORequestType::Write, 1, 0xac),
            request(BlockIORequestType::Read, 1, 0),
            request(BlockIORequestType::Write, 3, 0xff),
        ]);
        assert_eq!(results.len(), 3);
        assert!(results[0].result.is_ok());
        assert!(results[1].result.is_ok());
        assert!(results[1].request.buffer.iter().all(|byte| *byte == 0xac));
        assert!(results[2].result.is_err());
        let legacy = disk.process_requests();
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0].request.sector, 2);
        assert!(legacy[0].result.is_ok());
        drop(disk);
        let image = RootImage::open_read_only(&fixture.0).unwrap();
        let mut persisted = [0; SECTOR_SIZE];
        image.read_sectors(1, &mut persisted).unwrap();
        assert_eq!(persisted, [0xac; SECTOR_SIZE]);
    }
}
