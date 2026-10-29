use alloc::{sync::Arc, string::ToString};
use crate::fs::{VfsManager, FileType, FileSystemErrorKind, drivers::tmpfs::TmpFS};
use crate::fs::vfs_v2::propagation::{self, Propagation as P};

fn base() -> Arc<VfsManager> {
    let fs = Arc::new(VfsManager::new());
    for path in ["/m", "/n", "/source", "/dest"] { fs.create_dir(path).unwrap(); }
    fs
}
fn mark(fs: &VfsManager, path: &str) { fs.create_file(path, FileType::RegularFile).unwrap(); }
fn info(fs: &VfsManager, path: &str) -> propagation::PropagationInfo {
    propagation::information(&fs.resolve_path(path).unwrap().1).unwrap()
}
fn shared(fs: &VfsManager) { fs.set_mount_propagation("/", P::Shared, false).unwrap(); }
fn downstream(source: &Arc<VfsManager>, shared_group: bool) -> Arc<VfsManager> {
    let fs = VfsManager::clone_mount_namespace_deep(source).unwrap();
    fs.set_mount_propagation("/", P::Downstream, false).unwrap();
    if shared_group { shared(&fs); }
    fs
}

#[test_case]
fn shared_namespace_peers_publish_mounts_in_both_directions_and_detach_them() {
    let a = base(); shared(&a);
    let b = VfsManager::clone_mount_namespace_deep(&a).unwrap();
    assert_eq!(info(&a, "/"), info(&b, "/"));
    a.mount(TmpFS::new(0), "/m", 14).unwrap(); mark(&a, "/m/marker");
    assert!(b.resolve_path("/m/marker").is_ok());
    assert_eq!(info(&a, "/m"), info(&b, "/m"));
    assert!(!b.open("/m/marker", 0).unwrap().as_file().unwrap().allows_execution());
    b.mount(TmpFS::new(0), "/n", 0).unwrap(); mark(&b, "/n/back");
    assert!(a.resolve_path("/n/back").is_ok());
    b.unmount("/m").unwrap();
    assert!(a.resolve_path("/m/marker").is_err());
    assert!(b.resolve_path("/m/marker").is_err());
}

#[test_case]
fn downstream_groups_receive_upstream_events_but_only_send_to_their_own_peers() {
    let a = base(); shared(&a);
    let b = downstream(&a, true);
    let c = VfsManager::clone_mount_namespace_deep(&b).unwrap();
    let d = downstream(&b, false);
    a.mount(TmpFS::new(0), "/m", 0).unwrap(); mark(&a, "/m/upstream");
    for fs in [&b, &c, &d] { assert!(fs.resolve_path("/m/upstream").is_ok()); }
    b.mount(TmpFS::new(0), "/n", 0).unwrap(); mark(&b, "/n/local");
    for fs in [&c, &d] { assert!(fs.resolve_path("/n/local").is_ok()); }
    assert!(a.resolve_path("/n/local").is_err());
    assert!(info(&b, "/m").peer_group.is_some());
    assert_eq!(info(&d, "/m").peer_group, None);
    assert_eq!(info(&d, "/m").upstream_group, info(&b, "/m").peer_group);
}

#[test_case]
fn divergent_downstream_mount_causes_atomic_publication_failure_and_is_not_unmounted_upstream() {
    let a = base(); shared(&a);
    let b = downstream(&a, true);
    let c = VfsManager::clone_mount_namespace_deep(&b).unwrap();
    a.mount(TmpFS::new(0), "/m", 0).unwrap();
    b.unmount("/m").unwrap();
    assert!(a.resolve_path("/m").unwrap().1.parent.read().is_some());
    b.mount(TmpFS::new(0), "/m", 8).unwrap(); mark(&b, "/m/local");
    a.unmount("/m").unwrap();
    assert!(b.resolve_path("/m/local").is_ok());
    assert!(c.resolve_path("/m/local").is_ok());
    assert!(matches!(a.mount(TmpFS::new(0), "/m", 0), Err(e) if e.kind == FileSystemErrorKind::Busy));
    assert!(a.resolve_path("/m/local").is_err());
    assert!(Arc::ptr_eq(&a.resolve_path("/m").unwrap().1, &a.mount_tree.root_mount.read()));
    assert!(b.resolve_path("/m/local").is_ok());
}

#[test_case]
fn independent_rooted_view_starts_private_and_never_connects_to_outer_shared_tree() {
    let host = base(); shared(&host);
    let android = VfsManager::view_rooted_at(&host, "/source").unwrap();
    android.create_dir("/local").unwrap();
    assert_eq!(info(&android, "/").peer_group, None);
    shared(&android);
    android.mount(TmpFS::new(0), "/local", 0).unwrap(); mark(&android, "/local/private");
    assert!(host.resolve_path("/source/local/private").is_err());
    assert_ne!(info(&host, "/").peer_group, info(&android, "/").peer_group);
}

