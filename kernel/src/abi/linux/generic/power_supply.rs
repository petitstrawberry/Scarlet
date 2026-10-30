//! Read-only Linux power_supply representation of native telemetry. Missing
//! measurements stay absent; no charger, suspend or wake authority is exposed.
use crate::{
    device::power_supply::{self, ChargeState, SupplyKind},
    fs::{
        FileMetadata, FilePermission, FileSystemDriver, FileSystemError, FileSystemErrorKind as K,
        FileSystemType, FileType, SeekFrom,
        vfs_v2::core::{DirectoryEntryInternal, FileSystemId, FileSystemOperations, VfsNode},
    },
    object::capability::{
        ControlOps, FileObject, MemoryMappingOps, ReadyInterest, SelectWaitOutcome, Selectable,
        StreamOps,
    },
    sync::{IrqRwSpinLock, IrqSpinLock},
};
use alloc::{
    boxed::Box,
    collections::BTreeMap,
    format,
    string::String,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::any::Any;
use scarlet_abi::power_supply::PowerSupplySnapshot;
const ATTRIBUTES: [(&str, u8); 10] = [
    ("type", 1),
    ("present", 2),
    ("online", 3),
    ("status", 4),
    ("capacity", 5),
    ("voltage_now", 6),
    ("current_now", 7),
    ("temp", 8),
    ("input_current_limit", 9),
    ("uevent", 10),
];
fn error(kind: K) -> FileSystemError {
    FileSystemError::new(kind, "native power supply view")
}
fn value(snapshot: &PowerSupplySnapshot, field: u8) -> Option<String> {
    let kind = match snapshot.kind {
        SupplyKind::Unknown => "Unknown",
        SupplyKind::Battery => "Battery",
        SupplyKind::Mains => "Mains",
        SupplyKind::Usb => "USB",
        SupplyKind::Ups => "UPS",
    };
    if field == 1 {
        return Some(format!("{kind}\n"));
    }
    if field == 10 {
        let mut text = format!(
            "POWER_SUPPLY_NAME=scarlet{}\nPOWER_SUPPLY_TYPE={kind}\n",
            snapshot.id
        );
        for (field, key) in [
            (2, "PRESENT"),
            (3, "ONLINE"),
            (4, "STATUS"),
            (5, "CAPACITY"),
            (6, "VOLTAGE_NOW"),
            (7, "CURRENT_NOW"),
            (8, "TEMP"),
            (9, "INPUT_CURRENT_LIMIT"),
        ] {
            if let Some(value) = value(snapshot, field) {
                text.push_str(&format!("POWER_SUPPLY_{key}={value}"));
            }
        }
        return Some(text);
    }
    if snapshot.read_failed {
        return None;
    }
    let state = snapshot.state;
    match field {
        2 => state.present.map(|value| format!("{}\n", u8::from(value))),
        3 => state.online.map(|value| format!("{}\n", u8::from(value))),
        4 => state.charge_state.map(|value| {
            format!(
                "{}\n",
                match value {
                    ChargeState::Charging => "Charging",
                    ChargeState::Discharging => "Discharging",
                    ChargeState::NotCharging => "Not charging",
                    ChargeState::Full => "Full",
                }
            )
        }),
        5 => state
            .capacity_permille
            .filter(|value| *value <= 1000)
            .map(|value| format!("{}\n", value / 10)),
        6 => state.voltage_uv.map(|value| format!("{value}\n")),
        7 => state.current_ua.map(|value| format!("{value}\n")),
        8 => state
            .temperature_mc
            .map(|value| format!("{}\n", value / 100)),
        9 => state
            .input_current_limit_ua
            .map(|value| format!("{value}\n")),
        _ => None,
    }
}
struct Node {
    id: u64,
    supply: Option<u32>,
    field: u8,
    fs: Weak<PowerSupplyFs>,
}
impl VfsNode for Node {
    fn id(&self) -> u64 {
        self.id
    }
    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.fs
            .upgrade()
            .map(|fs| Arc::downgrade(&(fs as Arc<dyn FileSystemOperations>)))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn shield_labels(&self) -> Option<Arc<crate::shield::file::Labels>> {
        let fs = self.fs.upgrade()?;
        let mut labels = fs.labels.write();
        Some(
            labels
                .entry(self.id)
                .or_insert_with(crate::shield::file::Labels::new)
                .clone(),
        )
    }
    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        if self
            .supply
            .is_some_and(|id| id as usize >= power_supply::count())
        {
            return Err(error(K::NotFound));
        }
        Ok(FileMetadata {
            file_type: if self.field == 0 {
                FileType::Directory
            } else {
                FileType::RegularFile
            },
            size: 0,
            permissions: FilePermission {
                read: true,
                write: false,
                execute: self.field == 0,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: self.id,
            link_count: 1,
        })
    }
}
struct PowerSupplyFs {
    id: FileSystemId,
    root: Arc<Node>,
    labels: IrqRwSpinLock<BTreeMap<u64, Arc<crate::shield::file::Labels>>>,
}
impl PowerSupplyFs {
    fn new() -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            id: FileSystemId::new(),
            root: Arc::new(Node {
                id: 1,
                supply: None,
                field: 0,
                fs: weak.clone(),
            }),
            labels: IrqRwSpinLock::new(BTreeMap::new()),
        })
    }
    fn node(&self, supply: Option<u32>, field: u8) -> Arc<Node> {
        Arc::new(Node {
            id: supply.map_or(1, |id| 16 + id as u64 * 32 + field as u64),
            supply,
            field,
            fs: self.root.fs.clone(),
        })
    }
    fn own<'a>(&self, node: &'a Arc<dyn VfsNode>) -> Result<&'a Node, FileSystemError> {
        let node = node
            .as_any()
            .downcast_ref::<Node>()
            .ok_or_else(|| error(K::InvalidOperation))?;
        if !node
            .fs
            .upgrade()
            .is_some_and(|fs| core::ptr::eq(fs.as_ref(), self))
        {
            return Err(error(K::InvalidOperation));
        }
        node.metadata()?;
        Ok(node)
    }
    fn children(&self, node: &Node) -> Result<Vec<(String, Arc<Node>)>, FileSystemError> {
        if node.field != 0 {
            return Err(error(K::NotADirectory));
        }
        if let Some(id) = node.supply {
            let snapshot = power_supply::snapshot(id).map_err(|_| error(K::NotFound))?;
            Ok(ATTRIBUTES
                .iter()
                .filter(|(_, field)| value(&snapshot, *field).is_some())
                .map(|(name, field)| (String::from(*name), self.node(Some(id), *field)))
                .collect())
        } else {
            Ok((0..power_supply::count())
                .map(|id| (format!("scarlet{id}"), self.node(Some(id as u32), 0)))
                .collect())
        }
    }
}
impl FileSystemOperations for PowerSupplyFs {
    fn fs_id(&self) -> FileSystemId {
        self.id
    }
    fn name(&self) -> &str {
        "linux-power-supply"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn root_node(&self) -> Arc<dyn VfsNode> {
        self.root.clone()
    }
    fn supports_retained_node_identity(&self) -> bool {
        true
    }
    fn lookup(
        &self,
        parent: &Arc<dyn VfsNode>,
        name: &String,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        let node = self.own(parent)?;
        if node.field != 0 {
            return Err(error(K::NotADirectory));
        }
        if name == "." {
            return Ok(self.node(node.supply, 0));
        }
        if name == ".." {
            return Ok(self.root.clone());
        }
        self.children(node)?
            .into_iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, node)| node as Arc<dyn VfsNode>)
            .ok_or_else(|| error(K::NotFound))
    }
    fn create(
        &self,
        _: &Arc<dyn VfsNode>,
        _: &String,
        _: FileType,
        _: u32,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        Err(error(K::ReadOnly))
    }
    fn remove(&self, _: &Arc<dyn VfsNode>, _: &String) -> Result<(), FileSystemError> {
        Err(error(K::ReadOnly))
    }
    fn readdir(
        &self,
        node: &Arc<dyn VfsNode>,
    ) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        self.children(self.own(node)?)?
            .into_iter()
            .map(|(name, node)| {
                Ok(DirectoryEntryInternal {
                    name,
                    file_type: node.metadata()?.file_type,
                    file_id: node.id,
                })
            })
            .collect()
    }
    fn open(
        &self,
        node: &Arc<dyn VfsNode>,
        flags: u32,
    ) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let node = self.own(node)?;
        if flags & 3 != 0 || flags & (0x200 | 0x400) != 0 {
            return Err(error(K::PermissionDenied));
        }
        let bytes = if let Some(id) = node.supply.filter(|_| node.field != 0) {
            value(
                &power_supply::snapshot(id).map_err(|_| error(K::NotFound))?,
                node.field,
            )
            .ok_or_else(|| error(K::NotFound))?
            .into_bytes()
        } else {
            Vec::new()
        };
        Ok(Arc::new(File {
            node: self.node(node.supply, node.field),
            state: IrqSpinLock::new(State {
                position: 0,
                bytes,
                entries: None,
            }),
        }))
    }
}
struct State {
    position: usize,
    bytes: Vec<u8>,
    entries: Option<Vec<DirectoryEntryInternal>>,
}
struct File {
    node: Arc<Node>,
    state: IrqSpinLock<State>,
}
impl StreamOps for File {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, crate::object::capability::StreamError> {
        use crate::object::capability::StreamError;
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.node.field == 0 {
            let fs = self.node.fs.upgrade().ok_or(StreamError::InvalidArgument)?;
            // Provider I/O happens outside the file's IRQ lock.
            let entries = if self.state.lock().entries.is_none() {
                Some(
                    fs.readdir(&(self.node.clone() as Arc<dyn VfsNode>))
                        .map_err(|_| StreamError::InvalidArgument)?,
                )
            } else {
                None
            };
            let mut state = self.state.lock();
            if let Some(entries) = entries {
                state.entries = Some(entries);
            }
            let Some(entry) = state.entries.as_ref().unwrap().get(state.position) else {
                return Ok(0);
            };
            let record =
                crate::fs::DirectoryEntry::from_internal(&crate::fs::DirectoryEntryInternal {
                    name: entry.name.clone(),
                    file_type: entry.file_type.clone(),
                    size: 0,
                    file_id: entry.file_id,
                    metadata: None,
                });
            let size = record.entry_size();
            if buffer.len() < size {
                return Err(StreamError::InvalidArgument);
            }
            // SAFETY: the initialized native directory record lives through this copy.
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    (&record as *const crate::fs::DirectoryEntry).cast::<u8>(),
                    size,
                )
            };
            buffer[..size].copy_from_slice(bytes);
            state.position += 1;
            return Ok(size);
        }
        let mut state = self.state.lock();
        let count = buffer
            .len()
            .min(state.bytes.len().saturating_sub(state.position));
        buffer[..count].copy_from_slice(&state.bytes[state.position..state.position + count]);
        state.position += count;
        Ok(count)
    }
    fn write(&self, _: &[u8]) -> Result<usize, crate::object::capability::StreamError> {
        Err(crate::object::capability::StreamError::PermissionDenied)
    }
}
impl ControlOps for File {}
impl MemoryMappingOps for File {
    fn get_mapping_info(
        &self,
        _: usize,
        _: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("power supply telemetry cannot be mapped")
    }
}
impl Selectable for File {
    fn wait_until_ready(
        &self,
        _: ReadyInterest,
        _: &mut crate::arch::Trapframe,
        _: Option<u64>,
        _: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}
impl FileObject for File {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn metadata(&self) -> Result<FileMetadata, crate::object::capability::StreamError> {
        self.node
            .metadata()
            .map_err(|_| crate::object::capability::StreamError::InvalidArgument)
    }
    fn seek(&self, from: SeekFrom) -> Result<u64, crate::object::capability::StreamError> {
        use crate::object::capability::StreamError;
        let mut state = self.state.lock();
        let position = match from {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(n) => state.position as i128 + n as i128,
            SeekFrom::End(n) if self.node.field != 0 => state.bytes.len() as i128 + n as i128,
            _ => return Err(StreamError::NotSupported),
        };
        state.position = usize::try_from(position).map_err(|_| StreamError::InvalidArgument)?;
        if state.position == 0 {
            state.entries = None;
        }
        Ok(state.position as u64)
    }
}
struct Driver;
impl FileSystemDriver for Driver {
    fn name(&self) -> &'static str {
        "linux-power-supply"
    }
    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Virtual
    }
    fn create(&self) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        Ok(PowerSupplyFs::new())
    }
    fn create_from_option_string(
        &self,
        options: &str,
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        if !options.is_empty() {
            return Err(error(K::InvalidData));
        }
        self.create()
    }
}
fn register() {
    crate::fs::get_fs_driver_manager().register_driver(Box::new(Driver));
}
crate::driver_initcall!(register);

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn native_units_are_translated_without_inventing_unknown_measurements() {
        let mut snapshot = PowerSupplySnapshot {
            kind: SupplyKind::Battery,
            ..Default::default()
        };
        assert_eq!(value(&snapshot, 1).as_deref(), Some("Battery\n"));
        assert!(value(&snapshot, 5).is_none());
        snapshot.state.capacity_permille = Some(427);
        snapshot.state.temperature_mc = Some(-15200);
        snapshot.state.current_ua = Some(-123456);
        snapshot.state.input_current_limit_ua = Some(500000);
        assert_eq!(value(&snapshot, 5).as_deref(), Some("42\n"));
        assert_eq!(value(&snapshot, 8).as_deref(), Some("-152\n"));
        assert_eq!(value(&snapshot, 7).as_deref(), Some("-123456\n"));
        assert!(
            value(&snapshot, 10)
                .unwrap()
                .contains("POWER_SUPPLY_INPUT_CURRENT_LIMIT=500000\n")
        );
        assert!(!value(&snapshot, 10).unwrap().contains("VOLTAGE_NOW"));
        snapshot.read_failed = true;
        assert!(value(&snapshot, 5).is_none());
        assert!(!value(&snapshot, 10).unwrap().contains("CAPACITY"));
        snapshot.read_failed = false;
        snapshot.state.capacity_permille = Some(1001);
        assert!(value(&snapshot, 5).is_none());
    }
    #[test_case]
    fn power_supply_view_is_read_only_and_keeps_boot_stable_node_identity() {
        let fs = PowerSupplyFs::new();
        let root = fs.root_node();
        assert_eq!(fs.readdir(&root).unwrap().len(), power_supply::count());
        assert_eq!(fs.lookup(&root, &".".into()).unwrap().id(), root.id());
        assert!(fs.open(&root, 1).is_err());
        assert!(fs.open(&root, 0x200).is_err());
        assert!(
            fs.create(&root, &"fake-battery".into(), FileType::Directory, 0)
                .is_err()
        );
        assert!(fs.remove(&root, &"scarlet0".into()).is_err());
        assert_eq!(fs.node(Some(7), 5).id(), fs.node(Some(7), 5).id());
    }
}
