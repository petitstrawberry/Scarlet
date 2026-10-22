//! Finished `.app` inspection and transactional catalog changes.
//!
//! The shared stemd parser owns descriptor semantics. These extra checks cover
//! the complete distribution tree, including resources and embedded ELF files.
//! Transactions keep unpublished data outside `*.app`; a failed reload restores
//! the old tree and reconciles again, including after ambiguous transport errors.
use crate::desktop_catalog::native_fs as fs;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "../desktop_utilities/fs_sync.rs"]
mod fs_sync;

const MAX_ITEMS: usize = 100_000;
const MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_DEPTH: usize = 64;
static NEXT_STAGE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppInfo {
    pub path: PathBuf,
    pub name: String,
    pub id: String,
    pub target: String,
    pub bytes: u64,
    pub files: usize,
    fingerprint: u64,
}

pub fn valid_slug(name: &str) -> bool {
    name.strip_suffix(".app").is_some_and(|slug| {
        !slug.is_empty()
            && !matches!(slug, "." | "..")
            && slug
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
    })
}
fn portable(name: &str) -> bool {
    !name.is_empty()
        && !matches!(name, "." | "..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
fn text_path(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "App paths must be UTF-8".into())
}
fn real_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| format!("Cannot inspect {}: {e}", path.display()))?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(format!("{} must be a real directory", path.display()))
    }
}

