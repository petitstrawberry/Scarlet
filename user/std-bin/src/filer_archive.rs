//! Bounded ZIP and TAR browsing/extraction. Extraction creates a new directory;
//! archive paths, links and special files never select an external destination.
use crate::filer_operations::native_fs as fs;
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::filer_operations::{sync_file, validate_name};

const MAX_ENTRIES: usize = 20_000;
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 64 * 1024;
const MAX_TIME: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub is_directory: bool,
    pub size: u64,
    pub executable: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    pub entries: Vec<Entry>,
    pub total_bytes: u64,
}
impl Index {
    /// Include implicit parent directories without duplicating explicit entries.
    pub fn children(&self, directory: &str) -> Vec<Entry> {
        let prefix = if directory.is_empty() {
            String::new()
        } else {
            format!("{directory}/")
        };
        let mut children = BTreeMap::new();
        for entry in &self.entries {
            let Some(rest) = entry.path.strip_prefix(&prefix) else {
                continue;
            };
            if rest.is_empty() {
                continue;
            }
            let (name, nested) = rest
                .split_once('/')
                .map_or((rest, false), |(name, _)| (name, true));
            let path = format!("{prefix}{name}");
            children.entry(path.clone()).or_insert_with(|| Entry {
                path,
                is_directory: nested || entry.is_directory,
                size: if nested { 0 } else { entry.size },
                executable: !nested && entry.executable,
            });
        }
        let mut result: Vec<_> = children.into_values().collect();
        result.sort_by(|a, b| {
            b.is_directory
                .cmp(&a.is_directory)
                .then_with(|| a.path.cmp(&b.path))
        });
        result
    }
}
#[derive(Clone, Copy)]
enum Format {
    Zip,
    Tar,
    GzipTar,
}
fn format(path: &Path) -> Option<Format> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if name.ends_with(".zip") {
        Some(Format::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(Format::GzipTar)
    } else if name.ends_with(".tar") {
        Some(Format::Tar)
    } else {
        None
    }
}
pub fn supported(path: &Path) -> bool {
    format(path).is_some()
}
pub fn suggested_folder(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Archive");
    let lowercase = name.to_ascii_lowercase();
    for suffix in [".tar.gz", ".tgz", ".zip", ".tar"] {
        if lowercase.ends_with(suffix) {
            return format!("{} extracted", &name[..name.len() - suffix.len()]);
        }
    }
    "Archive extracted".into()
}

struct Budget<'a> {
    cancelled: &'a AtomicBool,
    start: Instant,
    bytes: u64,
    headers: usize,
}
impl<'a> Budget<'a> {
    fn new(cancelled: &'a AtomicBool) -> Self {
        Self {
            cancelled,
            start: Instant::now(),
            bytes: 0,
            headers: 0,
        }
    }
    fn check(&self) -> io::Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err(io::Error::other("Archive operation cancelled"))
        } else if self.start.elapsed() > MAX_TIME {
            Err(io::Error::other("Archive operation exceeded two minutes"))
        } else {
            Ok(())
        }
    }
    fn header(&mut self) -> io::Result<()> {
        self.check()?;
        self.headers += 1;
        if self.headers > MAX_ENTRIES {
            return Err(io::Error::other("Archive exceeds 20,000 entries"));
        }
        Ok(())
    }
    fn add_bytes(&mut self, bytes: u64) -> io::Result<()> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| io::Error::other("Archive size overflow"))?;
        if bytes > MAX_FILE_BYTES || self.bytes > MAX_EXPANDED_BYTES {
            return Err(io::Error::other(
                "Archive exceeds the 256 MiB file or 1 GiB expanded-size limit",
            ));
        }
        Ok(())
    }
}
fn archive_file(path: &Path) -> io::Result<File> {
    let file = fs::open_read_nofollow(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(io::Error::other(
            "Select an archive file no larger than 512 MiB",
        ));
    }
    Ok(file)
}

pub fn inspect(path: &Path, cancelled: &AtomicBool) -> Result<Index, String> {
    let kind = format(path).ok_or("Supported archives: ZIP, TAR, TAR.GZ and TGZ")?;
    let mut file = archive_file(path).map_err(|e| e.to_string())?;
    scan(&mut file, kind, &mut Budget::new(cancelled), None).map_err(|e| e.to_string())
}

/// Revalidate before creating the destination and always use create_new for data.
/// Errors remove only the newly-created extraction tree, never an existing item.
pub fn extract(
    path: &Path,
    directory: &Path,
    name: &str,
    expected: &Index,
    cancelled: &AtomicBool,
) -> Result<PathBuf, String> {
    validate_name(name)?;
    let kind = format(path).ok_or("Unsupported archive format")?;
    let mut input = archive_file(path).map_err(|e| e.to_string())?;
    let mut budget = Budget::new(cancelled);
    let current = scan(&mut input, kind, &mut budget, None).map_err(|e| e.to_string())?;
    if &current != expected {
        return Err(
            "Archive changed since it was opened. Close and reopen it before extracting.".into(),
        );
    }
    budget.check().map_err(|e| e.to_string())?;
    let destination = directory.join(name);
    fs::create_dir(&destination).map_err(|e| {
        format!(
            "Cannot create {}: {e}. Choose a new folder name; existing items are never replaced.",
            destination.display()
        )
    })?;
    let result = (|| {
        input.seek(SeekFrom::Start(0))?;
        let actual = scan(
            &mut input,
            kind,
            &mut Budget {
                cancelled,
                start: budget.start,
                bytes: 0,
                headers: 0,
            },
            Some(&destination),
        )?;
        if actual != current {
            return Err(io::Error::other("Archive changed while extracting"));
        }
        let mut directories = BTreeSet::new();
        directories.insert(PathBuf::new());
        for entry in &actual.entries {
            let path = Path::new(&entry.path);
            let mut parent = if entry.is_directory {
                Some(path)
            } else {
                path.parent()
            };
            while let Some(directory) = parent {
                directories.insert(directory.to_path_buf());
                parent = directory.parent();
            }
        }
        for directory in directories.iter().rev() {
            sync_file(&File::open(destination.join(directory))?)?;
        }
        sync_file(&File::open(directory)?)?;
        Ok::<_, io::Error>(())
    })();
    match result {
        Ok(()) => Ok(destination),
        Err(error) => match fs::remove_dir_all(&destination) {
            Ok(()) => Err(format!(
                "Extraction stopped: {error}. The incomplete extraction was removed."
            )),
            Err(cleanup) => Err(format!(
                "Extraction stopped: {error}. Incomplete files remain at {}: {cleanup}",
                destination.display()
            )),
        },
    }
}

