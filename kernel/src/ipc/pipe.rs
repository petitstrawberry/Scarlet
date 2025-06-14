//! Pipe implementation for inter-process communication
//! 
//! This module provides unidirectional and bidirectional pipe implementations
//! for data streaming between processes.

use alloc::{collections::VecDeque, string::String, sync::Arc, format};
use spin::Mutex;

use crate::object::capability::{StreamOps, StreamError};
use super::{IpcObject, IpcError};

/// Pipe-specific operations
/// 
/// This trait extends IpcObject with pipe-specific functionality.
pub trait PipeObject: IpcObject {
    /// Check if there are readers on the other end
    fn has_readers(&self) -> bool;
    
    /// Check if there are writers on the other end
    fn has_writers(&self) -> bool;
    
    /// Get the buffer size of the pipe
    fn buffer_size(&self) -> usize;
    
    /// Get the number of bytes currently in the pipe buffer
    fn available_bytes(&self) -> usize;
    
    /// Check if this end of the pipe is readable
    fn is_readable(&self) -> bool;
    
    /// Check if this end of the pipe is writable
    fn is_writable(&self) -> bool;
}

/// Represents errors specific to pipe operations
#[derive(Debug, Clone)]
pub enum PipeError {
    /// The pipe is broken (no readers or writers)
    BrokenPipe,
    /// The pipe buffer is full
    BufferFull,
    /// The pipe buffer is empty
    BufferEmpty,
    /// Invalid pipe state
    InvalidState,
    /// General IPC error
    IpcError(IpcError),
}

impl From<IpcError> for PipeError {
    fn from(ipc_err: IpcError) -> Self {
        PipeError::IpcError(ipc_err)
    }
}

impl From<StreamError> for PipeError {
    fn from(stream_err: StreamError) -> Self {
        PipeError::IpcError(IpcError::StreamError(stream_err))
    }
}

/// Internal shared state of a pipe
struct PipeState {
    /// Ring buffer for pipe data
    buffer: VecDeque<u8>,
    /// Maximum buffer size
    max_size: usize,
    /// Number of active readers
    reader_count: usize,
    /// Number of active writers
    writer_count: usize,
    /// Whether the pipe has been closed
    closed: bool,
}

impl PipeState {
    fn new(buffer_size: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(buffer_size),
            max_size: buffer_size,
            reader_count: 0,
            writer_count: 0,
            closed: false,
        }
    }
}

/// A unidirectional pipe endpoint
/// 
/// This represents one end of a pipe (either read or write).
/// Multiple UnidirectionalPipe instances can share the same underlying buffer.
pub struct UnidirectionalPipe {
    /// Shared pipe state
    state: Arc<Mutex<PipeState>>,
    /// Whether this endpoint can read
    can_read: bool,
    /// Whether this endpoint can write
    can_write: bool,
    /// Unique identifier for debugging
    id: String,
}

impl UnidirectionalPipe {
    /// Create a new pipe pair (read_end, write_end)
    pub fn create_pair(buffer_size: usize) -> (Self, Self) {
        let state = Arc::new(Mutex::new(PipeState::new(buffer_size)));
        
        let read_end = Self {
            state: state.clone(),
            can_read: true,
            can_write: false,
            id: "pipe_read".into(),
        };
        
        let write_end = Self {
            state: state.clone(),
            can_read: false,
            can_write: true,
            id: "pipe_write".into(),
        };
        
        // Register the endpoints
        {
            let mut pipe_state = state.lock();
            pipe_state.reader_count = 1;
            pipe_state.writer_count = 1;
        }
        
        (read_end, write_end)
    }
    
    /// Create a bidirectional pipe endpoint
    pub fn create_bidirectional(buffer_size: usize) -> Self {
        let state = Arc::new(Mutex::new(PipeState::new(buffer_size)));
        
        let pipe = Self {
            state: state.clone(),
            can_read: true,
            can_write: true,
            id: "pipe_bidirectional".into(),
        };
        
        // Register as both reader and writer
        {
            let mut pipe_state = state.lock();
            pipe_state.reader_count = 1;
            pipe_state.writer_count = 1;
        }
        
        pipe
    }
}

