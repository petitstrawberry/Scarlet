//! Mock objects for testing kernel object management system

use alloc::vec::Vec;
use alloc::string::{String, ToString};
use spin::Mutex;
use crate::object::capability::{StreamOps, StreamError, CloneOps};
use crate::fs::{FileType, FileMetadata, DirectoryEntry, SeekFrom};

/// Mock FileObject for testing purposes
pub struct MockFileObject {
    pub name: String,
    pub data: Vec<u8>,
    pub position: Mutex<usize>,
}

impl MockFileObject {
    pub fn new(data: Vec<u8>) -> Self {
        Self { 
            name: "unnamed".to_string(),
            data, 
            position: Mutex::new(0) 
        }
    }
    
    pub fn with_name_and_content(name: &str, content: &str) -> Self {
        Self { 
            name: name.to_string(),
            data: content.as_bytes().to_vec(), 
            position: Mutex::new(0) 
        }
    }
    
    pub fn from_data(data: Vec<u8>) -> Self {
        Self { 
            name: "unnamed".to_string(),
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

/// Mock PipeObject for testing purposes
pub struct MockPipeObject {
    pub read_buffer: Mutex<Vec<u8>>,
    pub write_buffer: Mutex<Vec<u8>>,
}

impl MockPipeObject {
    pub fn new() -> Self {
        Self {
            read_buffer: Mutex::new(Vec::new()),
            write_buffer: Mutex::new(Vec::new()),
        }
    }
    
    pub fn with_data(data: &str) -> Self {
        Self {
            read_buffer: Mutex::new(data.as_bytes().to_vec()),
            write_buffer: Mutex::new(Vec::new()),
        }
    }
}

impl StreamOps for MockPipeObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let mut read_buf = self.read_buffer.lock();
        let to_read = buffer.len().min(read_buf.len());
        
        if to_read > 0 {
            buffer[..to_read].copy_from_slice(&read_buf[..to_read]);
            read_buf.drain(..to_read);
        }
        
        Ok(to_read)
    }
    
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        let mut write_buf = self.write_buffer.lock();
        write_buf.extend_from_slice(buffer);
        Ok(buffer.len())
    }
}

// Since we don't have PipeOps trait defined yet, we'll implement PipeObject directly on crate::object::capability::PipeObject
// For now, MockPipeObject only implements StreamOps

impl CloneOps for MockPipeObject {
    fn custom_clone(&self) -> crate::object::KernelObject {
        use crate::object::KernelObject;
        use alloc::sync::Arc;
        
        // Clone the current state
        let read_data = self.read_buffer.lock().clone();
        let write_data = self.write_buffer.lock().clone();
        
        let cloned = MockPipeObject {
            read_buffer: Mutex::new(read_data),
            write_buffer: Mutex::new(write_data),
        };
        
        KernelObject::Pipe(Arc::new(cloned))
    }
}

impl crate::ipc::IpcObject for MockPipeObject {
    fn is_connected(&self) -> bool {
        true // Mock implementation
    }
    
    fn peer_count(&self) -> usize {
        1 // Mock implementation
    }
    
    fn description(&self) -> alloc::string::String {
        "Mock Pipe".to_string()
    }
}

impl crate::ipc::pipe::PipeObject for MockPipeObject {
    fn has_readers(&self) -> bool {
        true // Mock implementation
    }
    
    fn has_writers(&self) -> bool {
        true // Mock implementation
    }
    
    fn buffer_size(&self) -> usize {
        1024 // Mock buffer size
    }
    
    fn available_bytes(&self) -> usize {
        self.read_buffer.lock().len()
    }
    
    fn is_readable(&self) -> bool {
        true // Mock implementation
    }
    
    fn is_writable(&self) -> bool {
        true // Mock implementation
    }
}