fn safe_path(name: &str, is_directory: bool) -> io::Result<Option<String>> {
    if name.len() > 4096 || name.contains(['\\', ':', '\0']) || name.chars().any(char::is_control) {
        return Err(io::Error::other(
            "Archive contains an unsupported or unsafe path",
        ));
    }
    let mut parts = Vec::new();
    for component in Path::new(name).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => {
                let part = part
                    .to_str()
                    .ok_or_else(|| io::Error::other("Archive path is not UTF-8"))?;
                if part.len() > 255 {
                    return Err(io::Error::other("Archive path component is too long"));
                }
                parts.push(part);
            }
            _ => {
                return Err(io::Error::other(
                    "Archive paths must stay inside the extraction folder",
                ));
            }
        }
    }
    if parts.len() > 64 {
        return Err(io::Error::other(
            "Archive is nested more than 64 folders deep",
        ));
    }
    if parts.is_empty() {
        return if is_directory {
            Ok(None)
        } else {
            Err(io::Error::other("Archive file has an empty name"))
        };
    }
    Ok(Some(parts.join("/")))
}

fn add_entry(
    index: &mut Index,
    seen: &mut BTreeMap<String, Option<bool>>,
    entry: Entry,
) -> io::Result<()> {
    match seen.get(&entry.path) {
        Some(Some(_)) => {
            return Err(io::Error::other(format!(
                "Archive contains duplicate path: {}",
                entry.path
            )));
        }
        Some(None) if !entry.is_directory => {
            return Err(io::Error::other(
                "Archive replaces a parent folder with a file",
            ));
        }
        _ => {}
    }
    let mut parent = Path::new(&entry.path).parent();
    while let Some(path) = parent {
        let name = path.to_str().unwrap_or_default();
        if !name.is_empty() {
            if seen.get(name) == Some(&Some(false)) {
                return Err(io::Error::other("Archive uses a file as a parent folder"));
            }
            seen.entry(name.into()).or_insert(None);
        }
        parent = path.parent();
    }
    seen.insert(entry.path.clone(), Some(entry.is_directory));
    if seen.len() > MAX_ENTRIES {
        return Err(io::Error::other(
            "Archive exceeds 20,000 files and folders including implicit parents",
        ));
    }
    index.total_bytes = index
        .total_bytes
        .checked_add(entry.size)
        .ok_or_else(|| io::Error::other("Archive size overflow"))?;
    index.entries.push(entry);
    Ok(())
}

fn ensure_directory(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let mut path = root.to_path_buf();
    if !fs::symlink_metadata(root)?.is_dir() {
        return Err(io::Error::other("Extraction folder changed"));
    }
    for component in relative.components() {
        let Component::Normal(part) = component else {
            continue;
        };
        path.push(part);
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&path)?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(io::Error::other("Extraction path is not a real folder"));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(path)
}
fn output_file(root: &Path, entry: &Entry) -> io::Result<Option<File>> {
    let path = Path::new(&entry.path);
    if entry.is_directory {
        ensure_directory(root, path)?;
        return Ok(None);
    }
    ensure_directory(root, path.parent().unwrap_or_else(|| Path::new("")))?;
    fs::create_new_with_owner_mode(
        root.join(path),
        if entry.executable { 0o700 } else { 0o600 },
    )
    .map(Some)
}
fn copy_exact(
    reader: &mut dyn Read,
    mut writer: Option<&mut File>,
    size: u64,
    budget: &Budget<'_>,
) -> io::Result<()> {
    let mut remaining = size;
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        budget.check()?;
        let count = remaining.min(buffer.len() as u64) as usize;
        reader.read_exact(&mut buffer[..count])?;
        if let Some(output) = &mut writer {
            output.write_all(&buffer[..count])?;
        }
        remaining -= count as u64;
    }
    if let Some(output) = writer {
        output.flush()?;
        sync_file(output)?;
    }
    Ok(())
}

