use alloc::vec::Vec;
use crate::device::block::request::BlockIORequestType;

use super::*;

const SECTOR_SIZE: usize = 512;
const NUM_OF_SECTORS: usize = 1024;
pub struct TestDisk {
    buffer: Vec<u8>,
}

static mut TEST_DISK: Option<TestDisk> = None;


impl TestDisk {
    pub fn new() -> Self {
        Self { buffer: vec![0; SECTOR_SIZE * NUM_OF_SECTORS] }
    }

    pub fn write(&mut self, sector: usize, data: &[u8]) -> Result<(), &'static str> {
        let start = sector * SECTOR_SIZE;
        let end = start + data.len();

        if end > self.buffer.len() {
            return Err("Out of bounds");
        }

        self.buffer[start..end].copy_from_slice(data);
        Ok(())
    }

    pub fn read(&self, sector: usize, size: usize) -> Result<Vec<u8>, &'static str> {
        let start = sector * SECTOR_SIZE;
        let end = start + size;

        if end > self.buffer.len() {
            return Err("Out of bounds");
        }

        println!("Read {} bytes from sector {}", size, sector);
        Ok(self.buffer[start..end].to_vec())
    }

    #[allow(static_mut_refs)]
    pub fn get_device() -> GenericBlockDevice {

        unsafe {
            if TEST_DISK.is_none() {
                TEST_DISK = Some(TestDisk::new());
            }
        }

        GenericBlockDevice::new(1, "test_disk", 1024, |request| {
            let sector = request.sector;
            let count = request.sector_count;
            let disk = unsafe { TEST_DISK.as_mut().unwrap() };

            match request.request_type {
                BlockIORequestType::Read => {
                    match disk.read(sector, count * SECTOR_SIZE) {
                        Ok(data) => {
                            request.buffer = data;
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
                BlockIORequestType::Write => {
                    disk.write(sector, &request.buffer[..count * SECTOR_SIZE])
                }
            }
        })
    }
}

#[test_case]
fn test_write() {
    let mut disk = TestDisk::new();
    let data = vec![1, 2, 3, 4, 5];
    assert_eq!(disk.write(0, &data), Ok(()));
}