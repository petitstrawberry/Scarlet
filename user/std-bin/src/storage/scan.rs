//! Bounded, read-only logical file sizes. A snapshot is not physical disk usage.
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Directory,
    File,
    Link,
    Special,
    Unknown,
}

#[derive(Debug)]
pub struct Node {
    pub path: PathBuf,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub kind: Kind,
    pub bytes: u64,
    pub files: usize,
    pub links: usize,
    pub excluded: usize,
    pub complete: bool,
    pub issue: Option<String>,
}

impl Node {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy()
            .into_owned()
    }
}

#[derive(Debug)]
pub struct Snapshot {
    pub nodes: Vec<Node>,
    pub stopped: Option<String>,
    pub errors: usize,
    pub elapsed: Duration,
}

#[derive(Clone, Copy)]
pub struct Limits {
    pub nodes: usize,
    pub depth: usize,
    pub path_bytes: usize,
    pub time: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            nodes: 50_000,
            depth: 64,
            path_bytes: 16 * 1024 * 1024,
            time: Duration::from_secs(15),
        }
    }
}

pub fn scan(path: &Path, cancelled: &AtomicBool, limits: Limits) -> Result<Snapshot, String> {
    scan_with(&System, path, cancelled, limits)
}

// Injectable filesystem operations let tests exercise unreadable directories
// and interrupted enumeration without relying on the test process's UID.
trait Source {
    fn metadata(&self, path: &Path) -> io::Result<(Kind, u64)>;
    fn entries(&self, path: &Path) -> io::Result<Box<dyn Iterator<Item = io::Result<PathBuf>>>>;
}
struct System;
impl Source for System {
    fn metadata(&self, path: &Path) -> io::Result<(Kind, u64)> {
        #[cfg(target_os = "scarlet")]
        let meta = scarlet_fs::symlink_metadata(path)?;
        #[cfg(not(target_os = "scarlet"))]
        let meta = fs::symlink_metadata(path)?;
        let kind = if meta.file_type().is_symlink() {
            Kind::Link
        } else if meta.is_dir() {
            Kind::Directory
        } else if meta.is_file() {
            Kind::File
        } else {
            Kind::Special
        };
        Ok((kind, meta.len()))
    }
    fn entries(&self, path: &Path) -> io::Result<Box<dyn Iterator<Item = io::Result<PathBuf>>>> {
        Ok(Box::new(
            fs::read_dir(path)?.map(|entry| entry.map(|entry| entry.path())),
        ))
    }
}

fn scan_with(
    source: &impl Source,
    path: &Path,
    cancelled: &AtomicBool,
    limits: Limits,
) -> Result<Snapshot, String> {
    let start = Instant::now();
    if cancelled.load(Ordering::Relaxed) {
        return Err("Scan cancelled before it started".into());
    }
    if !path.is_absolute() {
        return Err("Choose an absolute folder path".into());
    }
    let root_cost = path.as_os_str().len();
    if limits.nodes == 0 || root_cost > limits.path_bytes {
        return Err("Folder path exceeds the scan limits".into());
    }
    // Reject symlink ancestors too; choosing a link must not silently scan its target.
    // Path operations cannot provide an atomic filesystem snapshot. A folder
    // changed concurrently may disappear or produce a partial result.
    let mut prefix = PathBuf::new();
    for part in path.components() {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Scan cancelled before it started".into());
        }
        if matches!(part, Component::ParentDir) {
            return Err("Choose a folder without '..' components".into());
        }
        prefix.push(part.as_os_str());
        if source
            .metadata(&prefix)
            .map_err(|e| format!("{}: {e}", prefix.display()))?
            .0
            != Kind::Directory
        {
            return Err(format!(
                "{} is not a folder, or is a symbolic link",
                prefix.display()
            ));
        }
    }
    let mut walker = Walker {
        source,
        cancelled,
        limits,
        start,
        path_bytes: root_cost,
        entries: 1,
        snapshot: Snapshot {
            nodes: vec![node(path.to_path_buf(), None)],
            stopped: None,
            errors: 0,
            elapsed: Duration::ZERO,
        },
    };
    walker.visit(0, 0);
    walker.available(0);
    // Postorder aggregation avoids mutable aliasing and includes partial totals.
    for id in (0..walker.snapshot.nodes.len()).rev() {
        if let Some(parent) = walker.snapshot.nodes[id].parent {
            let child = &walker.snapshot.nodes[id];
            let (bytes, files, links, excluded, complete) = (
                child.bytes,
                child.files,
                child.links,
                child.excluded,
                child.complete,
            );
            let parent = &mut walker.snapshot.nodes[parent];
            if let Some(total) = parent.bytes.checked_add(bytes) {
                parent.bytes = total;
            } else {
                parent.bytes = u64::MAX;
                parent.complete = false;
                parent.issue =
                    Some("Logical byte total overflowed; the displayed value is capped".into());
            }
            parent.files += files;
            parent.links += links;
            parent.excluded += excluded;
            parent.complete &= complete;
        }
    }
    for id in 0..walker.snapshot.nodes.len() {
        let mut children = std::mem::take(&mut walker.snapshot.nodes[id].children);
        children.sort_by(|&left, &right| {
            let a = &walker.snapshot.nodes[left];
            let b = &walker.snapshot.nodes[right];
            b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path))
        });
        walker.snapshot.nodes[id].children = children;
    }
    walker.snapshot.elapsed = start.elapsed();
    Ok(walker.snapshot)
}

