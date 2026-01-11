//! Wayland Shared Memory (wl_shm) Support
//!
//! This module implements the wl_shm protocol for shared memory buffers.
//! Wayland clients use shared memory to efficiently transfer pixel data
//! to the compositor without copying.

use std::collections::BTreeMap;

/// Shared memory pool
#[derive(Debug)]
pub struct ShmPool {
    /// Pool object ID
    pub pool_id: u32,
    /// Handle for the shared memory (received via Socket::recv_handle)
    /// The Linux compatibility layer converts SCM_RIGHTS FDs to kernel handles
    pub fd: i32,
    /// Size of the pool in bytes
    pub size: usize,
}

/// Shared memory buffer
#[derive(Debug)]
pub struct ShmBuffer {
    /// Buffer object ID
    pub buffer_id: u32,
    /// Parent pool ID
    pub pool_id: u32,
    /// Offset into the pool
    pub offset: i32,
    /// Width in pixels
    pub width: i32,
    /// Height in pixels
    pub height: i32,
    /// Stride (bytes per row)
    pub stride: i32,
    /// Pixel format
    pub format: u32,
}

/// wl_shm manager
pub struct ShmManager {
    /// Pools indexed by object ID
    pools: BTreeMap<u32, ShmPool>,
    /// Buffers indexed by object ID
    buffers: BTreeMap<u32, ShmBuffer>,
}

impl ShmManager {
    /// Create a new SHM manager
    pub fn new() -> Self {
        Self {
            pools: BTreeMap::new(),
            buffers: BTreeMap::new(),
        }
    }

    /// Create a new shared memory pool
    pub fn create_pool(&mut self, pool_id: u32, fd: i32, size: i32) {
        self.pools.insert(
            pool_id,
            ShmPool {
                pool_id,
                fd,
                size: size as usize,
            },
        );
    }

    /// Create a buffer from a pool
    pub fn create_buffer(
        &mut self,
        buffer_id: u32,
        pool_id: u32,
        offset: i32,
        width: i32,
        height: i32,
        stride: i32,
        format: u32,
    ) -> Result<(), &'static str> {
        // Verify pool exists
        if !self.pools.contains_key(&pool_id) {
            return Err("Pool not found");
        }

        self.buffers.insert(
            buffer_id,
            ShmBuffer {
                buffer_id,
                pool_id,
                offset,
                width,
                height,
                stride,
                format,
            },
        );

        Ok(())
    }

    /// Get a buffer by ID
    pub fn get_buffer(&self, buffer_id: u32) -> Option<&ShmBuffer> {
        self.buffers.get(&buffer_id)
    }

    /// Get a pool by ID
    pub fn get_pool(&self, pool_id: u32) -> Option<&ShmPool> {
        self.pools.get(&pool_id)
    }

    /// Destroy a buffer
    pub fn destroy_buffer(&mut self, buffer_id: u32) {
        self.buffers.remove(&buffer_id);
    }

    /// Destroy a pool
    pub fn destroy_pool(&mut self, pool_id: u32) {
        self.pools.remove(&pool_id);
    }

    /// Resize a pool
    pub fn resize_pool(&mut self, pool_id: u32, new_size: i32) -> Result<(), &'static str> {
        let pool = self.pools.get_mut(&pool_id).ok_or("Pool not found")?;
        pool.size = new_size as usize;
        Ok(())
    }
}

/// wl_shm format constants
pub mod shm_format {
    /// ARGB8888 format (most common)
    pub const ARGB8888: u32 = 0;
    /// XRGB8888 format (no alpha channel)
    pub const XRGB8888: u32 = 1;
}

/// wl_shm opcodes (requests from client)
pub mod shm_request {
    pub const CREATE_POOL: u16 = 0;
}

/// wl_shm opcodes (events from server)
pub mod shm_event {
    pub const FORMAT: u16 = 0;
}

/// wl_shm_pool opcodes (requests from client)
pub mod shm_pool_request {
    pub const CREATE_BUFFER: u16 = 0;
    pub const DESTROY: u16 = 1;
    pub const RESIZE: u16 = 2;
}

/// wl_buffer opcodes (requests from client)
pub mod buffer_request {
    pub const DESTROY: u16 = 0;
}

/// wl_buffer opcodes (events from server)
pub mod buffer_event {
    pub const RELEASE: u16 = 0;
}
