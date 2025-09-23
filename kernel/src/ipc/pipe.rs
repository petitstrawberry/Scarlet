//! Pipe implementation for inter-process communication
//! 
//! This module provides unidirectional pipe implementations for data streaming between processes:
//! - PipeEndpoint: Basic pipe endpoint with read/write capabilities
//! - UnidirectionalPipe: Traditional unidirectional pipe (read-only or write-only)

use alloc::{collections::VecDeque, string::String, sync::Arc, format};
#[cfg(test)]
use alloc::vec::Vec;
use spin::Mutex;

use crate::object::capability::{StreamOps, StreamError, CloneOps};
use crate::object::KernelObject;
use crate::sync::waker::Waker;
use super::{StreamIpcOps, IpcError};

/// Pipe-specific operations
/// 
/// This trait extends StreamIpcOps with pipe-specific functionality.
pub trait PipeObject: StreamIpcOps + CloneOps {
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
    /// Waker for tasks waiting to read from this pipe
    read_waker: Waker,
    /// Waker for tasks waiting to write to this pipe
    write_waker: Waker,
}

impl PipeState {
    fn new(buffer_size: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(buffer_size),
            max_size: buffer_size,
            reader_count: 0,
            writer_count: 0,
            closed: false,
            read_waker: Waker::new_interruptible("pipe_read"),
            write_waker: Waker::new_interruptible("pipe_write"),
        }
    }
}

/// A generic pipe endpoint
/// 
/// This represents the basic building block for all pipe types.
/// It can be configured for read-only, write-only, or bidirectional access.
pub struct PipeEndpoint {
    /// Shared pipe state
    state: Arc<Mutex<PipeState>>,
    /// Whether this endpoint can read
    can_read: bool,
    /// Whether this endpoint can write
    can_write: bool,
    /// Unique identifier for debugging
    id: String,
}

impl PipeEndpoint {
    /// Create a new pipe endpoint with specified capabilities
    fn new(state: Arc<Mutex<PipeState>>, can_read: bool, can_write: bool, id: String) -> Self {
        // Register this endpoint in the state
        {
            let mut pipe_state = state.lock();
            if can_read {
                pipe_state.reader_count += 1;
            }
            if can_write {
                pipe_state.writer_count += 1;
            }
        }
        
        Self {
            state,
            can_read,
            can_write,
            id,
        }
    }
}

impl StreamOps for PipeEndpoint {
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
                // Writers exist but no data available - block until data becomes available
                // Block the current task using the pipe read waker
                use crate::task::mytask;
                if let Some(task) = mytask() {
                    state.read_waker.wait(task.get_id(), task.get_trapframe());
                    
                    // After waking up, retry the read operation
                    return self.read(buffer);
                } else {
                    // No current task context, return WouldBlock for non-blocking fallback
                    return Err(StreamError::WouldBlock);
                }
            }
        }
        
        let bytes_to_read = buffer.len().min(state.buffer.len());
        for i in 0..bytes_to_read {
            buffer[i] = state.buffer.pop_front().unwrap();
        }
        
        // Data was consumed, wake up any waiting writers
        if bytes_to_read > 0 {
            state.write_waker.wake_all();
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
            // No space available - block until space becomes available
            // Block the current task using the pipe write waker
            use crate::task::mytask;
            if let Some(mut task) = mytask() {
                state.write_waker.wait(task.get_id(), task.get_trapframe());
                
                // After waking up, retry the write operation
                return self.write(buffer);
            } else {
                // No current task context, return WouldBlock for non-blocking fallback
                return Err(StreamError::WouldBlock);
            }
        }
        
        let bytes_to_write = buffer.len().min(available_space);
        for &byte in &buffer[..bytes_to_write] {
            state.buffer.push_back(byte);
        }
        
        // Data was written, wake up any waiting readers
        if bytes_to_write > 0 {
            state.read_waker.wake_all();
        }
        
        Ok(bytes_to_write)
    }
}