#[test_case]
fn recursive_bind_copies_submounts_and_filters_future_events_outside_source_subtree() {
    let fs = base();
    fs.create_dir("/source/nested").unwrap();
    fs.mount(TmpFS::new(0), "/source/nested", 14).unwrap(); mark(&fs, "/source/nested/initial");
    shared(&fs);
    fs.bind_mount_recursive("/source", "/dest", true).unwrap();
    assert!(fs.resolve_path("/dest/nested/initial").is_ok());
    assert_eq!(fs.resolve_path("/dest/nested").unwrap().1.restrictions.bits(), 14);
    fs.create_dir("/source/new").unwrap();
    fs.mount(TmpFS::new(0), "/source/new", 0).unwrap(); mark(&fs, "/source/new/future");
    assert!(fs.resolve_path("/dest/new/future").is_ok());
    fs.mount(TmpFS::new(0), "/n", 0).unwrap(); mark(&fs, "/n/outside");
    assert!(fs.resolve_path("/dest/n/outside").is_err());
    // A copied child's .. reaches its destination mount parent, not source mount.
    assert_eq!(fs.canonicalize("/dest/nested/..").unwrap(), "/dest");
}

#[test_case]
fn recursive_propagation_to_downstream_does_not_rejoin_nested_source_peer_groups() {
    let a = base();
    a.create_dir("/source/nested").unwrap();
    a.mount(TmpFS::new(0), "/source/nested", 0).unwrap();
    a.create_dir("/source/nested/local").unwrap();
    a.set_mount_propagation("/", P::Shared, true).unwrap();
    let b = downstream(&a, true);
    a.bind_mount_recursive("/source", "/dest", true).unwrap();
    let upstream = info(&a, "/dest/nested");
    let receiving = info(&b, "/dest/nested");
    assert_ne!(upstream.peer_group, receiving.peer_group);
    assert_eq!(receiving.upstream_group, upstream.peer_group);
    b.mount(TmpFS::new(0), "/dest/nested/local", 0).unwrap(); mark(&b, "/dest/nested/local/only-b");
    assert!(a.resolve_path("/dest/nested/local/only-b").is_err());
    assert!(a.resolve_path("/source/nested/local/only-b").is_err());
}

#[test_case]
fn recursive_private_breaks_all_shared_and_upstream_relationships() {
    let a = base(); a.set_mount_propagation("/", P::Shared, true).unwrap();
    a.mount(TmpFS::new(0), "/m", 0).unwrap();
    let b = VfsManager::clone_mount_namespace_deep(&a).unwrap();
    b.set_mount_propagation("/", P::Private, true).unwrap();
    for (_, mount) in b.mounted_paths() { assert_eq!(propagation::information(&mount).unwrap(), Default::default()); }
    b.mount(TmpFS::new(0), "/n", 0).unwrap(); mark(&b, "/n/local");
    assert!(a.resolve_path("/n/local").is_err());
}

#[test_case]
fn weak_topology_does_not_retain_dead_view_even_when_an_open_file_pins_its_mount() {
    let a = base(); shared(&a);
    let b = VfsManager::clone_mount_namespace_deep(&a).unwrap();
    mark(&b, "/pinned");
    let object = b.open("/pinned", 0).unwrap();
    let weak = Arc::downgrade(&b.view());
    drop(b);
    assert!(weak.upgrade().is_none());
    a.mount(TmpFS::new(0), "/m", 0).unwrap();
    assert!(object.as_file().unwrap().metadata().is_ok());
}

#[test_case]
fn downstream_flow_survives_last_intermediate_peer_exit() {
    let a = base(); shared(&a);
    let b = downstream(&a, true);
    let c = downstream(&b, false);
    drop(b);
    a.mount(TmpFS::new(0), "/m", 0).unwrap(); mark(&a, "/m/after-exit");
    assert!(c.resolve_path("/m/after-exit").is_ok());
    assert_eq!(info(&c, "/").upstream_group, info(&a, "/").peer_group);
}

#[test_case]
fn shared_self_bind_does_not_receive_its_own_unpublished_attachment() {
    let fs = base(); shared(&fs);
    fs.bind_mount_recursive("/", "/dest", false).unwrap();
    let bound = fs.resolve_path("/dest").unwrap().1;
    assert!(bound.children.read().is_empty());
    assert_eq!(info(&fs, "/dest").peer_group, info(&fs, "/").peer_group);
}