pub fn inspect(path: &Path) -> Result<AppInfo, String> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("Choose a .app directory")?;
    if !valid_slug(name) {
        return Err("Choose a lowercase-slug.app directory".into());
    }
    inspect_contents(path)
}
fn inspect_contents(path: &Path) -> Result<AppInfo, String> {
    real_directory(path)?;
    let entry = crate::desktop_catalog::validate_app_directory(text_path(path)?)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let executable = crate::desktop_catalog::expand_exec(&entry.exec, &[])
        .map_err(str::to_string)?
        .into_iter()
        .next()
        .ok_or("Missing app executable")?;
    let mut header = [0; 20];
    fs::open_read_nofollow(&executable)
        .and_then(|mut f| f.read_exact(&mut header))
        .map_err(|e| e.to_string())?;
    let machine = u16::from_le_bytes([header[18], header[19]]);
    let mut tree = Tree {
        items: 0,
        files: 0,
        bytes: 0,
        fingerprint: 0xcbf29ce484222325,
    };
    inspect_tree(path, path, 0, machine, Path::new(&executable), &mut tree)?;
    Ok(AppInfo {
        path: path.to_path_buf(),
        name: entry.name,
        id: entry.app_id,
        target: if machine == 183 {
            "aarch64-unknown-scarlet"
        } else {
            "riscv64gc-unknown-scarlet"
        }
        .into(),
        files: tree.files,
        bytes: tree.bytes,
        fingerprint: tree.fingerprint,
    })
}
struct Tree {
    items: usize,
    files: usize,
    bytes: u64,
    fingerprint: u64,
}
impl Tree {
    fn hash(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.fingerprint = (self.fingerprint ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
    }
}
fn children(path: &Path) -> Result<Vec<std::fs::DirEntry>, String> {
    let mut entries = fs::read_dir(path)
        .map_err(|e| format!("Cannot list {}: {e}", path.display()))?
        .take(MAX_ITEMS + 1)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if entries.len() > MAX_ITEMS {
        return Err("Directory exceeds the 100,000 item inspection limit".into());
    }
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}
fn inspect_tree(
    root: &Path,
    path: &Path,
    depth: usize,
    machine: u16,
    executable: &Path,
    tree: &mut Tree,
) -> Result<(), String> {
    tree.items += 1;
    if tree.items > MAX_ITEMS || depth > MAX_DEPTH {
        return Err("App exceeds the 100,000 item or 64-level inspection limit".into());
    }
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err(format!("App symlinks are unsupported: {}", path.display()));
    }
    if depth > 0 {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("App filenames must be portable ASCII")?;
        if !portable(name) {
            return Err(format!("Invalid app filename: {}", path.display()));
        }
        if matches!(name, "app.toml" | "Cargo.toml" | "bundle.toml") {
            return Err("Finished apps must not contain build recipes".into());
        }
    }
    tree.hash(
        path.strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .as_bytes(),
    );
    tree.hash(&[0, u8::from(metadata.is_dir())]);
    tree.hash(&fs::permissions_mode(&metadata.permissions()).to_le_bytes());
    if metadata.is_dir() {
        for child in children(path)? {
            inspect_tree(root, &child.path(), depth + 1, machine, executable, tree)?;
        }
    } else if metadata.is_file() {
        tree.files += 1;
        tree.bytes = tree
            .bytes
            .checked_add(metadata.len())
            .ok_or("App size overflow")?;
        if tree.bytes > MAX_BYTES {
            return Err("App exceeds the 8 GiB inspection limit".into());
        }
        let mut file = fs::open_read_nofollow(path).map_err(|e| e.to_string())?;
        // Stream every byte into a change detector; this is not a signature or trust check.
        let mut buffer = vec![0u8; 64 * 1024];
        let mut actual = 0u64;
        loop {
            let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            actual += n as u64;
            if actual > metadata.len() {
                return Err("App changed during inspection; inspect it again".into());
            }
            tree.hash(&buffer[..n]);
        }
        if actual != metadata.len() {
            return Err("App changed during inspection; inspect it again".into());
        }
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut magic = [0; 4];
        if file.read_exact(&mut magic).is_ok() && magic == *b"\x7fELF" {
            validate_elf(&mut file, metadata.len(), machine)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        if path == executable {
            if !fs::is_executable(&metadata.permissions()) {
                return Err("App executable has no execute permission".into());
            }
        }
    } else {
        return Err(format!(
            "Only regular files and directories may be installed: {}",
            path.display()
        ));
    }
    Ok(())
}
fn validate_elf(file: &mut File, length: u64, machine: u16) -> Result<(), String> {
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header)
        .map_err(|_| "Truncated ELF header")?;
    if &header[..8] != b"\x7fELF\x02\x01\x01\x53"
        || !matches!(u16::from_le_bytes([header[16], header[17]]), 2 | 3)
        || u16::from_le_bytes([header[18], header[19]]) != machine
        || u32::from_le_bytes(header[20..24].try_into().unwrap()) != 1
        || u16::from_le_bytes([header[52], header[53]]) != 64
    {
        return Err("Incompatible native ELF file".into());
    }
    let offset = u64::from_le_bytes(header[32..40].try_into().unwrap());
    let size = u16::from_le_bytes([header[54], header[55]]) as u64;
    let count = u16::from_le_bytes([header[56], header[57]]) as u64;
    if count == 0
        || count > 4096
        || offset < 64
        || size != 56
        || offset
            .checked_add(size * count)
            .is_none_or(|end| end > length)
    {
        return Err("Invalid ELF program headers".into());
    }
    let mut interpreter_seen = false;
    let mut load_seen = false;
    for index in 0..count {
        file.seek(SeekFrom::Start(offset + index * size))
            .map_err(|e| e.to_string())?;
        let mut program = [0u8; 56];
        file.read_exact(&mut program).map_err(|e| e.to_string())?;
        let kind = u32::from_le_bytes(program[..4].try_into().unwrap());
        if kind == 1 || kind == 3 {
            let start = u64::from_le_bytes(program[8..16].try_into().unwrap());
            let bytes = u64::from_le_bytes(program[32..40].try_into().unwrap());
            if start.checked_add(bytes).is_none_or(|end| end > length) {
                return Err("ELF segment exceeds file bounds".into());
            }
            if kind == 1 {
                let memory = u64::from_le_bytes(program[40..48].try_into().unwrap());
                if bytes > memory {
                    return Err("ELF file segment exceeds its memory extent".into());
                }
                load_seen |= memory > 0;
            }
            if kind == 3 {
                if interpreter_seen || bytes != b"/bin/scarlet-ld\0".len() as u64 {
                    return Err("Unsupported ELF interpreter".into());
                }
                interpreter_seen = true;
                file.seek(SeekFrom::Start(start))
                    .map_err(|e| e.to_string())?;
                let mut interpreter = [0u8; 16];
                file.read_exact(&mut interpreter)
                    .map_err(|e| e.to_string())?;
                if &interpreter != b"/bin/scarlet-ld\0" {
                    return Err("Unsupported ELF interpreter".into());
                }
            }
        }
    }
    if !load_seen {
        return Err("ELF file has no loadable segment".into());
    }
    Ok(())
}

