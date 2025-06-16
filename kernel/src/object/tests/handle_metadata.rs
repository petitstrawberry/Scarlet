//! Tests for handle metadata system
//!
//! This module contains tests for HandleMetadata, HandleType, AccessMode,
//! and SpecialSemantics functionality.

use crate::object::{
    HandleMetadata, HandleType, StandardStreamType, AccessMode, SpecialSemantics,
    HandleTable, KernelObject
};
use super::mock::{MockFileObject, MockPipeObject};
use alloc::sync::Arc;

#[test_case]
fn test_handle_metadata_creation() {
    // Test basic metadata creation
    let metadata = HandleMetadata {
        handle_type: HandleType::Regular,
        access_mode: AccessMode::ReadWrite,
        special_semantics: None,
    };
    
    assert_eq!(metadata.handle_type, HandleType::Regular);
    assert_eq!(metadata.access_mode, AccessMode::ReadWrite);
    assert!(metadata.special_semantics.is_none());
}

#[test_case]
fn test_standard_stream_metadata() {
    // Test standard stream metadata
    let stdin_metadata = HandleMetadata {
        handle_type: HandleType::StandardStream(StandardStreamType::Stdin),
        access_mode: AccessMode::ReadOnly,
        special_semantics: None,
    };
    
    let stdout_metadata = HandleMetadata {
        handle_type: HandleType::StandardStream(StandardStreamType::Stdout),
        access_mode: AccessMode::WriteOnly,
        special_semantics: None,
    };
    
    let stderr_metadata = HandleMetadata {
        handle_type: HandleType::StandardStream(StandardStreamType::Stderr),
        access_mode: AccessMode::WriteOnly,
        special_semantics: None,
    };
    
    // Verify types
    match stdin_metadata.handle_type {
        HandleType::StandardStream(StandardStreamType::Stdin) => {},
        _ => panic!("Expected stdin type"),
    }
    
    match stdout_metadata.handle_type {
        HandleType::StandardStream(StandardStreamType::Stdout) => {},
        _ => panic!("Expected stdout type"),
    }
    
    match stderr_metadata.handle_type {
        HandleType::StandardStream(StandardStreamType::Stderr) => {},
        _ => panic!("Expected stderr type"),
    }
    
    // Verify access modes
    assert_eq!(stdin_metadata.access_mode, AccessMode::ReadOnly);
    assert_eq!(stdout_metadata.access_mode, AccessMode::WriteOnly);
    assert_eq!(stderr_metadata.access_mode, AccessMode::WriteOnly);
}

#[test_case]
fn test_ipc_channel_metadata() {
    // Test IPC channel metadata
    let ipc_metadata = HandleMetadata {
        handle_type: HandleType::IpcChannel,
        access_mode: AccessMode::ReadWrite,
        special_semantics: None,
    };
    
    assert_eq!(ipc_metadata.handle_type, HandleType::IpcChannel);
    assert_eq!(ipc_metadata.access_mode, AccessMode::ReadWrite);
}

#[test_case]
fn test_special_semantics() {
    // Test CloseOnExec semantics
    let close_on_exec_metadata = HandleMetadata {
        handle_type: HandleType::Regular,
        access_mode: AccessMode::ReadWrite,
        special_semantics: Some(SpecialSemantics::CloseOnExec),
    };
    
    assert!(close_on_exec_metadata.special_semantics.is_some());
    assert_eq!(close_on_exec_metadata.special_semantics.unwrap(), SpecialSemantics::CloseOnExec);
}

#[test_case]
fn test_access_mode_combinations() {
    // Test all access mode variants
    let read_only = AccessMode::ReadOnly;
    let write_only = AccessMode::WriteOnly;
    let read_write = AccessMode::ReadWrite;
    
    // Test equality
    assert_eq!(read_only, AccessMode::ReadOnly);
    assert_eq!(write_only, AccessMode::WriteOnly);
    assert_eq!(read_write, AccessMode::ReadWrite);
    
    // Test inequality
    assert_ne!(read_only, write_only);
    assert_ne!(read_only, read_write);
    assert_ne!(write_only, read_write);
}

#[test_case]
fn test_metadata_inference_for_files() {
    let mut table = HandleTable::new();
    let file_obj = Arc::new(MockFileObject::with_name_and_content("test.txt", "Hello, World!"));
    let kernel_obj = KernelObject::File(file_obj);
    
    // Insert without explicit metadata - should infer Regular type
    let handle = table.insert(kernel_obj).expect("Failed to insert file");
    
    let metadata = table.get_metadata(handle).expect("Metadata should exist");
    assert_eq!(metadata.handle_type, HandleType::Regular);
    assert_eq!(metadata.access_mode, AccessMode::ReadWrite);
    assert!(metadata.special_semantics.is_none());
}