// Check cancellation below decompression and ZIP parsing too: one decoder call
// may consume many empty gzip members or a large amount of ZIP metadata.
struct ArchiveInput<'a> {
    file: &'a mut File,
    cancelled: &'a AtomicBool,
    start: Instant,
    length: u64,
}
impl ArchiveInput<'_> {
    fn check(&self) -> io::Result<()> {
        Budget {
            cancelled: self.cancelled,
            start: self.start,
            bytes: 0,
            headers: 0,
        }
        .check()
    }
}
impl Read for ArchiveInput<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        let remaining = self.length.saturating_sub(self.file.stream_position()?);
        let count = buffer.len().min(64 * 1024).min(remaining as usize);
        self.file.read(&mut buffer[..count])
    }
}
impl Seek for ArchiveInput<'_> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.check()?;
        match from {
            SeekFrom::End(offset) => {
                let position = self
                    .length
                    .checked_add_signed(offset)
                    .ok_or_else(|| io::Error::other("Invalid archive seek"))?;
                self.file.seek(SeekFrom::Start(position))
            }
            _ => self.file.seek(from),
        }
    }
}
fn scan(
    file: &mut File,
    kind: Format,
    budget: &mut Budget<'_>,
    destination: Option<&Path>,
) -> io::Result<Index> {
    file.seek(SeekFrom::Start(0))?;
    let length = file.metadata()?.len();
    if length > MAX_ARCHIVE_BYTES {
        return Err(io::Error::other("Archive grew beyond 512 MiB"));
    }
    let mut input = ArchiveInput {
        file,
        cancelled: budget.cancelled,
        start: budget.start,
        length,
    };
    let mut index = match kind {
        Format::Zip => scan_zip(&mut input, budget, destination)?,
        Format::Tar => {
            let index = scan_tar(&mut input, budget, destination)?;
            check_tar_padding(&mut input, budget)?;
            index
        }
        Format::GzipTar => {
            let mut decoder = flate2::read::MultiGzDecoder::new(input);
            let index = scan_tar(&mut decoder, budget, destination)?;
            // Read to the end to validate gzip CRC/trailer, with bounded padding.
            check_tar_padding(&mut decoder, budget)?;
            index
        }
    };
    index.entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(index)
}

struct ZipDirectory {
    start: u64,
    bytes: Vec<u8>,
    local_headers: BTreeMap<u64, [u8; 30]>,
}
fn zip_word(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}
fn zip_dword(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
// Validate actual record boundaries, not just the untrusted EOCD size. The ZIP
// library otherwise ignores this size and can fall back to earlier EOCD records.
// ZIP64, split volumes, prepended-offset archives and ambiguous EOCDs are rejected.
fn check_zip_directory(input: &mut ArchiveInput<'_>) -> io::Result<ZipDirectory> {
    let length = input.length;
    let tail_len = length.min(65_557) as usize;
    let mut tail = vec![0u8; tail_len];
    input.seek(SeekFrom::End(-(tail_len as i64)))?;
    input.read_exact(&mut tail)?;
    let position = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| {
            tail[i..].starts_with(b"PK\x05\x06")
                && i + 22 + zip_word(&tail, i + 20) as usize == tail.len()
        })
        .ok_or_else(|| io::Error::other("ZIP end directory is missing or invalid"))?;
    let header = &tail[position..];
    let end = length - tail_len as u64 + position as u64;
    let count = zip_word(header, 10) as usize;
    let size = zip_dword(header, 12) as usize;
    let start = u64::from(zip_dword(header, 16));
    if zip_word(header, 4) != 0
        || zip_word(header, 6) != 0
        || zip_word(header, 8) as usize != count
        || count > MAX_ENTRIES
        || size > 16 * 1024 * 1024
        || start == u32::MAX as u64
        || start.checked_add(size as u64) != Some(end)
    {
        return Err(io::Error::other(
            "ZIP64, split volumes or invalid/oversized ZIP directories are not supported",
        ));
    }
    let mut bytes = vec![0; (length - start) as usize];
    input.seek(SeekFrom::Start(start))?;
    input.read_exact(&mut bytes)?;
    if &bytes[size..] != header {
        return Err(io::Error::other("ZIP end directory changed while reading"));
    }
    // Keep one unambiguous EOCD. This also excludes signatures inside comments
    // or names which the library could otherwise reinterpret after parse errors.
    if bytes
        .windows(4)
        .enumerate()
        .any(|(at, signature)| signature == b"PK\x05\x06" && at != size)
    {
        return Err(io::Error::other("ZIP has ambiguous end-directory records"));
    }
    let mut offset = 0usize;
    let mut local_headers = BTreeMap::new();
    for _ in 0..count {
        input.check()?;
        if offset + 46 > size || &bytes[offset..offset + 4] != b"PK\x01\x02" {
            return Err(io::Error::other(
                "ZIP directory record count or size is invalid",
            ));
        }
        let record = &bytes[offset..];
        let name = zip_word(record, 28) as usize;
        let extra = zip_word(record, 30) as usize;
        let comment = zip_word(record, 32) as usize;
        let next = offset + 46 + name + extra + comment;
        if next > size
            || zip_word(record, 34) != 0
            || zip_dword(record, 20) == u32::MAX
            || zip_dword(record, 24) == u32::MAX
            || u64::from(zip_dword(record, 42)) + 30 > start
        {
            return Err(io::Error::other(
                "ZIP directory points outside its bounded records",
            ));
        }
        let local_offset = u64::from(zip_dword(record, 42));
        input.seek(SeekFrom::Start(local_offset))?;
        let mut local = [0u8; 30];
        input.read_exact(&mut local)?;
        let variable_size = u32::from(zip_word(&local, 26)) + u32::from(zip_word(&local, 28));
        if &local[..4] != b"PK\x03\x04"
            || local_offset + 30 + u64::from(variable_size) + u64::from(zip_dword(record, 20))
                > start
        {
            return Err(io::Error::other(
                "ZIP local file extends into its directory",
            ));
        }
        // The metadata parser only needs local magic and variable-field length.
        // Supply a minimal safe header, so arbitrary CRC/data bytes cannot become
        // extra EOCD candidates during its backwards fallback search.
        let mut safe_local = [0u8; 30];
        safe_local[..4].copy_from_slice(b"PK\x03\x04");
        let name_size = variable_size.min(u16::MAX as u32) as u16;
        let extra_size = (variable_size - u32::from(name_size)) as u16;
        safe_local[26..28].copy_from_slice(&name_size.to_le_bytes());
        safe_local[28..30].copy_from_slice(&extra_size.to_le_bytes());
        local_headers.insert(local_offset, safe_local);
        let mut field = offset + 46 + name;
        let extra_end = field + extra;
        while field < extra_end {
            if field + 4 > extra_end {
                return Err(io::Error::other("Invalid ZIP extension field"));
            }
            let tag = zip_word(&bytes, field);
            let field_size = zip_word(&bytes, field + 2) as usize;
            field += 4 + field_size;
            if tag == 1 || field > extra_end {
                return Err(io::Error::other(
                    "ZIP64 or invalid ZIP extensions are unsupported",
                ));
            }
        }
        offset = next;
    }
    if offset != size {
        return Err(io::Error::other(
            "ZIP directory contains undeclared records or trailing bytes",
        ));
    }
    let mut previous_end = 0;
    for &offset in local_headers.keys() {
        if offset < previous_end {
            return Err(io::Error::other("ZIP local headers overlap"));
        }
        previous_end = offset + 30;
    }
    Ok(ZipDirectory {
        start,
        bytes,
        local_headers,
    })
}