impl StreamOps for UnidirectionalPipe {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if !self.can_read {
            return Err(StreamError::NotSupported);
        }
        
        let mut state = self.state.lock();
        
        if state.closed {
            return Err(StreamError::Closed);
        }
        
        if state.buffer.is_empty() {
            if state.writer_count == 0 {
                // No writers left, return EOF
                return Ok(0);
            } else {
                // Writers exist but no data available
                return Err(StreamError::WouldBlock);
            }
        }
        
        let bytes_to_read = buffer.len().min(state.buffer.len());
        for i in 0..bytes_to_read {
            buffer[i] = state.buffer.pop_front().unwrap();
        }
        
        Ok(bytes_to_read)
    }
    
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        if !self.can_write {
            return Err(StreamError::NotSupported);
        }
        
        let mut state = self.state.lock();
        
        if state.closed {
            return Err(StreamError::Closed);
        }
        
        if state.reader_count == 0 {
            return Err(StreamError::BrokenPipe);
        }
        
        let available_space = state.max_size - state.buffer.len();
        if available_space == 0 {
            return Err(StreamError::WouldBlock);
        }
        
        let bytes_to_write = buffer.len().min(available_space);
        for &byte in &buffer[..bytes_to_write] {
            state.buffer.push_back(byte);
        }
        
        Ok(bytes_to_write)
    }
    
    fn release(&self) -> Result<(), StreamError> {
        let mut state = self.state.lock();
        
        if self.can_read {
            state.reader_count = state.reader_count.saturating_sub(1);
        }
        if self.can_write {
            state.writer_count = state.writer_count.saturating_sub(1);
        }
        
        if state.reader_count == 0 && state.writer_count == 0 {
            state.closed = true;
            state.buffer.clear();
        }
        
        Ok(())
    }
}

impl IpcObject for UnidirectionalPipe {
    fn is_connected(&self) -> bool {
        let state = self.state.lock();
        !state.closed && (state.reader_count > 0 || state.writer_count > 0)
    }
    
    fn peer_count(&self) -> usize {
        let state = self.state.lock();
        let mut count = state.reader_count + state.writer_count;
        
        // Don't count ourselves
        if self.can_read {
            count = count.saturating_sub(1);
        }
        if self.can_write {
            count = count.saturating_sub(1);
        }
        
        count
    }
    
    fn description(&self) -> String {
        let access = match (self.can_read, self.can_write) {
            (true, true) => "read/write",
            (true, false) => "read-only",
            (false, true) => "write-only",
            (false, false) => "no-access",
        };
        
        format!("{}({})", self.id, access)
    }
}

impl PipeObject for UnidirectionalPipe {
    fn has_readers(&self) -> bool {
        let state = self.state.lock();
        state.reader_count > 0
    }
    
    fn has_writers(&self) -> bool {
        let state = self.state.lock();
        state.writer_count > 0
    }
    
    fn buffer_size(&self) -> usize {
        let state = self.state.lock();
        state.max_size
    }
    
    fn available_bytes(&self) -> usize {
        let state = self.state.lock();
        state.buffer.len()
    }
    
    fn is_readable(&self) -> bool {
        self.can_read
    }
    
    fn is_writable(&self) -> bool {
        self.can_write
    }
}

impl Drop for UnidirectionalPipe {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

impl Clone for UnidirectionalPipe {
    fn clone(&self) -> Self {
        let new_pipe = Self {
            state: self.state.clone(),
            can_read: self.can_read,
            can_write: self.can_write,
            id: format!("{}_clone", self.id),
        };
        
        // Increment reference counts
        {
            let mut state = self.state.lock();
            if self.can_read {
                state.reader_count += 1;
            }
            if self.can_write {
                state.writer_count += 1;
            }
        }
        
        new_pipe
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test_case]
    fn test_pipe_creation() {
        let (read_end, write_end) = UnidirectionalPipe::create_pair(1024);
        
        assert!(read_end.is_readable());
        assert!(!read_end.is_writable());
        assert!(!write_end.is_readable());
        assert!(write_end.is_writable());
        
        assert!(read_end.has_writers());
        assert!(write_end.has_readers());
    }
    
