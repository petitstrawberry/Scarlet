//! Windows NT Object Manager primitives.
//!
//! This module provides a small NT-style handle table used by the Windows ABI.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;

use crate::fs::FileObject;

/// Standard input pseudo-handle (`-4` in Win32 APIs).
pub const STD_INPUT_HANDLE: u32 = u32::MAX - 3;
/// Standard output pseudo-handle (`-9` in Win32 APIs).
pub const STD_OUTPUT_HANDLE: u32 = u32::MAX - 8;
/// Standard error pseudo-handle (`-11` in Win32 APIs).
pub const STD_ERROR_HANDLE: u32 = u32::MAX - 10;

/// NT file object wrapper.
#[derive(Clone)]
pub struct NtFileObject {
    /// Backing Scarlet file object.
    pub file: Arc<dyn FileObject>,
    /// Optional path used during open.
    pub path: Option<String>,
}

/// NT process object placeholder.
#[derive(Clone, Copy, Default)]
pub struct NtProcessObject {
    /// Process identifier.
    pub process_id: u32,
}

/// NT thread object placeholder.
#[derive(Clone, Copy, Default)]
pub struct NtThreadObject {
    /// Thread identifier.
    pub thread_id: u32,
}

/// NT event object placeholder.
#[derive(Clone, Copy, Default)]
pub struct NtEventObject;

/// NT timer object placeholder.
#[derive(Clone, Copy, Default)]
pub struct NtTimerObject;

#[derive(Clone)]
pub struct NtSectionObject {
    pub file: Option<Arc<dyn FileObject>>,
    pub maximum_size: u64,
    pub section_page_protection: u32,
    pub allocation_attributes: u32,
}

impl Default for NtSectionObject {
    fn default() -> Self {
        Self {
            file: None,
            maximum_size: 0,
            section_page_protection: 0,
            allocation_attributes: 0,
        }
    }
}

/// NT keyed mutex object placeholder.
#[derive(Clone, Copy, Default)]
pub struct NtKeyedMutexObject;

/// NT mutant (mutex) object placeholder.
#[derive(Clone, Copy, Default)]
pub struct NtMutantObject;

/// NT I/O completion object placeholder.
#[derive(Clone, Copy, Default)]
pub struct NtIoCompletionObject;

/// NT semaphore object placeholder.
#[derive(Clone, Copy, Default)]
pub struct NtSemaphoreObject;

/// NT debug object placeholder.
#[derive(Clone, Copy, Default)]
pub struct NtDebugObject;

/// NT object variants stored in the handle table.
#[derive(Clone)]
pub enum NtObject {
    /// Null object marker.
    Null,
    /// File object.
    File(NtFileObject),
    /// Process object.
    Process(NtProcessObject),
    /// Thread object.
    Thread(NtThreadObject),
    /// Event object.
    Event(NtEventObject),
    /// Timer object.
    Timer(NtTimerObject),
    /// Section object.
    Section(NtSectionObject),
    /// Keyed mutex object.
    KeyedMutex(NtKeyedMutexObject),
    /// Mutant object.
    Mutant(NtMutantObject),
    /// I/O completion object.
    IoCompletion(NtIoCompletionObject),
    /// Semaphore object.
    Semaphore(NtSemaphoreObject),
    /// Debug object.
    DebugObject(NtDebugObject),
}

/// Per-process NT object table.
#[derive(Clone)]
pub struct NtObjectTable {
    objects: BTreeMap<u32, NtObject>,
    next_handle: u32,
}

impl Default for NtObjectTable {
    fn default() -> Self {
        Self::new()
    }
}

impl NtObjectTable {
    /// Create an empty NT object table.
    ///
    /// Handle allocation starts at `4` and advances by `4`, keeping `0..=3`
    /// reserved for pseudo-handle semantics.
    pub fn new() -> Self {
        Self {
            objects: BTreeMap::new(),
            next_handle: 4,
        }
    }

    /// Insert an object and allocate a new handle.
    pub fn insert(&mut self, object: NtObject) -> u32 {
        let handle = self.next_handle;
        self.next_handle = self.next_handle.wrapping_add(4);
        self.objects.insert(handle, object);
        handle
    }

    /// Insert or replace an object at a fixed handle value.
    pub fn insert_at(&mut self, handle: u32, object: NtObject) {
        self.objects.insert(handle, object);
    }

    /// Get an object by handle.
    pub fn get(&self, handle: u32) -> Option<&NtObject> {
        self.objects.get(&handle)
    }

    /// Remove an object by handle.
    pub fn remove(&mut self, handle: u32) -> Option<NtObject> {
        self.objects.remove(&handle)
    }

    /// Register stdin/stdout/stderr pseudo-handles.
    pub fn register_console_pseudo_handles(&mut self) {
        self.insert_at(STD_INPUT_HANDLE, NtObject::Null);
        self.insert_at(STD_OUTPUT_HANDLE, NtObject::Null);
        self.insert_at(STD_ERROR_HANDLE, NtObject::Null);
    }
}