// During metadata parsing, expose only minimal local headers before the directory. The
// parser can never fall back to an attacker-controlled EOCD inside file data.
// Serve the immutable verified metadata thereafter, so a concurrent file change
// cannot replace the checked count/size between validation and parser allocation.
struct ZipInput<'a, 'b> {
    input: &'a mut ArchiveInput<'b>,
    directory: ZipDirectory,
    position: u64,
    metadata_only: &'a Cell<bool>,
}
impl Read for ZipInput<'_, '_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.input.check()?;
        let remaining = self.input.length.saturating_sub(self.position);
        let mut count = buffer.len().min(64 * 1024).min(remaining as usize);
        if self.position < self.directory.start {
            count = count.min((self.directory.start - self.position) as usize);
            if self.metadata_only.get() {
                buffer[..count].fill(0);
                let end = self.position + count as u64;
                for (&at, header) in self
                    .directory
                    .local_headers
                    .range(self.position.saturating_sub(29)..end)
                {
                    let from = at.max(self.position);
                    let to = (at + 30).min(end);
                    buffer[(from - self.position) as usize..(to - self.position) as usize]
                        .copy_from_slice(&header[(from - at) as usize..(to - at) as usize]);
                }
            } else {
                self.input.seek(SeekFrom::Start(self.position))?;
                count = self.input.read(&mut buffer[..count])?;
            }
        } else if count != 0 {
            let at = (self.position - self.directory.start) as usize;
            buffer[..count].copy_from_slice(&self.directory.bytes[at..at + count]);
        }
        self.position += count as u64;
        Ok(count)
    }
}
impl Seek for ZipInput<'_, '_> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.input.check()?;
        self.position = match from {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::End(offset) => self.input.length.checked_add_signed(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
        }
        .ok_or_else(|| io::Error::other("Invalid ZIP seek"))?;
        Ok(self.position)
    }
}
fn scan_zip(
    input: &mut ArchiveInput<'_>,
    budget: &mut Budget<'_>,
    destination: Option<&Path>,
) -> io::Result<Index> {
    let directory = check_zip_directory(input)?;
    let metadata_only = Cell::new(true);
    let reader = ZipInput {
        input,
        directory,
        position: 0,
        metadata_only: &metadata_only,
    };
    let mut archive = zip::ZipArchive::new(reader).map_err(io::Error::other)?;
    metadata_only.set(false);
    if archive.len() > MAX_ENTRIES {
        return Err(io::Error::other("Archive exceeds 20,000 entries"));
    }
    let mut index = Index::default();
    let mut seen = BTreeMap::new();
    for number in 0..archive.len() {
        budget.header()?;
        let mut file = archive.by_index(number).map_err(io::Error::other)?;
        if file.encrypted()
            || file.is_symlink()
            || file
                .unix_mode()
                .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000 | 0o040000))
        {
            return Err(io::Error::other(
                "Encrypted entries, links and special files are not supported",
            ));
        }
        let directory = file.is_dir();
        let Some(path) = safe_path(file.name(), directory)? else {
            continue;
        };
        let entry = Entry {
            path,
            is_directory: directory,
            size: if directory { 0 } else { file.size() },
            executable: !directory && file.unix_mode().is_some_and(|mode| mode & 0o100 != 0),
        };
        budget.add_bytes(entry.size)?;
        if directory && file.size() != 0 {
            return Err(io::Error::other("Archive directory contains file data"));
        }
        add_entry(&mut index, &mut seen, entry.clone())?;
        if let Some(root) = destination {
            let mut output = output_file(root, &entry)?;
            copy_exact(&mut file, output.as_mut(), entry.size, budget)?;
            // Reading past the advertised size checks the decompressor's CRC.
            let mut end = [0u8; 1];
            if file.read(&mut end)? != 0 {
                return Err(io::Error::other("ZIP entry exceeds its declared size"));
            }
        }
    }
    Ok(index)
}