fn node(path: PathBuf, parent: Option<usize>) -> Node {
    Node {
        path,
        parent,
        children: Vec::new(),
        kind: Kind::Unknown,
        bytes: 0,
        files: 0,
        links: 0,
        excluded: 0,
        complete: true,
        issue: None,
    }
}

struct Walker<'a, S> {
    source: &'a S,
    cancelled: &'a AtomicBool,
    limits: Limits,
    start: Instant,
    path_bytes: usize,
    entries: usize,
    snapshot: Snapshot,
}
impl<S: Source> Walker<'_, S> {
    fn available(&mut self, id: usize) -> bool {
        let reason = if self.cancelled.load(Ordering::Relaxed) {
            Some("Scan cancelled".into())
        } else if self.start.elapsed() >= self.limits.time {
            Some(format!(
                "Time limit reached ({} seconds)",
                self.limits.time.as_secs()
            ))
        } else {
            self.snapshot.stopped.clone()
        };
        if let Some(reason) = reason {
            self.snapshot.stopped.get_or_insert(reason);
            self.snapshot.nodes[id].complete = false;
            false
        } else {
            true
        }
    }
    fn fail(&mut self, id: usize, message: String) {
        self.snapshot.errors += 1;
        let node = &mut self.snapshot.nodes[id];
        node.complete = false;
        // Errors can include arbitrarily long names. Bound retained diagnostics.
        node.issue
            .get_or_insert_with(|| message.chars().take(240).collect());
    }
    fn visit(&mut self, id: usize, depth: usize) {
        if !self.available(id) {
            return;
        }
        let path = self.snapshot.nodes[id].path.clone();
        let (kind, len) = match self.source.metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                self.fail(id, error.to_string());
                return;
            }
        };
        self.snapshot.nodes[id].kind = kind;
        match kind {
            Kind::File => {
                self.snapshot.nodes[id].bytes = len;
                self.snapshot.nodes[id].files = 1;
            }
            Kind::Link => self.snapshot.nodes[id].links = 1,
            Kind::Special | Kind::Unknown => self.snapshot.nodes[id].excluded = 1,
            Kind::Directory => {
                if depth >= self.limits.depth {
                    self.fail(
                        id,
                        format!("Depth limit reached ({} levels)", self.limits.depth),
                    );
                    return;
                }
                let entries = match self.source.entries(&path) {
                    Ok(entries) => entries,
                    Err(error) => {
                        self.fail(id, error.to_string());
                        return;
                    }
                };
                for entry in entries {
                    if !self.available(id) {
                        break;
                    }
                    if self.entries >= self.limits.nodes {
                        self.snapshot.stopped = Some(format!(
                            "Item limit reached ({} entries)",
                            self.limits.nodes
                        ));
                        self.snapshot.nodes[id].complete = false;
                        break;
                    }
                    self.entries += 1;
                    let path = match entry {
                        Ok(path) => path,
                        Err(error) => {
                            self.fail(id, error.to_string());
                            continue;
                        }
                    };
                    let cost = path.as_os_str().len();
                    let reason = if self.snapshot.nodes.len() >= self.limits.nodes {
                        Some(format!(
                            "Item limit reached ({} entries)",
                            self.limits.nodes
                        ))
                    } else if cost > self.limits.path_bytes.saturating_sub(self.path_bytes) {
                        Some("Path memory limit reached".into())
                    } else {
                        None
                    };
                    if let Some(reason) = reason {
                        self.snapshot.stopped = Some(reason);
                        self.snapshot.nodes[id].complete = false;
                        break;
                    }
                    self.path_bytes += cost;
                    let child = self.snapshot.nodes.len();
                    self.snapshot.nodes.push(node(path, Some(id)));
                    self.snapshot.nodes[id].children.push(child);
                    self.visit(child, depth + 1);
                }
            }
        }
    }
}

pub fn size(bytes: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut amount = bytes as f64;
    let mut unit = 0;
    while amount >= 1024.0 && unit + 1 < units.len() {
        amount /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{amount:.1} {}", units[unit])
    }
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