    #[test_case]
    fn test_pipe_basic_io() {
        let (read_end, write_end) = UnidirectionalPipe::create_pair(1024);
        
        let data = b"Hello, Pipe!";
        let written = write_end.write(data).unwrap();
        assert_eq!(written, data.len());
        
        let mut buffer = [0u8; 1024];
        let read = read_end.read(&mut buffer).unwrap();
        assert_eq!(read, data.len());
        assert_eq!(&buffer[..read], data);
    }
    
    #[test_case]
    fn test_bidirectional_pipe() {
        let pipe = UnidirectionalPipe::create_bidirectional(1024);
        
        assert!(pipe.is_readable());
        assert!(pipe.is_writable());
        
        let data = b"Bidirectional test";
        let written = pipe.write(data).unwrap();
        assert_eq!(written, data.len());
        
        let mut buffer = [0u8; 1024];
        let read = pipe.read(&mut buffer).unwrap();
        assert_eq!(read, data.len());
        assert_eq!(&buffer[..read], data);
    }
    
    #[test_case]
    fn test_pipe_reference_counting() {
        let (read_end, write_end) = UnidirectionalPipe::create_pair(1024);
        
        // Initially: 1 reader, 1 writer
        assert_eq!(read_end.peer_count(), 1); // 1 writer peer
        assert_eq!(write_end.peer_count(), 1); // 1 reader peer
        assert!(read_end.has_writers());
        assert!(write_end.has_readers());
        
        // Clone the read end (should increment reader count)
        let read_end_clone = read_end.clone();
        assert_eq!(read_end.peer_count(), 1); // Still 1 writer peer
        assert_eq!(write_end.peer_count(), 2); // Now 2 reader peers
        assert_eq!(read_end_clone.peer_count(), 1); // 1 writer peer from clone's perspective
        
        // Clone the write end (should increment writer count)
        let write_end_clone = write_end.clone();
        assert_eq!(read_end.peer_count(), 2); // Now 2 writer peers
        assert_eq!(write_end.peer_count(), 2); // Still 2 reader peers
        assert_eq!(write_end_clone.peer_count(), 2); // 2 reader peers from clone's perspective
        
        // Drop one reader (should decrement reader count)
        drop(read_end_clone);
        assert_eq!(read_end.peer_count(), 2); // Still 2 writer peers
        assert_eq!(write_end.peer_count(), 1); // Back to 1 reader peer
        
        // Drop one writer (should decrement writer count)
        drop(write_end_clone);
        assert_eq!(read_end.peer_count(), 1); // Back to 1 writer peer
        assert_eq!(write_end.peer_count(), 1); // Still 1 reader peer
    }
    
    // #[test_case]
    // fn test_pipe_broken_pipe_detection() {
    //     let (read_end, write_end) = UnidirectionalPipe::create_pair(1024);
        
    //     // Initially both ends are connected
    //     assert!(read_end.is_connected());
    //     assert!(write_end.is_connected());
    //     assert!(read_end.has_writers());
    //     assert!(write_end.has_readers());
        
    //     // Drop the write end (should break the pipe for readers)
    //     drop(write_end);
        
    //     // Read end should detect that writers are gone
    //     assert!(!read_end.has_writers());
        
    //     // Reading should return EOF (0 bytes) when no writers remain
    //     let mut buffer = [0u8; 10];
    //     let bytes_read = read_end.read(&mut buffer).unwrap();
    //     assert_eq!(bytes_read, 0); // EOF
    // }
    
    // #[test_case]
    // fn test_pipe_write_to_closed_pipe() {
    //     let (read_end, write_end) = UnidirectionalPipe::create_pair(1024);
        
    //     // Drop the read end (no more readers)
    //     drop(read_end);
        
    //     // Write end should detect that readers are gone
    //     assert!(!write_end.has_readers());
        
    //     // Writing should fail with BrokenPipe error
    //     let data = b"Should fail";
    //     let result = write_end.write(data);
    //     assert!(result.is_err());
    //     if let Err(StreamError::BrokenPipe) = result {
    //         // Expected error
    //     } else {
    //         panic!("Expected BrokenPipe error, got: {:?}", result);
    //     }
    // }
    