impl StreamIpcOps for PipeEndpoint {
    fn is_connected(&self) -> bool {
        let state = self.state.lock();
        !state.closed && (state.reader_count > 0 || state.writer_count > 0)
    }
    
    fn peer_count(&self) -> usize {
        // This is a generic implementation - specific pipe types may override this
        let state = self.state.lock();
        
        match (self.can_read, self.can_write) {
            (true, false) => state.writer_count,     // Reader: count writers
            (false, true) => state.reader_count,     // Writer: count readers
            (false, false) => 0,                     // Invalid endpoint
            (true, true) => {
                // This should not happen for unidirectional pipes
                // Return total peers minus self
                (state.reader_count + state.writer_count).saturating_sub(2)
            },
        }
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

impl CloneOps for PipeEndpoint {
    fn custom_clone(&self) -> KernelObject {
        // Clone this endpoint directly (which properly increments counters)
        // and wrap the result in the SAME Arc structure to maintain proper Drop behavior
        KernelObject::from_pipe_object(Arc::new(self.clone()))
    }
}

impl PipeObject for PipeEndpoint {
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

impl Drop for PipeEndpoint {
    fn drop(&mut self) {
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
    }
}

impl Clone for PipeEndpoint {
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

/// A unidirectional pipe (read-only or write-only endpoint)
pub struct UnidirectionalPipe {
    endpoint: PipeEndpoint,
}

impl UnidirectionalPipe {
    /// Create a new pipe pair (read_end, write_end) as KernelObjects
    pub fn create_pair(buffer_size: usize) -> (KernelObject, KernelObject) {
        let state = Arc::new(Mutex::new(PipeState::new(buffer_size)));
        
        let read_end = Self {
            endpoint: PipeEndpoint::new(state.clone(), true, false, "unidirectional_read".into()),
        };
        
        let write_end = Self {
            endpoint: PipeEndpoint::new(state.clone(), false, true, "unidirectional_write".into()),
        };
        
        // Wrap in KernelObjects
        let read_obj = KernelObject::from_pipe_object(Arc::new(read_end));
        let write_obj = KernelObject::from_pipe_object(Arc::new(write_end));
        
        (read_obj, write_obj)
    }

    /// Create a new pipe pair for internal testing (returns raw pipes)
    #[cfg(test)]
    pub fn create_pair_raw(buffer_size: usize) -> (Self, Self) {
        let state = Arc::new(Mutex::new(PipeState::new(buffer_size)));
        
        let read_end = Self {
            endpoint: PipeEndpoint::new(state.clone(), true, false, "unidirectional_read".into()),
        };
        
        let write_end = Self {
            endpoint: PipeEndpoint::new(state.clone(), false, true, "unidirectional_write".into()),
        };
        
        (read_end, write_end)
    }

}

// Delegate all traits to the underlying endpoint
impl StreamOps for UnidirectionalPipe {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        self.endpoint.read(buffer)
    }
    
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        self.endpoint.write(buffer)
    }
}

impl StreamIpcOps for UnidirectionalPipe {
    fn is_connected(&self) -> bool {
        self.endpoint.is_connected()
    }
    
    fn peer_count(&self) -> usize {
        // Unidirectional pipe specific peer_count implementation
        let state = self.endpoint.state.lock();
        
        match (self.endpoint.can_read, self.endpoint.can_write) {
            (true, false) => state.writer_count,     // Reader: count writers
            (false, true) => state.reader_count,     // Writer: count readers
            _ => 0, // Unidirectional pipes should not have both capabilities
        }
    }
    
    fn description(&self) -> String {
        self.endpoint.description()
    }
}

impl CloneOps for UnidirectionalPipe {
    fn custom_clone(&self) -> KernelObject {
        // Clone this pipe directly (which properly increments counters)
        // and wrap the result in a new Arc
        KernelObject::from_pipe_object(Arc::new(self.clone()))
    }
}

impl PipeObject for UnidirectionalPipe {
    fn has_readers(&self) -> bool {
        self.endpoint.has_readers()
    }
    
    fn has_writers(&self) -> bool {
        self.endpoint.has_writers()
    }
    
    fn buffer_size(&self) -> usize {
        self.endpoint.buffer_size()
    }
    
    fn available_bytes(&self) -> usize {
        self.endpoint.available_bytes()
    }
    
    fn is_readable(&self) -> bool {
        self.endpoint.is_readable()
    }
    
    fn is_writable(&self) -> bool {
        self.endpoint.is_writable()
    }
}

impl Clone for UnidirectionalPipe {
    fn clone(&self) -> Self {
        Self {
            endpoint: self.endpoint.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test_case]
    fn test_pipe_creation() {
        let (read_end, write_end) = UnidirectionalPipe::create_pair_raw(1024);
        
        assert!(read_end.is_readable());
        assert!(!read_end.is_writable());
        assert!(!write_end.is_readable());
        assert!(write_end.is_writable());
        
        assert!(read_end.has_writers());
        assert!(write_end.has_readers());
    }
    
    #[test_case]
    fn test_pipe_basic_io() {
        let (read_end, write_end) = UnidirectionalPipe::create_pair_raw(1024);
        
        let data = b"Hello, Pipe!";
        let written = write_end.write(data).unwrap();
        assert_eq!(written, data.len());
        
        let mut buffer = [0u8; 1024];
        let read = read_end.read(&mut buffer).unwrap();
        assert_eq!(read, data.len());
        assert_eq!(&buffer[..read], data);
    }
    
    #[test_case]
    fn test_pipe_reference_counting() {
        let (read_end, write_end) = UnidirectionalPipe::create_pair_raw(1024);
        
        // Initially: 1 reader, 1 writer
        assert_eq!(read_end.peer_count(), 1); // 1 writer peer
        assert_eq!(write_end.peer_count(), 1); // 1 reader peer
        assert!(read_end.has_writers());
        assert!(write_end.has_readers());
        
        // Debug: Check internal state
        {
            let state = read_end.endpoint.state.lock();
            assert_eq!(state.reader_count, 1);
            assert_eq!(state.writer_count, 1);
        }
        
        // Clone the read end (should increment reader count)
        let read_end_clone = read_end.clone();
        
        // Debug: Check internal state after clone
        {
            let state = read_end.endpoint.state.lock();
            assert_eq!(state.reader_count, 2); // Should be 2 after clone
            assert_eq!(state.writer_count, 1); // Should remain 1
        }
        
        assert_eq!(read_end.peer_count(), 1); // Reader: 1 writer peer
        assert_eq!(write_end.peer_count(), 2); // Writer: 2 reader peers (read_end + read_end_clone)
        assert_eq!(read_end_clone.peer_count(), 1); // Reader: 1 writer peer
        
        // Clone the write end (should increment writer count)
        let write_end_clone = write_end.clone();
        
        // Debug: Check internal state after write clone
        {
            let state = read_end.endpoint.state.lock();
            assert_eq!(state.reader_count, 2); // Still 2 readers
            assert_eq!(state.writer_count, 2); // Now 2 writers
        }
        
        assert_eq!(read_end.peer_count(), 2); // Reader: 2 writer peers (write_end + write_end_clone)
        assert_eq!(write_end.peer_count(), 2); // Writer: 2 reader peers (read_end + read_end_clone)
        assert_eq!(write_end_clone.peer_count(), 2); // Writer: 2 reader peers (read_end + read_end_clone)
        
        // Drop one reader (should decrement reader count)
        drop(read_end_clone);
        assert_eq!(read_end.peer_count(), 2); // Reader: 2 writer peers (write_end + write_end_clone)
        assert_eq!(write_end.peer_count(), 1); // Writer: 1 reader peer (read_end only)
        
        // Drop one writer (should decrement writer count)
        drop(write_end_clone);
        assert_eq!(read_end.peer_count(), 1); // Reader: 1 writer peer (write_end only)
        assert_eq!(write_end.peer_count(), 1); // Writer: 1 reader peer (read_end only)
    }
    
    #[test_case]
    fn test_pipe_broken_pipe_detection() {
        let (read_end, write_end) = UnidirectionalPipe::create_pair_raw(1024);
        
        // Initially both ends are connected
        assert!(read_end.is_connected());
        assert!(write_end.is_connected());
        assert!(read_end.has_writers());
        assert!(write_end.has_readers());
        
        // Drop the write end (should break the pipe for readers)
        drop(write_end);
        
        // Read end should detect that writers are gone
        assert!(!read_end.has_writers());
        
        // Reading should return EOF (0 bytes) when no writers remain
        let mut buffer = [0u8; 10];
        let bytes_read = read_end.read(&mut buffer).unwrap();
        assert_eq!(bytes_read, 0); // EOF
    }
    
    #[test_case]
    fn test_pipe_write_to_closed_pipe() {
        let (read_end, write_end) = UnidirectionalPipe::create_pair_raw(1024);
        
        // Drop the read end (no more readers)
        drop(read_end);
        
        // Write end should detect that readers are gone
        assert!(!write_end.has_readers());
        
        // Writing should fail with BrokenPipe error
        let data = b"Should fail";
        let result = write_end.write(data);
        assert!(result.is_err());
        if let Err(StreamError::BrokenPipe) = result {
            // Expected error
        } else {
            panic!("Expected BrokenPipe error, got: {:?}", result);
        }
    }
    
    #[test_case]
    fn test_pipe_clone_independent_operations() {
        let (read_end, write_end) = UnidirectionalPipe::create_pair_raw(1024);
        
        // Clone both ends
        let read_clone = read_end.clone();
        let write_clone = write_end.clone();
        
        // Write from original write end
        let data1 = b"From original";
        write_end.write(data1).unwrap();
        
        // Write from cloned write end
        let data2 = b" and clone";
        write_clone.write(data2).unwrap();
        
        // Read all data from original read end
        let mut buffer1 = [0u8; 50];
        let bytes1 = read_end.read(&mut buffer1).unwrap();
        let total_expected = data1.len() + data2.len();
        assert_eq!(bytes1, total_expected);
        
        // The data should be concatenated in the order of writes
        let mut expected_data = Vec::new();
        expected_data.extend_from_slice(data1);
        expected_data.extend_from_slice(data2);
        assert_eq!(&buffer1[..bytes1], &expected_data);
        
        // Buffer should now be empty - trying to read should block or return EOF
        let mut buffer2 = [0u8; 10];
        let bytes2 = read_clone.read(&mut buffer2);
        assert!(bytes2.is_err() || bytes2.unwrap() == 0);
    }
    
    #[test_case]
    fn test_pipe_buffer_management() {
        let (read_end, write_end) = UnidirectionalPipe::create_pair_raw(10); // Small buffer
        
        // Test buffer size reporting
        assert_eq!(read_end.buffer_size(), 10);
        assert_eq!(write_end.buffer_size(), 10);
        assert_eq!(read_end.available_bytes(), 0);
        
        // Fill buffer partially
        let data = b"12345";
        write_end.write(data).unwrap();
        assert_eq!(read_end.available_bytes(), 5);
        assert_eq!(write_end.available_bytes(), 5);
        
        // Fill buffer completely
        let more_data = b"67890";
        write_end.write(more_data).unwrap();
        assert_eq!(read_end.available_bytes(), 10);
        
        // Buffer should be full, next write should fail or partial
        let overflow_data = b"X";
        let result = write_end.write(overflow_data);
        assert!(result.is_err() || result.unwrap() == 0);
        
        // Read some data to make space
        let mut buffer = [0u8; 3];
        let bytes_read = read_end.read(&mut buffer).unwrap();
        assert_eq!(bytes_read, 3);
        assert_eq!(&buffer, b"123");
        assert_eq!(read_end.available_bytes(), 7);
        
        // Now writing should work again
        let new_data = b"XYZ";
        let bytes_written = write_end.write(new_data).unwrap();
        assert_eq!(bytes_written, 3);
        assert_eq!(read_end.available_bytes(), 10);
    }
    
    // === DUP SEMANTICS TESTS ===
    // These tests verify correct dup() behavior for pipes at the KernelObject level
    
    #[test_case]
    fn test_kernel_object_pipe_dup_semantics() {
        // Create pipe through KernelObject interface
        let (read_obj, write_obj) = UnidirectionalPipe::create_pair(1024);
        
        // Verify initial state through KernelObject interface
        if let Some(read_pipe) = read_obj.as_pipe() {
            if let Some(write_pipe) = write_obj.as_pipe() {
                // Initially: 1 reader, 1 writer
                assert_eq!(read_pipe.peer_count(), 1); // 1 writer
                assert_eq!(write_pipe.peer_count(), 1); // 1 reader
                assert!(read_pipe.has_writers());
                assert!(write_pipe.has_readers());
            } else {
                panic!("write_obj should be a pipe");
            }
        } else {
            panic!("read_obj should be a pipe");
        }
        
        // Clone the read end using KernelObject::clone (simulates dup syscall)
        let read_obj_cloned = read_obj.clone();
        
        // Verify that the clone operation correctly updated peer counts
        if let Some(read_pipe) = read_obj.as_pipe() {
            if let Some(write_pipe) = write_obj.as_pipe() {
                if let Some(read_pipe_cloned) = read_obj_cloned.as_pipe() {
                    // After dup: 2 readers, 1 writer
                    assert_eq!(write_pipe.peer_count(), 2); // 2 readers now!
                    assert_eq!(read_pipe.peer_count(), 1); // 1 writer
                    assert_eq!(read_pipe_cloned.peer_count(), 1); // 1 writer
                    
                    // All endpoints should still be connected
                    assert!(read_pipe.has_writers());
                    assert!(write_pipe.has_readers());
                    assert!(read_pipe_cloned.has_writers());
                } else {
                    panic!("read_obj_cloned should be a pipe");
                }
            }
        }
    }
    
    #[test_case]
    fn test_kernel_object_pipe_write_dup_semantics() {
        // Create pipe through KernelObject interface
        let (read_obj, write_obj) = UnidirectionalPipe::create_pair(1024);
        
        // Clone the write end using KernelObject::clone (simulates dup syscall)
        let write_obj_cloned = write_obj.clone();
        
        // Verify that the clone operation correctly updated peer counts
        if let Some(read_pipe) = read_obj.as_pipe() {
            if let Some(write_pipe) = write_obj.as_pipe() {
                if let Some(write_pipe_cloned) = write_obj_cloned.as_pipe() {
                    // After dup: 1 reader, 2 writers
                    assert_eq!(read_pipe.peer_count(), 2); // 2 writers now!
                    assert_eq!(write_pipe.peer_count(), 1); // 1 reader
                    assert_eq!(write_pipe_cloned.peer_count(), 1); // 1 reader
                    
                    // All endpoints should still be connected
                    assert!(read_pipe.has_writers());
                    assert!(write_pipe.has_readers());
                    assert!(write_pipe_cloned.has_readers());
                } else {
                    panic!("write_obj_cloned should be a pipe");
                }
            }
        }
    }
    
    #[test_case]
    fn test_kernel_object_pipe_dup_io_operations() {
        // Create pipe through KernelObject interface
        let (read_obj, write_obj) = UnidirectionalPipe::create_pair(1024);
        
        // Clone both ends
        let read_obj_cloned = read_obj.clone();
        let write_obj_cloned = write_obj.clone();
        
        // Write from original write end
        if let Some(write_stream) = write_obj.as_stream() {
            let data1 = b"Hello from original writer";
            let written = write_stream.write(data1).unwrap();
            assert_eq!(written, data1.len());
        }
        
        // Write from cloned write end
        if let Some(write_stream_cloned) = write_obj_cloned.as_stream() {
            let data2 = b" and cloned writer";
            let written = write_stream_cloned.write(data2).unwrap();
            assert_eq!(written, data2.len());
        }
        
        // Read from original read end
        if let Some(read_stream) = read_obj.as_stream() {
            let mut buffer = [0u8; 100];
            let bytes_read = read_stream.read(&mut buffer).unwrap();
            let total_expected = b"Hello from original writer and cloned writer".len();
            assert_eq!(bytes_read, total_expected);
            assert_eq!(&buffer[..bytes_read], b"Hello from original writer and cloned writer");
        }
        
        // Buffer should now be empty
        if let Some(read_stream_cloned) = read_obj_cloned.as_stream() {
            let mut buffer = [0u8; 10];
            let result = read_stream_cloned.read(&mut buffer);
            // Should either return 0 (EOF) or WouldBlock since buffer is empty
            assert!(result.is_err() || result.unwrap() == 0);
        }
    }
    
    #[test_case]
    fn test_kernel_object_pipe_dup_broken_pipe_detection() {
        // Create pipe through KernelObject interface
        let (read_obj, write_obj) = UnidirectionalPipe::create_pair(1024);
        
        // Clone the read end
        let read_obj_cloned = read_obj.clone();
        
        // Initially, write end should see 2 readers
        if let Some(write_pipe) = write_obj.as_pipe() {
            assert_eq!(write_pipe.peer_count(), 2);
        }
        
        // Drop one read end
        drop(read_obj);
        
        // Write end should still see 1 reader
        if let Some(write_pipe) = write_obj.as_pipe() {
            assert_eq!(write_pipe.peer_count(), 1);
            assert!(write_pipe.has_readers());
        }
        
        // Writing should still work
        if let Some(write_stream) = write_obj.as_stream() {
            let data = b"Still works";
            let written = write_stream.write(data).unwrap();
            assert_eq!(written, data.len());
        }
        
        // Drop the last read end
        drop(read_obj_cloned);
        
        // Now write end should see no readers
        if let Some(write_pipe) = write_obj.as_pipe() {
            assert_eq!(write_pipe.peer_count(), 0);
            assert!(!write_pipe.has_readers());
        }
        
        // Writing should now fail with BrokenPipe
        if let Some(write_stream) = write_obj.as_stream() {
            let data = b"Should fail";
            let result = write_stream.write(data);
            assert!(result.is_err());
            if let Err(StreamError::BrokenPipe) = result {
                // Expected error
            } else {
                panic!("Expected BrokenPipe error, got: {:?}", result);
            }
        }
    }
    
    #[test_case]
    fn test_kernel_object_pipe_dup_vs_arc_clone_comparison() {
        // This test demonstrates the difference between KernelObject::clone (correct dup)
        // and Arc::clone (incorrect for pipes)
        
        let (read_obj, write_obj) = UnidirectionalPipe::create_pair(1024);
        
        // === Correct way: KernelObject::clone (uses CloneOps) ===
        let _read_obj_dup = read_obj.clone();
        
        // This should correctly increment reader count
        if let Some(write_pipe) = write_obj.as_pipe() {
            assert_eq!(write_pipe.peer_count(), 2); // 2 readers after dup
        }
        
        // === Demonstrate what would happen with Arc::clone (incorrect) ===
        // We can't directly test Arc::clone without exposing the internal Arc,
        // but we can verify that our CloneOps implementation is being used
        
        if let Some(cloneable) = read_obj.as_cloneable() {
            // This should be Some for pipes (they implement CloneOps)
            let _custom_cloned = cloneable.custom_clone();
            
            // Verify the custom clone also works correctly
            if let Some(write_pipe) = write_obj.as_pipe() {
                assert_eq!(write_pipe.peer_count(), 3); // 3 readers now
            }
        } else {
            panic!("Pipe should implement CloneOps capability");
        }
    }
}
