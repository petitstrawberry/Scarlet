use super::*;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "scarlet-app-manager-test-{}-{}",
            std::process::id(),
            NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::create_dir(path.join("installed")).unwrap();
        Self(path)
    }
    fn root(&self) -> PathBuf {
        self.0.join("installed")
    }
    fn app(&self, slug: &str, id: &str) -> AppInfo {
        let path = self.0.join(slug);
        fs::create_dir_all(path.join("bin")).unwrap();
        fs::write(path.join(format!("{id}.desktop")), format!("[Desktop Entry]\nType=Application\nName=Test App\nExec=bin/test\nX-Scarlet-AppFormat=1\nX-Scarlet-Target=aarch64-unknown-scarlet\n")).unwrap();
        fs::write(path.join("bin/test"), elf()).unwrap();
        fs::set_permissions(path.join("bin/test"), fs::Permissions::from_mode(0o755)).unwrap();
        inspect(&path).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn elf() -> Vec<u8> {
    let mut bytes = vec![0; 120];
    bytes[..8].copy_from_slice(b"\x7fELF\x02\x01\x01\x53");
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&183u16.to_le_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
    bytes[64..68].copy_from_slice(&1u32.to_le_bytes());
    bytes[96..104].copy_from_slice(&120u64.to_le_bytes());
    bytes[104..112].copy_from_slice(&120u64.to_le_bytes());
    bytes
}
#[test]
fn install_publishes_only_complete_verified_app_and_reloads() {
    let f = Fixture::new();
    let app = f.app("test.app", "org.test");
    let mut calls = 0;
    install(&f.root(), &app, || {
        calls += 1;
        assert!(f.root().join("test.app/bin/test").is_file());
        Ok(1)
    })
    .unwrap();
    assert_eq!(calls, 1);
    assert!(app.path.is_dir());
    let apps = installed(&f.root()).unwrap();
    assert_eq!(apps.len(), 1);
    assert!(same_contents(&apps[0], &app));
    assert_eq!(children(&f.root()).unwrap().len(), 1);
}
#[test]
fn reload_failure_rolls_install_back_and_reconciles_again() {
    let f = Fixture::new();
    let app = f.app("test.app", "org.test");
    let mut calls = 0;
    let error = install(&f.root(), &app, || {
        calls += 1;
        if calls == 1 {
            Err("timeout".into())
        } else {
            assert!(!f.root().join("test.app").exists());
            Ok(0)
        }
    })
    .unwrap_err();
    assert!(error.contains("previous catalog reloaded"));
    assert_eq!(calls, 2);
    assert!(children(&f.root()).unwrap().is_empty());
    assert!(app.path.exists());
}
#[test]
fn ambiguous_reload_reports_unconfirmed_catalog_after_restoring_files() {
    let f = Fixture::new();
    let app = f.app("test.app", "org.test");
    let mut calls = 0;
    let error = install(&f.root(), &app, || {
        calls += 1;
        Err("offline".into())
    })
    .unwrap_err();
    assert!(error.contains("catalog state is unconfirmed"));
    assert_eq!(calls, 2);
    assert!(children(&f.root()).unwrap().is_empty());
}
#[test]
fn removal_is_reversible_until_catalog_confirms() {
    let f = Fixture::new();
    let app = f.app("test.app", "org.test");
    install(&f.root(), &app, || Ok(1)).unwrap();
    let selected = installed(&f.root()).unwrap().remove(0);
    let mut calls = 0;
    let error = remove(&f.root(), &selected, || {
        calls += 1;
        if calls == 1 {
            assert!(!selected.path.exists());
            Err("rejected".into())
        } else {
            assert!(selected.path.exists());
            Ok(1)
        }
    })
    .unwrap_err();
    assert!(error.contains("Removal cancelled"));
    assert!(selected.path.exists());
    assert_eq!(calls, 2);
    remove(&f.root(), &selected, || Ok(0)).unwrap();
    assert!(children(&f.root()).unwrap().is_empty());
    assert!(app.path.exists());
}
#[test]
fn refuses_duplicate_ids_existing_destinations_and_changed_source() {
    let f = Fixture::new();
    let app = f.app("test.app", "org.test");
    install(&f.root(), &app, || Ok(1)).unwrap();
    let duplicate = f.app("other.app", "org.test");
    assert!(
        install(&f.root(), &duplicate, || panic!("must not reload"))
            .unwrap_err()
            .contains("already installed")
    );
    let fresh = f.app("fresh.app", "org.fresh");
    fs::write(f.root().join("fresh.app"), b"keep").unwrap();
    assert!(install(&f.root(), &fresh, || panic!("must not reload")).is_err());
    assert_eq!(fs::read(f.root().join("fresh.app")).unwrap(), b"keep");
    fs::write(duplicate.path.join("new-resource"), b"changed").unwrap();
    assert!(
        install(&f.root(), &duplicate, || panic!("must not reload"))
            .unwrap_err()
            .contains("changed after inspection")
    );
}
#[test]
fn refuses_remove_outside_root_or_changed_app() {
    let f = Fixture::new();
    let app = f.app("test.app", "org.test");
    assert!(remove(&f.root(), &app, || panic!()).is_err());
    install(&f.root(), &app, || Ok(1)).unwrap();
    let selected = installed(&f.root()).unwrap().remove(0);
    fs::write(selected.path.join("extra"), b"new").unwrap();
    assert!(
        remove(&f.root(), &selected, || panic!())
            .unwrap_err()
            .contains("changed")
    );
    assert!(selected.path.exists());
}
#[test]
fn rejects_symlinks_recipes_nonportable_paths_and_missing_execute_permission() {
    let f = Fixture::new();
    let app = f.app("test.app", "org.test");
    symlink(app.path.join("bin/test"), app.path.join("link")).unwrap();
    assert!(inspect(&app.path).unwrap_err().contains("symlink"));
    fs::remove_file(app.path.join("link")).unwrap();
    fs::write(app.path.join("Cargo.toml"), b"source").unwrap();
    assert!(inspect(&app.path).unwrap_err().contains("recipes"));
    fs::remove_file(app.path.join("Cargo.toml")).unwrap();
    fs::write(app.path.join("space name"), b"x").unwrap();
    assert!(inspect(&app.path).unwrap_err().contains("filename"));
    fs::remove_file(app.path.join("space name")).unwrap();
    fs::set_permissions(app.path.join("bin/test"), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        inspect(&app.path)
            .unwrap_err()
            .contains("execute permission")
    );
}
#[test]
fn rejects_mixed_abi_and_malformed_embedded_elf() {
    let f = Fixture::new();
    let app = f.app("test.app", "org.test");
    let path = app.path.join("embedded");
    let mut bytes = elf();
    bytes[7] = 0;
    fs::write(&path, &bytes).unwrap();
    assert!(inspect(&app.path).is_err());
    bytes = elf();
    bytes[18..20].copy_from_slice(&243u16.to_le_bytes());
    fs::write(&path, &bytes).unwrap();
    assert!(inspect(&app.path).is_err());
    fs::write(&path, b"\x7fELF").unwrap();
    assert!(inspect(&app.path).unwrap_err().contains("Truncated"));
}
#[test]
fn validates_native_interpreter_and_program_bounds() {
    let f = Fixture::new();
    let app = f.app("test.app", "org.test");
    let path = app.path.join("bin/test");
    let mut bytes = elf();
    bytes.resize(192, 0);
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&2u16.to_le_bytes());
    bytes[120..124].copy_from_slice(&3u32.to_le_bytes());
    bytes[128..136].copy_from_slice(&176u64.to_le_bytes());
    bytes[152..160].copy_from_slice(&16u64.to_le_bytes());
    bytes[176..].copy_from_slice(b"/bin/scarlet-ld\0");
    fs::write(&path, &bytes).unwrap();
    inspect(&app.path).unwrap();
    bytes[177] = b'x';
    fs::write(&path, &bytes).unwrap();
    assert!(inspect(&app.path).unwrap_err().contains("interpreter"));
    bytes[128..136].copy_from_slice(&u64::MAX.to_le_bytes());
    fs::write(&path, &bytes).unwrap();
    assert!(inspect(&app.path).unwrap_err().contains("bounds"));
}
#[test]
fn shared_descriptor_rules_and_slug_identity_are_preserved() {
    for name in [".app", "...app", "../a.app", "Upper.app", "a b.app"] {
        assert!(!valid_slug(name), "{name}");
    }
    let f = Fixture::new();
    let app = f.app("different-name.app", "org.stable.Id");
    assert_eq!(app.id, "org.stable.Id");
    fs::write(app.path.join("second.desktop"), b"[Desktop Entry]").unwrap();
    assert!(inspect(&app.path).unwrap_err().contains("exactly one"));
}
#[test]
fn operation_lock_prevents_overlapping_transactions_without_removing_foreign_lock() {
    let f = Fixture::new();
    let app = f.app("test.app", "org.test");
    let lock = OperationLock::acquire(&f.root()).unwrap();
    assert!(install(&f.root(), &app, || panic!()).is_err());
    assert!(lock.0.is_dir());
    drop(lock);
    install(&f.root(), &app, || Ok(1)).unwrap();
}

#[test]
fn deep_resources_use_bounded_traversal_without_large_stack_frames() {
    let f = Fixture::new();
    let app = f.app("test.app", "org.test");
    let mut path = app.path.clone();
    for _ in 0..60 {
        path.push("d");
        fs::create_dir(&path).unwrap();
    }
    inspect(&app.path).unwrap();
    for _ in 0..6 {
        path.push("d");
        fs::create_dir(&path).unwrap();
    }
    assert!(inspect(&app.path).unwrap_err().contains("64-level"));
}