fn tar_string(bytes: &[u8]) -> io::Result<String> {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    if bytes[end..].iter().any(|b| *b != 0) {
        return Err(io::Error::other("Invalid TAR string padding"));
    }
    String::from_utf8(bytes[..end].to_vec()).map_err(io::Error::other)
}
fn tar_number(bytes: &[u8]) -> io::Result<u64> {
    if bytes.first().is_some_and(|b| b & 0x80 != 0) {
        // GNU base-256 positive integers.
        if bytes[0] & 0x40 != 0 {
            return Err(io::Error::other("Negative TAR size"));
        }
        let mut value = u64::from(bytes[0] & 0x7f);
        for byte in &bytes[1..] {
            value = value
                .checked_mul(256)
                .and_then(|v| v.checked_add(u64::from(*byte)))
                .ok_or_else(|| io::Error::other("TAR number overflow"))?;
        }
        return Ok(value);
    }
    let text = std::str::from_utf8(bytes)
        .map_err(io::Error::other)?
        .trim_matches(['\0', ' ']);
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 8).map_err(io::Error::other)
}
#[derive(Default)]
struct Pax {
    path: Option<String>,
    size: Option<u64>,
}
fn parse_pax(data: &[u8], global: bool) -> io::Result<Pax> {
    let mut result = Pax::default();
    let mut offset = 0;
    while offset < data.len() {
        let rest = &data[offset..];
        let space = rest
            .iter()
            .position(|b| *b == b' ')
            .ok_or_else(|| io::Error::other("Invalid PAX record"))?;
        let length = std::str::from_utf8(&rest[..space])
            .map_err(io::Error::other)?
            .parse::<usize>()
            .map_err(io::Error::other)?;
        if length <= space + 2 || length > rest.len() || rest[length - 1] != b'\n' {
            return Err(io::Error::other("Invalid PAX record length"));
        }
        let record = std::str::from_utf8(&rest[space + 1..length - 1]).map_err(io::Error::other)?;
        let (key, value) = record
            .split_once('=')
            .ok_or_else(|| io::Error::other("Invalid PAX key"))?;
        match key {
            "path" if !global => {
                if result.path.replace(value.into()).is_some() {
                    return Err(io::Error::other("Duplicate PAX path"));
                }
            }
            "size" if !global => {
                if result
                    .size
                    .replace(value.parse::<u64>().map_err(io::Error::other)?)
                    .is_some()
                {
                    return Err(io::Error::other("Duplicate PAX size"));
                }
            }
            "path" | "size" | "linkpath" => {
                return Err(io::Error::other(
                    "Global paths, global sizes and links are unsupported",
                ));
            }
            key if key.starts_with("GNU.sparse") || key.starts_with("SCHILY.") => {
                return Err(io::Error::other(
                    "Sparse or special TAR extensions are unsupported",
                ));
            }
            _ => {} // Ordinary timestamps, owner names and comments do not affect extraction.
        }
        offset += length;
    }
    Ok(result)
}
fn scan_tar(
    reader: &mut dyn Read,
    budget: &mut Budget<'_>,
    destination: Option<&Path>,
) -> io::Result<Index> {
    let mut index = Index::default();
    let mut seen = BTreeMap::new();
    let mut pending = Pax::default();
    loop {
        budget.check()?;
        let mut header = [0u8; 512];
        reader.read_exact(&mut header)?;
        if header.iter().all(|b| *b == 0) {
            let mut second = [0u8; 512];
            reader.read_exact(&mut second)?;
            if second.iter().any(|b| *b != 0) || pending.path.is_some() || pending.size.is_some() {
                return Err(io::Error::other("Invalid TAR terminator"));
            }
            break;
        }
        budget.header()?;
        let checksum = tar_number(&header[148..156])?;
        let sum: u64 = header
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if (148..156).contains(&i) {
                    u64::from(b' ')
                } else {
                    u64::from(*b)
                }
            })
            .sum();
        if checksum != sum {
            return Err(io::Error::other("TAR header checksum mismatch"));
        }
        let kind = header[156];
        let header_size = tar_number(&header[124..136])?;
        if matches!(kind, b'x' | b'g' | b'L') {
            if header_size > MAX_METADATA_BYTES {
                return Err(io::Error::other("TAR extension header is too large"));
            }
            let mut data = vec![0u8; header_size as usize];
            reader.read_exact(&mut data)?;
            discard_padding(reader, header_size, budget)?;
            if kind == b'L' {
                if pending.path.is_some() {
                    return Err(io::Error::other("Conflicting TAR path extensions"));
                }
                pending.path = Some(tar_string(&data)?);
            } else {
                let pax = parse_pax(&data, kind == b'g')?;
                if kind == b'x' {
                    if pending.path.is_some() || pending.size.is_some() {
                        return Err(io::Error::other("Conflicting TAR extensions"));
                    }
                    pending = pax;
                }
            }
            continue;
        }
        if !matches!(kind, 0 | b'0' | b'5') {
            return Err(io::Error::other(
                "TAR links, devices and special entries are unsupported",
            ));
        }
        let mut name = tar_string(&header[..100])?;
        if &header[257..263] == b"ustar\0" {
            let prefix = tar_string(&header[345..500])?;
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
        } else if &header[257..263] != b"ustar " && header[257..263].iter().any(|b| *b != 0) {
            return Err(io::Error::other("Unsupported TAR header format"));
        }
        if let Some(path) = pending.path.take() {
            name = path;
        }
        let directory = kind == b'5' || name.ends_with('/');
        let size = pending.size.take().unwrap_or(header_size);
        if directory && size != 0 {
            return Err(io::Error::other("TAR directory contains data"));
        }
        budget.add_bytes(size)?;
        if let Some(path) = safe_path(&name, directory)? {
            let entry = Entry {
                path,
                is_directory: directory,
                size,
                executable: !directory && tar_number(&header[100..108])? & 0o100 != 0,
            };
            add_entry(&mut index, &mut seen, entry.clone())?;
            let mut output = destination
                .map(|root| output_file(root, &entry))
                .transpose()?
                .flatten();
            copy_exact(reader, output.as_mut(), size, budget)?;
        } else {
            copy_exact(reader, None, size, budget)?;
        }
        discard_padding(reader, size, budget)?;
    }
    Ok(index)
}
fn discard_padding(reader: &mut dyn Read, size: u64, budget: &Budget<'_>) -> io::Result<()> {
    copy_exact(reader, None, (512 - size % 512) % 512, budget)
}

