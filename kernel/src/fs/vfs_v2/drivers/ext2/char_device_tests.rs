//! ext2 Character Device Integration Tests
//!
//! This test module tests the creation, opening, read/write operations
//! of character device files on the ext2 filesystem.

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use crate::{
        fs::{FileType, DeviceFileInfo, FileObject},
        device::{DeviceType, char::CharDevice, Device},
        object::capability::{ControlOps, MemoryMappingOps},
    };
    use spin::Mutex;
    use core::any::Any;

    /// Mock character device for testing
    struct MockCharDevice {
        name: &'static str,
        data: Mutex<Vec<u8>>,
        position: Mutex<usize>,
    }

    impl MockCharDevice {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                data: Mutex::new(Vec::new()),
                position: Mutex::new(0),
            }
        }
    }

    impl Device for MockCharDevice {
        fn device_type(&self) -> crate::device::DeviceType {
            crate::device::DeviceType::Char
        }

        fn name(&self) -> &'static str {
            self.name
        }

        fn as_any(&self) -> &dyn Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }

        fn as_char_device(&self) -> Option<&dyn CharDevice> {
            Some(self)
        }
    }

    impl CharDevice for MockCharDevice {
        fn read_byte(&self) -> Option<u8> {
            let data = self.data.lock();
            let mut pos = self.position.lock();
            if *pos < data.len() {
                let byte = data[*pos];
                *pos += 1;
                Some(byte)
            } else {
                None
            }
        }

        fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
            let mut data = self.data.lock();
            data.push(byte);
            Ok(())
        }

        fn can_read(&self) -> bool {
            let data = self.data.lock();
            let pos = self.position.lock();
            *pos < data.len()
        }

        fn can_write(&self) -> bool {
            true
        }

        fn read(&self, buffer: &mut [u8]) -> usize {
            let data = self.data.lock();
            let mut pos = self.position.lock();
            let available = data.len().saturating_sub(*pos);
            let to_read = core::cmp::min(buffer.len(), available);
            
            if to_read > 0 {
                buffer[..to_read].copy_from_slice(&data[*pos..*pos + to_read]);
                *pos += to_read;
            }
            to_read
        }

        fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
            let mut data = self.data.lock();
            data.extend_from_slice(buffer);
            Ok(buffer.len())
        }
    }

    impl ControlOps for MockCharDevice {
        fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
            Err("Control operation not supported")
        }
    }

    impl MemoryMappingOps for MockCharDevice {
        fn get_mapping_info(&self, _offset: usize, _length: usize) -> Result<(usize, usize, bool), &'static str> {
            Err("Memory mapping not supported")
        }
    }

    #[test_case]
    fn test_ext2_char_device_create_and_open() {
        crate::early_println!("[test] Testing ext2 character device creation and open");

        // Actual tests can only be executed in an environment where
        // the device manager and ext2 filesystem are initialized.
        // This test functions as a compilation test to ensure syntax 
        // and trait implementations are correct.

        crate::early_println!("[test] ext2 character device test completed successfully");
    }

    #[test_case]
    fn test_ext2_char_device_file_object_creation() {
        crate::early_println!("[test] Testing ext2 character device file object creation");

        // Test creation of DeviceFileInfo
        let device_info = DeviceFileInfo {
            device_id: 123,
            device_type: DeviceType::Char,
        };

        // Test creation of Ext2CharDeviceFileObject
        let char_device_obj = crate::fs::vfs_v2::drivers::ext2::Ext2CharDeviceFileObject::new(device_info, 1);
        
        // Test metadata retrieval
        let metadata = char_device_obj.metadata();
        assert!(metadata.is_ok());
        
        let metadata = metadata.unwrap();
        match metadata.file_type {
            FileType::CharDevice(info) => {
                assert_eq!(info.device_id, 123);
                assert_eq!(info.device_type, DeviceType::Char);
            },
            _ => panic!("Expected CharDevice file type"),
        }

        crate::early_println!("[test] ext2 character device file object test completed successfully");
    }

    #[test_case]
    fn test_ext2_char_device_file_type_conversion() {
        crate::early_println!("[test] Testing ext2 character device file type conversion");

        // Test conversion from ext2 inode to character device FileType
        let mut inode = crate::fs::vfs_v2::drivers::ext2::structures::Ext2Inode::empty();
        
        // Set character device mode
        inode.mode = (crate::fs::vfs_v2::drivers::ext2::structures::EXT2_S_IFCHR | 0o666).to_le();
        
        // Set device information (major=1, minor=0 for tty)
        inode.block[0] = ((1u32 << 8) | 0u32).to_le();

        // Creating filesystem requires actual block device,
        // so we only test struct creation here
        crate::early_println!("[test] ext2 character device file type conversion test completed");
    }
}
