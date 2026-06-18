//! Test DirtyFlags implementation

use scarlet_ui::DirtyFlags;

#[test]
fn test_dirty_flags_empty() {
    let flags = DirtyFlags::empty();
    assert!(flags.is_empty());
}

#[test]
fn test_dirty_flags_build() {
    let mut flags = DirtyFlags::empty();
    flags |= DirtyFlags::BUILD;
    assert!(flags.contains(DirtyFlags::BUILD));
    assert!(!flags.contains(DirtyFlags::LAYOUT));
}

#[test]
fn test_dirty_flags_multiple() {
    let mut flags = DirtyFlags::empty();
    flags |= DirtyFlags::BUILD;
    flags |= DirtyFlags::LAYOUT;
    flags |= DirtyFlags::PAINT;

    assert!(flags.contains(DirtyFlags::BUILD));
    assert!(flags.contains(DirtyFlags::LAYOUT));
    assert!(flags.contains(DirtyFlags::PAINT));
    assert!(!flags.contains(DirtyFlags::CHILDREN));
}

#[test]
fn test_dirty_flags_remove() {
    let mut flags = DirtyFlags::BUILD | DirtyFlags::PAINT;
    assert!(flags.contains(DirtyFlags::BUILD));

    flags -= DirtyFlags::BUILD;
    assert!(!flags.contains(DirtyFlags::BUILD));
    assert!(flags.contains(DirtyFlags::PAINT));
}

#[test]
fn test_dirty_flags_all() {
    let flags = DirtyFlags::BUILD | DirtyFlags::LAYOUT | DirtyFlags::PAINT | DirtyFlags::CHILDREN;
    assert!(flags.contains(DirtyFlags::BUILD));
    assert!(flags.contains(DirtyFlags::LAYOUT));
    assert!(flags.contains(DirtyFlags::PAINT));
    assert!(flags.contains(DirtyFlags::CHILDREN));
}

#[test]
fn test_dirty_flags_default() {
    let flags: DirtyFlags = Default::default();
    assert!(flags.is_empty());
}

#[test]
fn test_dirty_flags_combination() {
    let flags = DirtyFlags::BUILD | DirtyFlags::CHILDREN;
    assert!(flags.contains(DirtyFlags::BUILD));
    assert!(flags.contains(DirtyFlags::CHILDREN));
    assert!(!flags.contains(DirtyFlags::LAYOUT));
    assert!(!flags.contains(DirtyFlags::PAINT));
}
