//! Resolve aliases explicitly: Scarlet's current std canonicalize only makes
//! a path absolute, which is insufficient for once-only DSO identity.
//! Use has_root for Scarlet slash paths: the current native std is_absolute
//! incorrectly requires a Windows-style prefix on non-Unix-configured targets.
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

fn components(path: &Path) -> io::Result<VecDeque<OsString>> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(Ok(name.to_owned())),
            Component::ParentDir => Some(Ok(OsString::from(".."))),
            Component::CurDir | Component::RootDir => None,
            Component::Prefix(_) => Some(Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "non-native pathname prefix",
            ))),
        })
        .collect()
}

pub(crate) fn resolve(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.has_root() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut pending = components(&absolute)?;
    let mut resolved = PathBuf::from("/");
    let mut links = 0;
    while let Some(component) = pending.pop_front() {
        if component == ".." {
            resolved.pop();
            continue;
        }
        resolved.push(&component);
        let metadata = std::fs::symlink_metadata(&resolved)?;
        if metadata.file_type().is_symlink() {
            links += 1;
            if links > 40 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "too many symbolic links in library pathname",
                ));
            }
            let target = std::fs::read_link(&resolved)?;
            if target.as_os_str().is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "empty symbolic link",
                ));
            }
            resolved.pop();
            if target.has_root() {
                resolved = PathBuf::from("/");
            }
            let mut next = components(&target)?;
            next.append(&mut pending);
            pending = next;
        } else if !pending.is_empty() && !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "non-directory library pathname component",
            ));
        }
    }
    Ok(resolved)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn canonical_identity_follows_symlinks_before_parent_components() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("scarlet-dl-path-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(root.join("real/sub")).unwrap();
        std::fs::write(root.join("real/lib.so"), b"fixture").unwrap();
        std::os::unix::fs::symlink("real/sub", root.join("alias")).unwrap();
        std::os::unix::fs::symlink("loop", root.join("loop")).unwrap();
        assert_eq!(
            resolve(&root.join("alias/../lib.so")).unwrap(),
            std::fs::canonicalize(root.join("real/lib.so")).unwrap()
        );
        assert!(resolve(&root.join("loop")).is_err());
        assert!(resolve(&root.join("real/lib.so/..")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