    // #[test_case]
    // fn test_pipe_clone_independent_operations() {
    //     let (read_end, write_end) = UnidirectionalPipe::create_pair(1024);
        
    //     // Clone both ends
    //     let read_clone = read_end.clone();
    //     let write_clone = write_end.clone();
        
    //     // Write from original write end
    //     let data1 = b"From original";
    //     write_end.write(data1).unwrap();
        
    //     // Write from cloned write end
    //     let data2 = b" and clone";
    //     write_clone.write(data2).unwrap();
        
    //     // Read from original read end
    //     let mut buffer1 = [0u8; 50];
    //     let bytes1 = read_end.read(&mut buffer1).unwrap();
    //     assert_eq!(bytes1, data1.len());
    //     assert_eq!(&buffer1[..bytes1], data1);
        
    //     // Read from cloned read end (should get data2)
    //     let mut buffer2 = [0u8; 50];
    //     let bytes2 = read_clone.read(&mut buffer2).unwrap();
    //     assert_eq!(bytes2, data2.len());
    //     assert_eq!(&buffer2[..bytes2], data2);
        
    //     // Buffer should now be empty
    //     let mut buffer3 = [0u8; 10];
    //     let bytes3 = read_end.read(&mut buffer3);
    //     assert!(bytes3.is_err() || bytes3.unwrap() == 0);
    // }
    
    // #[test_case]
    // fn test_pipe_buffer_management() {
    //     let (read_end, write_end) = UnidirectionalPipe::create_pair(10); // Small buffer
        
    //     // Test buffer size reporting
    //     assert_eq!(read_end.buffer_size(), 10);
    //     assert_eq!(write_end.buffer_size(), 10);
    //     assert_eq!(read_end.available_bytes(), 0);
        
    //     // Fill buffer partially
    //     let data = b"12345";
    //     write_end.write(data).unwrap();
    //     assert_eq!(read_end.available_bytes(), 5);
    //     assert_eq!(write_end.available_bytes(), 5);
        
    //     // Fill buffer completely
    //     let more_data = b"67890";
    //     write_end.write(more_data).unwrap();
    //     assert_eq!(read_end.available_bytes(), 10);
        
    //     // Buffer should be full, next write should fail or partial
    //     let overflow_data = b"X";
    //     let result = write_end.write(overflow_data);
    //     assert!(result.is_err() || result.unwrap() == 0);
        
    //     // Read some data to make space
    //     let mut buffer = [0u8; 3];
    //     let bytes_read = read_end.read(&mut buffer).unwrap();
    //     assert_eq!(bytes_read, 3);
    //     assert_eq!(&buffer, b"123");
    //     assert_eq!(read_end.available_bytes(), 7);
        
    //     // Now writing should work again
    //     let new_data = b"XYZ";
    //     let bytes_written = write_end.write(new_data).unwrap();
    //     assert_eq!(bytes_written, 3);
    //     assert_eq!(read_end.available_bytes(), 10);
    // }
    
    // #[test_case]
    // fn test_pipe_bidirectional_reference_counting() {
    //     let pipe = UnidirectionalPipe::create_bidirectional(1024);
        
    //     // Bidirectional pipe counts as both reader and writer
    //     assert!(pipe.has_readers());
    //     assert!(pipe.has_writers());
    //     assert_eq!(pipe.peer_count(), 0); // No peers (self doesn't count)
        
    //     // Clone should increment both counts
    //     let pipe_clone = pipe.clone();
    //     assert_eq!(pipe.peer_count(), 1); // 1 peer (the clone)
    //     assert_eq!(pipe_clone.peer_count(), 1); // 1 peer (the original)
        
    //     // Both should still be connected
    //     assert!(pipe.is_connected());
    //     assert!(pipe_clone.is_connected());
        
    //     // Drop the clone
    //     drop(pipe_clone);
    //     assert_eq!(pipe.peer_count(), 0); // Back to no peers
    //     assert!(pipe.is_connected()); // Still connected to itself
    // }
}
