/// Simplified VFS v2 tests
///
/// These are basic tests to verify that VFS v2 components compile and work correctly.
pub mod advanced_tests;
pub mod cross_vfs_tests;
pub mod performance_tests;
pub mod symlink_cross_fs_test;

use crate::fs::vfs_v2::{
    core::*,
    drivers::tmpfs::TmpFS,
    manager::VfsManager,
    mount_tree::{MountOptionsV2, MountPoint, MountTree, MountType},
};
use alloc::{string::ToString, sync::Arc};

#[test_case]
fn test_nested_cwd_retains_ancestors_and_releases_them() {
    let vfs = VfsManager::new();
    vfs.create_dir("/usr").unwrap();
    vfs.create_dir("/usr/share").unwrap();
    vfs.create_dir("/usr/share/icons").unwrap();
    vfs.set_cwd_by_path("/usr").unwrap();
    vfs.set_cwd_by_path("share").unwrap();
    vfs.set_cwd_by_path("icons").unwrap();

    assert_eq!(vfs.get_cwd_path(), "/usr/share/icons");
    assert_eq!(vfs.resolve_path_to_absolute("."), "/usr/share/icons");
    assert_eq!(vfs.resolve_path_to_absolute("../"), "/usr/share");
    assert!(vfs.open(".", 0).is_ok());

    let (icons, _) = vfs.get_cwd().unwrap();
    let share = icons.parent().unwrap();
    let usr = share.parent().unwrap();
    let weak_icons = Arc::downgrade(&icons);
    let weak_share = Arc::downgrade(&share);
    let weak_usr = Arc::downgrade(&usr);
    let (resolved_parent, _) = vfs.resolve_path("..").unwrap();
    assert!(Arc::ptr_eq(&resolved_parent, &share));
    drop((icons, share, usr, resolved_parent));

    vfs.set_cwd_by_path("..").unwrap();
    assert_eq!(vfs.get_cwd_path(), "/usr/share");
    assert!(weak_icons.upgrade().is_none());
    assert!(weak_share.upgrade().is_some());
    assert!(weak_usr.upgrade().is_some());

    vfs.set_cwd_by_path("/").unwrap();
    assert!(weak_share.upgrade().is_none());
    assert!(weak_usr.upgrade().is_none());
}

/// Test basic mount tree operations
#[test_case]
fn test_mount_tree_basic() {
    // Create root TmpFS
    let root_tmpfs = TmpFS::new(1024 * 1024);
    let root_node = root_tmpfs.root_node();
    let root_entry = VfsEntry::new(None, "/".to_string(), root_node);

    // Create mount tree
    let mount_tree = MountTree::new(root_entry.clone(), root_tmpfs.clone());

    // Test basic functionality
    assert_eq!(mount_tree.root_mount.read().root.name(), "/");
    // For now, just verify that the mount tree was created successfully
}

/// Test mount point creation
#[test_case]
fn test_mount_point_creation() {
    // Create TmpFS
    let tmpfs = TmpFS::new(1024 * 1024);
    let root_node = tmpfs.root_node();
    let entry = VfsEntry::new(None, "/".to_string(), root_node);

    // Create mount point
    let mount_point = MountPoint::new_regular("/mnt".to_string(), entry.clone(), tmpfs.clone());

    // Test properties
    assert_eq!(*mount_point.path.read(), "/mnt");
    assert!(matches!(mount_point.mount_type, MountType::Regular));
}

/// Test VfsManager creation
#[test_case]
fn test_vfs_manager_creation() {
    let manager = VfsManager::new();

    // Test that manager is created successfully
    // Just verify it can be created without panicking
    let _manager_arc = Arc::new(manager);
}

/// Removing a socket node must also release its local-socket registry name.
#[cfg(feature = "network")]
#[test_case]
fn test_remove_socket_file_unregisters_named_socket() {
    use crate::fs::{FileType, SocketFileInfo};
    use crate::network::local::LocalSocket;
    use crate::network::{
        LocalSocketAddress, NetworkManager, SocketAddress, SocketError, SocketObject,
        SocketProtocol, SocketType,
    };

    let vfs = VfsManager::new();
    let path = "/vfs-remove-named-socket-test";
    let socket: Arc<dyn SocketObject> = Arc::new(LocalSocket::new(
        SocketType::Stream,
        SocketProtocol::Default,
    ));
    socket
        .bind(&SocketAddress::Local(
            LocalSocketAddress::from_path(path).unwrap(),
        ))
        .unwrap();

    let network = NetworkManager::get_manager();
    let socket_id = network.allocate_socket_id(Arc::clone(&socket)).unwrap();
    network
        .register_named_socket(path, Arc::clone(&socket))
        .unwrap();
    vfs.create_file(path, FileType::Socket(SocketFileInfo { socket_id }))
        .unwrap();

    vfs.remove(path).unwrap();
    assert!(matches!(
        network.lookup_named_socket(path),
        Err(SocketError::ConnectionRefused)
    ));
}

/// Test mount options
#[test_case]
fn test_mount_options() {
    let options = MountOptionsV2 {
        readonly: true,
        flags: 0,
    };

    let default_options = MountOptionsV2::default();

    assert!(options.readonly);
    assert!(!default_options.readonly);
}