#[test_case]
fn test_metadata_inference_for_pipes() {
    let mut table = HandleTable::new();
    let pipe_obj = Arc::new(MockPipeObject::new());
    let kernel_obj = KernelObject::Pipe(pipe_obj);
    
    // Insert without explicit metadata - should infer IpcChannel type for pipes
    let handle = table.insert(kernel_obj).expect("Failed to insert pipe");
    
    let metadata = table.get_metadata(handle).expect("Metadata should exist");
    assert_eq!(metadata.handle_type, HandleType::IpcChannel);
    assert_eq!(metadata.access_mode, AccessMode::ReadWrite);
    assert!(metadata.special_semantics.is_none());
}

#[test_case]
fn test_explicit_metadata_insertion() {
    let mut table = HandleTable::new();
    let file_obj = Arc::new(MockFileObject::with_name_and_content("stdin", ""));
    let kernel_obj = KernelObject::File(file_obj);
    
    // Create explicit metadata for stdin
    let stdin_metadata = HandleMetadata {
        handle_type: HandleType::StandardStream(StandardStreamType::Stdin),
        access_mode: AccessMode::ReadOnly,
        special_semantics: None,
    };
    
    // Insert with explicit metadata
    let handle = table.insert_with_metadata(kernel_obj, stdin_metadata.clone())
        .expect("Failed to insert with metadata");
    
    let retrieved_metadata = table.get_metadata(handle).expect("Metadata should exist");
    assert_eq!(retrieved_metadata.handle_type, stdin_metadata.handle_type);
    assert_eq!(retrieved_metadata.access_mode, stdin_metadata.access_mode);
    assert_eq!(retrieved_metadata.special_semantics, stdin_metadata.special_semantics);
}

#[test_case]
fn test_metadata_persistence_across_operations() {
    let mut table = HandleTable::new();
    let file_obj = Arc::new(MockFileObject::with_name_and_content("test.txt", "data"));
    let kernel_obj = KernelObject::File(file_obj);
    
    let metadata = HandleMetadata {
        handle_type: HandleType::IpcChannel,
        access_mode: AccessMode::ReadOnly,
        special_semantics: Some(SpecialSemantics::CloseOnExec),
    };
    
    let handle = table.insert_with_metadata(kernel_obj, metadata.clone())
        .expect("Failed to insert");
    
    // Verify metadata persists
    let retrieved = table.get_metadata(handle).expect("Metadata should exist");
    assert_eq!(retrieved.handle_type, metadata.handle_type);
    assert_eq!(retrieved.access_mode, metadata.access_mode);
    assert_eq!(retrieved.special_semantics, metadata.special_semantics);
    
    // Check that metadata is cleared when handle is removed
    let _obj = table.remove(handle).expect("Failed to remove handle");
    assert!(table.get_metadata(handle).is_none());
}

#[test_case]
fn test_metadata_with_iterator() {
    let mut table = HandleTable::new();
    
    // Insert multiple handles with different metadata
    let file1 = Arc::new(MockFileObject::with_name_and_content("file1.txt", "data1"));
    let file2 = Arc::new(MockFileObject::with_name_and_content("file2.txt", "data2"));
    let pipe1 = Arc::new(MockPipeObject::new());
    
    let regular_metadata = HandleMetadata {
        handle_type: HandleType::Regular,
        access_mode: AccessMode::ReadWrite,
        special_semantics: None,
    };
    
    let stdin_metadata = HandleMetadata {
        handle_type: HandleType::StandardStream(StandardStreamType::Stdin),
        access_mode: AccessMode::ReadOnly,
        special_semantics: None,
    };
    
    let ipc_metadata = HandleMetadata {
        handle_type: HandleType::IpcChannel,
        access_mode: AccessMode::ReadWrite,
        special_semantics: Some(SpecialSemantics::CloseOnExec),
    };
    
    let _h1 = table.insert_with_metadata(KernelObject::File(file1), regular_metadata).unwrap();
    let _h2 = table.insert_with_metadata(KernelObject::File(file2), stdin_metadata).unwrap();
    let _h3 = table.insert_with_metadata(KernelObject::Pipe(pipe1), ipc_metadata).unwrap();
    
    // Test iterator with metadata
    let mut count = 0;
    let mut found_regular = false;
    let mut found_stdin = false;
    let mut found_ipc = false;
    
    for (_handle, _obj, metadata) in table.iter_with_metadata() {
        count += 1;
        match metadata.handle_type {
            HandleType::Regular => found_regular = true,
            HandleType::StandardStream(StandardStreamType::Stdin) => found_stdin = true,
            HandleType::IpcChannel => found_ipc = true,
            _ => {}
        }
    }
    
    assert_eq!(count, 3);
    assert!(found_regular);
    assert!(found_stdin);
    assert!(found_ipc);
}

#[test_case]
fn test_metadata_clone() {
    let original = HandleMetadata {
        handle_type: HandleType::StandardStream(StandardStreamType::Stdout),
        access_mode: AccessMode::WriteOnly,
        special_semantics: Some(SpecialSemantics::CloseOnExec),
    };
    
    let cloned = original.clone();
    
    assert_eq!(original.handle_type, cloned.handle_type);
    assert_eq!(original.access_mode, cloned.access_mode);
    assert_eq!(original.special_semantics, cloned.special_semantics);
}