pub fn installed(root: &Path) -> Result<Vec<AppInfo>, String> {
    real_directory(root)?;
    let mut apps = Vec::<AppInfo>::new();
    for entry in children(root)? {
        if !entry.file_name().to_string_lossy().ends_with(".app") {
            continue;
        }
        let app = inspect(&entry.path())?;
        if apps.iter().any(|old| old.id == app.id) {
            return Err(format!("Duplicate installed app ID: {}", app.id));
        }
        apps.push(app);
    }
    apps.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(apps)
}

struct OperationLock(PathBuf);
impl OperationLock {
    fn acquire(root: &Path) -> Result<Self, String> {
        real_directory(root)?;
        let lock = root.join(".applications-operation");
        fs::create_dir(&lock).map_err(|e| format!("Cannot reserve {}: {e}. Another operation may be active; after a crash, inspect this directory before removing its lock.", lock.display()))?;
        Ok(Self(lock))
    }
}
impl Drop for OperationLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}
fn stage(root: &Path) -> Result<PathBuf, String> {
    for _ in 0..128 {
        let path = root.join(format!(
            ".applications-stage-{}-{}",
            std::process::id(),
            NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Cannot create staging directory: {e}")),
        }
    }
    Err("Cannot reserve an app staging directory".into())
}
fn absent(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(format!(
            "{} already exists; remove the installed version first",
            path.display()
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Cannot inspect destination: {e}")),
    }
}
fn copy_tree(
    source: &Path,
    destination: &Path,
    depth: usize,
    items: &mut usize,
    total_bytes: &mut u64,
) -> Result<(), String> {
    *items += 1;
    if depth > MAX_DEPTH || *items > MAX_ITEMS {
        return Err("App changed beyond inspection limits".into());
    }
    let metadata = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("App acquired a symlink during copying".into());
    }
    if metadata.is_dir() {
        fs::create_dir(destination).map_err(|e| e.to_string())?;
        for child in children(source)? {
            copy_tree(
                &child.path(),
                &destination.join(child.file_name()),
                depth + 1,
                items,
                total_bytes,
            )?;
        }
        fs::set_permissions(destination, metadata.permissions()).map_err(|e| e.to_string())?;
    } else if metadata.is_file() {
        *total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or("App size overflow while copying")?;
        if *total_bytes > MAX_BYTES {
            return Err("App grew beyond the 8 GiB copying limit".into());
        }
        let mut input = fs::open_read_nofollow(source).map_err(|e| e.to_string())?;
        let mut output =
            fs::create_new_with_permissions(destination, Some(&metadata.permissions()))
                .map_err(|e| e.to_string())?;
        let copied = std::io::copy(
            &mut (&mut input).take(metadata.len().saturating_add(1)),
            &mut output,
        )
        .map_err(|e| e.to_string())?;
        if copied != metadata.len() {
            return Err("App changed during copying; inspect it again".into());
        }
        output.flush().map_err(|e| e.to_string())?;
        fs_sync::sync_file(&output).map_err(|e| format!("Cannot synchronize copied file: {e}"))?;
    } else {
        return Err("Unsupported app file type".into());
    }
    Ok(())
}
fn same_contents(a: &AppInfo, b: &AppInfo) -> bool {
    a.id == b.id
        && a.name == b.name
        && a.target == b.target
        && a.bytes == b.bytes
        && a.files == b.files
        && a.fingerprint == b.fingerprint
}
fn reconcile_after_rollback(
    reload: &mut impl FnMut() -> Result<usize, String>,
    cause: &str,
) -> String {
    match reload() {
        Ok(_) => format!("{cause}. Files restored and the previous catalog reloaded"),
        Err(error) => format!(
            "{cause}. Files restored, but catalog state is unconfirmed ({error}). Use Reload catalog when the service is available"
        ),
    }
}

pub fn install(
    root: &Path,
    inspected: &AppInfo,
    mut reload: impl FnMut() -> Result<usize, String>,
) -> Result<String, String> {
    let _lock = OperationLock::acquire(root)?;
    let current = inspect(&inspected.path)?;
    if !same_contents(&current, inspected) {
        return Err("The package changed after inspection; inspect it again".into());
    }
    let apps = installed(root)?;
    if apps.iter().any(|app| app.id == current.id) {
        return Err(format!(
            "{} is already installed. Remove it before installing another package with that ID",
            current.id
        ));
    }
    let destination = root.join(current.path.file_name().ok_or("Invalid source directory")?);
    absent(&destination)?;
    let staging = stage(root)?;
    let staged_app = staging.join("package");
    let prepared = (|| {
        copy_tree(&current.path, &staged_app, 0, &mut 0, &mut 0)?;
        let copied = inspect_contents(&staged_app)?;
        if !same_contents(&current, &copied) {
            return Err("The copied package differs from the inspected source".into());
        }
        absent(&destination)?;
        fs::rename(&staged_app, &destination).map_err(|e| format!("Cannot publish app: {e}"))
    })();
    if let Err(error) = prepared {
        let cleanup = fs::remove_dir_all(&staging);
        return Err(match cleanup {
            Ok(()) => error,
            Err(e) => format!(
                "{error}. Incomplete copy remains at {}: {e}",
                staging.display()
            ),
        });
    }
    if let Err(error) = reload() {
        if let Err(rollback) = fs::rename(&destination, &staged_app) {
            return Err(format!(
                "Catalog reload failed ({error}); rollback failed ({rollback}). App remains at {}. Catalog state is unconfirmed; use Reload catalog",
                destination.display()
            ));
        }
        let message =
            reconcile_after_rollback(&mut reload, &format!("Installation cancelled: {error}"));
        if let Err(cleanup) = fs::remove_dir_all(&staging) {
            return Err(format!(
                "{message}. Staged copy remains at {}: {cleanup}",
                staging.display()
            ));
        }
        return Err(message);
    }
    let _ = fs::remove_dir(&staging);
    Ok(format!(
        "Installed {}. Application catalog reloaded",
        current.name
    ))
}

pub fn remove(
    root: &Path,
    selected: &AppInfo,
    mut reload: impl FnMut() -> Result<usize, String>,
) -> Result<String, String> {
    let _lock = OperationLock::acquire(root)?;
    if selected.path.parent() != Some(root) {
        return Err(
            "Only a selected app directly inside the managed directory can be removed".into(),
        );
    }
    let current = inspect(&selected.path)?;
    if !same_contents(&current, selected) {
        return Err("The selected app changed; refresh the list before removing it".into());
    }
    let staging = stage(root)?;
    let removed_app = staging.join(
        selected
            .path
            .file_name()
            .ok_or("Invalid selected app path")?,
    );
    if let Err(error) = fs::rename(&selected.path, &removed_app) {
        let _ = fs::remove_dir(&staging);
        return Err(format!("Cannot remove app: {error}"));
    }
    if let Err(error) = reload() {
        if let Err(rollback) = absent(&selected.path)
            .and_then(|()| fs::rename(&removed_app, &selected.path).map_err(|e| e.to_string()))
        {
            return Err(format!(
                "Catalog reload failed ({error}); restore failed ({rollback}). Original files are retained at {}. Catalog state is unconfirmed",
                removed_app.display()
            ));
        }
        let _ = fs::remove_dir(&staging);
        return Err(reconcile_after_rollback(
            &mut reload,
            &format!("Removal cancelled: {error}"),
        ));
    }
    if let Err(error) = fs::remove_dir_all(&staging) {
        let recovery = if cfg!(target_os = "scarlet") {
            "App files are retained. Inspect and install the saved .app to restore it."
        } else {
            "Some files could not be cleaned up."
        };
        return Ok(format!(
            "Removed package {} from the managed directory and reloaded the catalog. {recovery} Location: {}. Cleanup: {error}",
            current.name,
            removed_app.display()
        ));
    }
    Ok(format!(
        "Removed package {} and reloaded the catalog. Running instances, personal data and legacy launcher entries are unchanged",
        current.name
    ))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
