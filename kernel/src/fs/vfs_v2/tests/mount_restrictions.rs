use alloc::{sync::Arc, string::ToString, vec};
use crate::fs::{VfsManager, FileType, FileSystemErrorKind, DeviceFileInfo};
use crate::fs::vfs_v2::{drivers::{tmpfs::TmpFS, overlayfs::OverlayFS}, mount_tree::MountRestrictions as R};

fn fixture() -> Arc<VfsManager> {
    let fs = Arc::new(VfsManager::new());
    fs.create_dir("/restricted").unwrap();
    fs.create_dir("/bind").unwrap();
    fs.mount(TmpFS::new(0), "/restricted", 14).unwrap();
    fs.create_file("/restricted/image", FileType::RegularFile).unwrap();
    fs
}
fn assert_restricted(fs: &VfsManager, path: &str) {
    let object = fs.open(path, 0).unwrap();
    let file = object.as_file().unwrap();
    assert!(!file.allows_execution());
    assert!(!file.allows_execution_privilege_gain());
    assert_eq!(object.as_memory_mappable().unwrap().maximum_mapping_permissions() & 4, 0);
}

#[test_case]
fn restrictions_survive_bind_rooted_view_namespace_copy_and_detached_handles() {
    let fs = fixture();
    fs.bind_mount("/restricted", "/bind").unwrap();
    assert_restricted(&fs, "/bind/image");
    let rooted = VfsManager::view_rooted_at(&fs, "/bind").unwrap();
    assert_restricted(&rooted, "/image");
    let copy = VfsManager::clone_mount_namespace_deep(&fs).unwrap();
    assert_restricted(&copy, "/restricted/image");
    let object = fs.open("/bind/image", 0).unwrap();
    fs.unmount("/bind").unwrap();
    assert!(!object.as_file().unwrap().allows_execution());
    assert_eq!(copy.resolve_path("/bind").unwrap().1.restrictions.bits(), 14);
    let (entry, mount) = rooted.resolve_path("/").unwrap();
    assert!(!rooted.open_from(&entry, &mount, "image", 0).unwrap().as_file().unwrap().allows_execution());
    assert!(!rooted.open_at(Some((&entry, &mount)), "image", 0, 0).unwrap().as_file().unwrap().allows_execution());
}

#[test_case]
fn nodev_blocks_all_opens_before_driver_but_keeps_metadata() {
    let fs = fixture();
    for (name, device_type) in [("char", crate::device::DeviceType::Char), ("block", crate::device::DeviceType::Block)] {
        let path = alloc::format!("/restricted/{name}");
        fs.create_device_file(&path, DeviceFileInfo { device_id: usize::MAX, device_type }).unwrap();
        assert!(fs.resolve_path(&path).unwrap().0.node().metadata().is_ok());
        let (entry, mount) = fs.resolve_path("/restricted").unwrap();
        for result in [fs.open(&path, 0), fs.open_at(None, &path, 0, 0), fs.open_from(&entry, &mount, name, 0)] {
            assert!(matches!(result, Err(e) if e.kind == FileSystemErrorKind::PermissionDenied));
        }
    }
}

#[test_case]
fn overlay_retains_actual_backing_mount_including_nested_restricted_mounts() {
    let fs = fixture();
    let overlay = OverlayFS::new_from_paths_and_vfs(None, vec![(&fs, "/")], "restricted-lower").unwrap();
    let view = VfsManager::new_with_root(overlay);
    assert_restricted(&view, "/restricted/image");
    fs.create_device_file("/restricted/char", DeviceFileInfo {
        device_id: usize::MAX, device_type: crate::device::DeviceType::Char }).unwrap();
    assert!(matches!(view.open("/restricted/char", 0), Err(e) if e.kind == FileSystemErrorKind::PermissionDenied));
}

#[test_case]
fn unsupported_mount_flags_do_not_replace_existing_mounts() {
    let fs = fixture();
    for flags in [1, 0x10, u32::MAX] {
        assert!(fs.mount(TmpFS::new(0), "/restricted", flags).is_err());
        assert_restricted(&fs, "/restricted/image");
    }
    assert_eq!(R::from_bits(14).unwrap().bits(), 14);
}
