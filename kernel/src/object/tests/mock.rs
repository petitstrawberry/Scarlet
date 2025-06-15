//! Mock objects for testing kernel object management system

use alloc::vec::Vec;
use spin::Mutex;
use crate::object::capability::{StreamOps, StreamError};
use crate::fs::{FileType, FileMetadata, DirectoryEntry, SeekFrom};

/// Mock FileObject for testing purposes
pub struct MockFileObject {
    pub data: Vec<u8>,
    pub position: Mutex<usize>,
}

impl MockFileObject {
    pub fn new(data: Vec<u8>) -> Self {
        Self { 
            data, 
            position: Mutex::new(0) 
        }
    }
}

impl StreamOps for MockFileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let mut pos = self.position.lock();
        let available = self.data.len().saturating_sub(*pos);
        let to_read = buffer.len().min(available);
        
        if to_read > 0 {
            buffer[..to_read].copy_from_slice(&self.data[*pos..*pos + to_read]);
            *pos += to_read;
        }
        
        Ok(to_read)
    }
    
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        // Mock implementation - just return the buffer length
        Ok(buffer.len())
    }
}

impl crate::fs::FileObject for MockFileObject {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut pos = self.position.lock();
        let new_pos = match whence {
            SeekFrom::Start(offset) => offset as usize,
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    *pos + offset as usize
                } else {
                    pos.saturating_sub((-offset) as usize)
                }
            }
            SeekFrom::End(offset) => {
                if offset >= 0 {
                    self.data.len() + offset as usize
                } else {
                    self.data.len().saturating_sub((-offset) as usize)
                }
            }
        };
        
        *pos = new_pos;
        Ok(new_pos as u64)
    }
    
    fn readdir(&self) -> Result<Vec<DirectoryEntry>, StreamError> {
        Err(StreamError::NotSupported)
    }
    
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: FileType::RegularFile,
            size: self.data.len(),
            permissions: crate::fs::FilePermission {
                read: true,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 1,
            link_count: 1,
        })
    }
}

/// Mock FileObject for testing task integration
pub struct MockTaskFileObject {
    pub data: Vec<u8>,
    pub position: Mutex<usize>,
}

impl MockTaskFileObject {
    pub fn new(data: Vec<u8>) -> Self {
        Self { 
            data, 
            position: Mutex::new(0) 
        }
    }
}

impl StreamOps for MockTaskFileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let mut pos = self.position.lock();
        let available = self.data.len().saturating_sub(*pos);
        let to_read = buffer.len().min(available);
        
        if to_read > 0 {
            buffer[..to_read].copy_from_slice(&self.data[*pos..*pos + to_read]);
            *pos += to_read;
        }
        
        Ok(to_read)
    }
    
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        Ok(buffer.len())
    }
}

impl crate::fs::FileObject for MockTaskFileObject {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut pos = self.position.lock();
        let new_pos = match whence {
            SeekFrom::Start(offset) => offset as usize,
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    *pos + offset as usize
                } else {
                    pos.saturating_sub((-offset) as usize)
                }
            }
            SeekFrom::End(offset) => {
                if offset >= 0 {
                    self.data.len() + offset as usize
                } else {
                    self.data.len().saturating_sub((-offset) as usize)
                }
            }
        };
        
        *pos = new_pos;
        Ok(new_pos as u64)
    }
    
    fn readdir(&self) -> Result<Vec<DirectoryEntry>, StreamError> {
        Err(StreamError::NotSupported)
    }
    
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: FileType::RegularFile,
            size: self.data.len(),
            permissions: crate::fs::FilePermission {
                read: true,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 1,
            link_count: 1,
        })
    }
}