fn check_tar_padding(reader: &mut dyn Read, budget: &Budget<'_>) -> io::Result<()> {
    let mut padding = [0u8; 4096];
    let mut trailing = 0usize;
    loop {
        budget.check()?;
        let count = reader.read(&mut padding)?;
        if count == 0 {
            break;
        }
        trailing += count;
        if trailing > 1024 * 1024 || padding[..count].iter().any(|b| *b != 0) {
            return Err(io::Error::other("Unexpected data after TAR terminator"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicUsize;
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "scarlet-archive-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn idle() -> AtomicBool {
        AtomicBool::new(false)
    }
    fn tar_header(name: &str, kind: u8, size: u64) -> [u8; 512] {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..108].copy_from_slice(b"0000644\0");
        let size = format!("{size:011o}\0");
        header[124..136].copy_from_slice(size.as_bytes());
        header[156] = kind;
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].fill(b' ');
        let checksum: u64 = header.iter().map(|b| u64::from(*b)).sum();
        header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
        header
    }
    fn tar_bytes(entries: &[(&str, u8, &[u8])]) -> Vec<u8> {
        let mut result = Vec::new();
        for (name, kind, data) in entries {
            result.extend_from_slice(&tar_header(name, *kind, data.len() as u64));
            result.extend_from_slice(data);
            result.resize(result.len().div_ceil(512) * 512, 0);
        }
        result.resize(result.len() + 1024, 0);
        result
    }
    fn write_zip(path: &Path, name: &str, data: &[u8], deflated: bool) {
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default().compression_method(if deflated {
            zip::CompressionMethod::Deflated
        } else {
            zip::CompressionMethod::Stored
        });
        writer.start_file(name, options).unwrap();
        writer.write_all(data).unwrap();
        writer.finish().unwrap();
    }
    fn pax(key: &str, value: &str) -> Vec<u8> {
        let record = format!(" {key}={value}\n");
        let mut length = record.len() + 1;
        loop {
            let next = record.len() + length.to_string().len();
            if next == length {
                break;
            }
            length = next;
        }
        format!("{length}{record}").into_bytes()
    }
    #[test]
    fn zip_and_tar_browse_implicit_folders_and_extract_unicode_contents() {
        let fixture = Fixture::new();
        for extension in ["zip", "tar", "tar.gz"] {
            let path = fixture.0.join(format!("sample.{extension}"));
            let bytes = "こんにちは".as_bytes();
            if extension == "zip" {
                write_zip(&path, "nested/メモ.txt", bytes, true);
            } else {
                let tar = tar_bytes(&[("nested/", b'5', b""), ("nested/メモ.txt", b'0', bytes)]);
                if extension == "tar.gz" {
                    let mut writer = flate2::write::GzEncoder::new(
                        File::create(&path).unwrap(),
                        flate2::Compression::default(),
                    );
                    writer.write_all(&tar).unwrap();
                    writer.finish().unwrap();
                } else {
                    fs::write(&path, tar).unwrap();
                }
            }
            let index = inspect(&path, &idle()).unwrap();
            assert_eq!(index.total_bytes, bytes.len() as u64);
            assert_eq!(index.children("")[0].path, "nested");
            assert!(index.children("")[0].is_directory);
            assert_eq!(index.children("nested")[0].path, "nested/メモ.txt");
            let output = extract(
                &path,
                &fixture.0,
                &format!("out-{extension}"),
                &index,
                &idle(),
            )
            .unwrap();
            assert_eq!(fs::read(output.join("nested/メモ.txt")).unwrap(), bytes);
        }
    }
    #[test]
    fn rejects_traversal_absolute_windows_paths_and_links_before_creating_output() {
        let fixture = Fixture::new();
        for name in [
            "../outside",
            "/outside",
            "a/../../outside",
            "C:/outside",
            "a\\outside",
            "a\nname",
        ] {
            let path = fixture.0.join("unsafe.tar");
            fs::write(&path, tar_bytes(&[(name, b'0', b"x")])).unwrap();
            assert!(inspect(&path, &idle()).is_err(), "accepted {name}");
            let zip = fixture.0.join("unsafe.zip");
            write_zip(&zip, name, b"x", false);
            assert!(inspect(&zip, &idle()).is_err(), "accepted ZIP {name}");
        }
        for kind in [b'1', b'2', b'3', b'4', b'6', b'S'] {
            let path = fixture.0.join("link.tar");
            fs::write(&path, tar_bytes(&[("link", kind, b"")])).unwrap();
            assert!(inspect(&path, &idle()).is_err());
        }
        let path = fixture.0.join("link.zip");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        zip.add_symlink("link", "/outside", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.finish().unwrap();
        assert!(inspect(&path, &idle()).is_err());
    }
    #[test]
    fn rejects_duplicate_paths_and_file_parent_conflicts() {
        let fixture = Fixture::new();
        let path = fixture.0.join("duplicate.tar");
        for entries in [
            vec![
                ("same", b'0', b"1".as_slice()),
                ("./same", b'0', b"2".as_slice()),
            ],
            vec![("a", b'0', b"1".as_slice()), ("a/b", b'0', b"2".as_slice())],
            vec![("a/b", b'0', b"1".as_slice()), ("a", b'0', b"2".as_slice())],
        ] {
            fs::write(&path, tar_bytes(&entries)).unwrap();
            assert!(inspect(&path, &idle()).is_err());
        }
    }
    #[test]
    fn existing_destination_and_changed_archive_are_preserved() {
        let fixture = Fixture::new();
        let path = fixture.0.join("data.zip");
        write_zip(&path, "file", b"original", true);
        let index = inspect(&path, &idle()).unwrap();
        fs::create_dir(fixture.0.join("existing")).unwrap();
        fs::write(fixture.0.join("existing/keep"), b"keep").unwrap();
        assert!(extract(&path, &fixture.0, "existing", &index, &idle()).is_err());
        assert_eq!(fs::read(fixture.0.join("existing/keep")).unwrap(), b"keep");
        write_zip(&path, "changed", b"changed", true);
        assert!(extract(&path, &fixture.0, "new", &index, &idle()).is_err());
        assert!(!fixture.0.join("new").exists());
    }
    #[test]
    fn corrupted_zip_payload_removes_incomplete_new_extraction() {
        let fixture = Fixture::new();
        let path = fixture.0.join("data.zip");
        let payload = b"unique payload bytes";
        write_zip(&path, "file", payload, false);
        let index = inspect(&path, &idle()).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        let offset = bytes
            .windows(payload.len())
            .position(|w| w == payload)
            .unwrap();
        bytes[offset] ^= 1;
        fs::write(&path, bytes).unwrap();
        let error = extract(&path, &fixture.0, "new", &index, &idle()).unwrap_err();
        assert!(error.contains("Extraction stopped"));
        assert!(!fixture.0.join("new").exists());
    }
    #[test]
    fn invalid_checksums_truncation_and_size_bombs_are_rejected() {
        let fixture = Fixture::new();
        let path = fixture.0.join("bad.tar");
        let mut bytes = tar_bytes(&[("data", b'0', b"ok")]);
        bytes[0] ^= 1;
        fs::write(&path, &bytes).unwrap();
        assert!(inspect(&path, &idle()).unwrap_err().contains("checksum"));
        fs::write(&path, &bytes[..100]).unwrap();
        assert!(inspect(&path, &idle()).is_err());
        fs::write(&path, tar_header("huge", b'0', MAX_FILE_BYTES + 1)).unwrap();
        assert!(inspect(&path, &idle()).unwrap_err().contains("limit"));
        fs::write(&path, tar_bytes(&[("data", b'0', b"ok")])).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        bytes.push(b'x');
        fs::write(&path, bytes).unwrap();
        assert!(inspect(&path, &idle()).is_err());
    }
    #[test]
    fn accepts_pax_and_gnu_long_names_but_rejects_malicious_extensions() {
        let fixture = Fixture::new();
        let path = fixture.0.join("long.tar");
        let name = format!("nested/{}.txt", "x".repeat(120));
        let extended_path = pax("path", &name);
        fs::write(
            &path,
            tar_bytes(&[("pax", b'x', &extended_path), ("short", b'0', b"data")]),
        )
        .unwrap();
        assert_eq!(inspect(&path, &idle()).unwrap().entries[0].path, name);
        let gnu = format!("{name}\0");
        fs::write(
            &path,
            tar_bytes(&[
                ("././@LongLink", b'L', gnu.as_bytes()),
                ("short", b'0', b"data"),
            ]),
        )
        .unwrap();
        assert_eq!(inspect(&path, &idle()).unwrap().entries[0].path, name);
        let bad = pax("path", "../../escape");
        fs::write(
            &path,
            tar_bytes(&[("pax", b'x', &bad), ("short", b'0', b"data")]),
        )
        .unwrap();
        assert!(inspect(&path, &idle()).is_err());
    }
    #[test]
    fn gzip_trailer_corruption_and_cancellation_are_reported() {
        let fixture = Fixture::new();
        let path = fixture.0.join("data.tgz");
        let tar = tar_bytes(&[("data", b'0', b"ok")]);
        let mut writer = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        writer.write_all(&tar).unwrap();
        let mut bytes = writer.finish().unwrap();
        let end = bytes.len() - 1;
        bytes[end] ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(inspect(&path, &idle()).is_err());
        let path = fixture.0.join("data.zip");
        write_zip(&path, "file", b"ok", true);
        let index = inspect(&path, &idle()).unwrap();
        let cancelled = AtomicBool::new(true);
        assert!(inspect(&path, &cancelled).is_err());
        assert!(extract(&path, &fixture.0, "cancelled", &index, &cancelled).is_err());
        assert!(!fixture.0.join("cancelled").exists());
    }
    #[test]
    fn zip_directory_actual_size_and_count_must_match_before_parser_allocation() {
        let fixture = Fixture::new();
        let path = fixture.0.join("large-directory.zip");
        // 270 legal-size comments exceed the 16 MiB directory cap. The former
        // EOCD-only check accepted this archive when its size field claimed zero.
        let count = 270u16;
        let mut bytes = Vec::new();
        let mut offsets = Vec::new();
        for number in 0..count {
            offsets.push(bytes.len() as u32);
            let name = format!("file{number}");
            let mut local = [0u8; 30];
            local[..4].copy_from_slice(b"PK\x03\x04");
            local[26..28].copy_from_slice(&(name.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&local);
            bytes.extend_from_slice(name.as_bytes());
        }
        let start = bytes.len() as u32;
        for number in 0..count {
            let name = format!("file{number}");
            let mut record = [0u8; 46];
            record[..4].copy_from_slice(b"PK\x01\x02");
            record[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
            record[32..34].copy_from_slice(&u16::MAX.to_le_bytes());
            record[42..46].copy_from_slice(&offsets[number as usize].to_le_bytes());
            bytes.extend_from_slice(&record);
            bytes.extend_from_slice(name.as_bytes());
            bytes.resize(bytes.len() + u16::MAX as usize, b'x');
        }
        assert!(bytes.len() - start as usize > 16 * 1024 * 1024);
        let mut end = [0u8; 22];
        end[..4].copy_from_slice(b"PK\x05\x06");
        end[8..10].copy_from_slice(&count.to_le_bytes());
        end[10..12].copy_from_slice(&count.to_le_bytes());
        end[16..20].copy_from_slice(&start.to_le_bytes());
        bytes.extend_from_slice(&end); // deliberately forged central size zero
        fs::write(&path, &bytes).unwrap();
        assert!(inspect(&path, &idle()).unwrap_err().contains("directories"));
        // A matching small byte range with an incorrect record count also fails.
        write_zip(&path, "file", b"", false);
        let mut bytes = fs::read(&path).unwrap();
        let end = bytes.len() - 22;
        bytes[end + 8..end + 12].fill(0);
        fs::write(&path, &bytes).unwrap();
        assert!(inspect(&path, &idle()).is_err());
    }
    #[test]
    fn zip_parser_cannot_fall_back_to_eocd_in_file_payload() {
        let fixture = Fixture::new();
        let path = fixture.0.join("fallback.zip");
        // A valid empty EOCD inside stored data would be an accepted fallback
        // if the final directory fails library parsing. It must stay invisible.
        let mut fake = [0u8; 22];
        fake[..4].copy_from_slice(b"PK\x05\x06");
        write_zip(&path, "payload", &fake, false);
        let mut bytes = fs::read(&path).unwrap();
        let end = bytes.len() - 22;
        let directory = zip_dword(&bytes, end + 16) as usize;
        // AES method without its required extra field causes a metadata parse
        // error in ZipArchive::new, exercising its earlier-EOCD fallback path.
        bytes[directory + 10..directory + 12].copy_from_slice(&99u16.to_le_bytes());
        fs::write(&path, bytes).unwrap();
        assert!(inspect(&path, &idle()).is_err());
    }
    #[test]
    fn zip_metadata_snapshot_cannot_change_after_validation() {
        let fixture = Fixture::new();
        let path = fixture.0.join("snapshot.zip");
        write_zip(&path, "file", b"ok", false);
        let mut file = archive_file(&path).unwrap();
        let cancelled = idle();
        let mut input = ArchiveInput {
            length: file.metadata().unwrap().len(),
            file: &mut file,
            cancelled: &cancelled,
            start: Instant::now(),
        };
        let directory = check_zip_directory(&mut input).unwrap();
        let mut changed = fs::read(&path).unwrap();
        let end = changed.len() - 22;
        changed[end + 8..end + 12].fill(255);
        fs::write(&path, changed).unwrap();
        let metadata_only = Cell::new(true);
        let reader = ZipInput {
            input: &mut input,
            directory,
            position: 0,
            metadata_only: &metadata_only,
        };
        assert_eq!(zip::ZipArchive::new(reader).unwrap().len(), 1);
    }
    #[test]
    fn raw_input_checks_cancellation_for_decoder_internal_reads() {
        let fixture = Fixture::new();
        let path = fixture.0.join("members.gz");
        let member = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default())
            .finish()
            .unwrap();
        fs::write(&path, member.repeat(100)).unwrap();
        let mut file = File::open(&path).unwrap();
        let cancelled = idle();
        let input = ArchiveInput {
            length: file.metadata().unwrap().len(),
            file: &mut file,
            cancelled: &cancelled,
            start: Instant::now(),
        };
        let mut decoder = flate2::read::MultiGzDecoder::new(input);
        cancelled.store(true, Ordering::Relaxed);
        let error = decoder.read(&mut [0; 1]).unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    }
    #[cfg(unix)]
    #[test]
    fn executable_files_keep_only_owner_access_at_creation() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        for extension in ["zip", "tar"] {
            let path = fixture.0.join(format!("app.{extension}"));
            if extension == "zip" {
                let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
                writer
                    .start_file(
                        "Sample.app/bin/run",
                        zip::write::SimpleFileOptions::default().unix_permissions(0o4777),
                    )
                    .unwrap();
                writer.write_all(b"program").unwrap();
                writer.finish().unwrap();
            } else {
                let mut bytes = tar_bytes(&[("Sample.app/bin/run", b'0', b"program")]);
                bytes[100..108].copy_from_slice(b"0004777\0");
                bytes[148..156].fill(b' ');
                let checksum: u64 = bytes[..512].iter().map(|b| u64::from(*b)).sum();
                bytes[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
                fs::write(&path, bytes).unwrap();
            }
            let index = inspect(&path, &idle()).unwrap();
            assert!(index.entries[0].executable);
            let output = extract(&path, &fixture.0, extension, &index, &idle()).unwrap();
            assert_eq!(
                fs::metadata(output.join("Sample.app/bin/run"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o700
            );
        }
    }
    #[test]
    fn implicit_directories_count_toward_item_budget() {
        let mut index = Index::default();
        let mut seen = BTreeMap::new();
        for number in 0..MAX_ENTRIES / 2 {
            add_entry(
                &mut index,
                &mut seen,
                Entry {
                    path: format!("d{number}/f"),
                    is_directory: false,
                    size: 0,
                    executable: false,
                },
            )
            .unwrap();
        }
        assert!(
            add_entry(
                &mut index,
                &mut seen,
                Entry {
                    path: "extra".into(),
                    is_directory: false,
                    size: 0,
                    executable: false,
                }
            )
            .is_err()
        );
    }
}
