//! Checked, eager ELF64 loading for Scarlet's native ABI.
//!
//! This crate never dereferences process addresses. The platform implements file
//! search, zero-filled mappings, memory access and page protection. All object
//! metadata and relocations are validated before memory is written. Only native
//! little-endian AArch64 and RV64 RELA objects are supported; TLS, symbol versions,
//! IFUNC, RELR, executable stacks and text relocations fail explicitly.
#![no_std]

extern crate alloc;

use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::fmt;

mod elf;
use elf::{Parsed, Symbol};

pub const PAGE_SIZE: usize = 4096;
/// Maximum page-rounded virtual span accepted for a single ELF image.
pub const MAX_IMAGE_SIZE: usize = 1024 * 1024 * 1024;
const MAX_OBJECTS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Machine {
    Aarch64,
    Riscv64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Format(&'static str),
    Unsupported(&'static str),
    MissingSymbol(String),
    Platform(String),
    Overflow,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(s) => write!(f, "invalid ELF: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported ELF feature: {s}"),
            Self::MissingSymbol(s) => write!(f, "undefined symbol: {s}"),
            Self::Platform(s) => write!(f, "platform error: {s}"),
            Self::Overflow => f.write_str("ELF address or size overflow"),
        }
    }
}
impl core::error::Error for Error {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Permissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

pub struct File {
    pub identity: String,
    pub bytes: Vec<u8>,
}

/// Platform memory operations must check their own address space permissions.
/// `map` returns zero-filled, readable/writable, non-executable storage aligned to
/// `align` (a power of two, at least PAGE_SIZE). Its whole `size` is reserved.
/// `read_file` returns a stable canonical identity used to deduplicate objects.
/// `requester` is that identity for dependency search, or None for an initial load.
/// Protection calls are page aligned, may include inaccessible pages, and must
/// synchronize instruction caches before making new code executable.
pub trait Platform {
    fn read_file(&mut self, requester: Option<&str>, name: &str) -> Result<File, Error>;
    fn map(&mut self, size: usize, align: usize) -> Result<usize, Error>;
    fn write(&mut self, address: usize, bytes: &[u8]) -> Result<(), Error>;
    fn read(&mut self, address: usize, bytes: &mut [u8]) -> Result<(), Error>;
    fn protect(
        &mut self,
        address: usize,
        size: usize,
        permissions: Permissions,
    ) -> Result<(), Error>;
    fn unmap(&mut self, address: usize, size: usize);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectId(usize);

struct Object {
    identity: String,
    parsed: Parsed,
    bias: usize,
    owned_mapping: Option<(usize, usize)>,
    dependencies: Vec<ObjectId>,
    initialized: bool,
}

/// One process-wide global scope. Keep this value alive while any loaded code is
/// executing. Loading is eager and adds RTLD_GLOBAL-like scope; unloading and
/// RTLD_LOCAL semantics are intentionally not provided.
pub struct LoaderContext<P: Platform> {
    platform: P,
    machine: Machine,
    objects: Vec<Object>,
    exports: Vec<(Vec<u8>, usize)>,
}

impl<P: Platform> LoaderContext<P> {
    pub fn new(platform: P, machine: Machine) -> Self {
        Self {
            platform,
            machine,
            objects: Vec::new(),
            exports: Vec::new(),
        }
    }
    pub fn platform(&self) -> &P {
        &self.platform
    }
    pub fn platform_mut(&mut self) -> &mut P {
        &mut self.platform
    }
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }
    pub fn identity(&self, id: ObjectId) -> Result<&str, Error> {
        Ok(&self.object(id)?.identity)
    }
    /// Register a platform ABI export used after the ELF global scope. Intended
    /// for dlopen/dlsym/dlerror and runtime entry points supplied by scarlet-ld.
    pub fn add_symbol(&mut self, name: &str, address: usize) {
        if let Some((_, old)) = self.exports.iter_mut().find(|(n, _)| n == name.as_bytes()) {
            *old = address;
        } else {
            self.exports.push((name.as_bytes().to_vec(), address));
        }
    }
    pub fn load(&mut self, path: &str) -> Result<ObjectId, Error> {
        self.load_inner(path, None)
    }
    /// Adopt the kernel's already mapped main program into this loader's scope.
    ///
    /// # Safety
    /// The caller must ensure the file matches the currently mapped image,
    /// `bias + p_vaddr` addresses each complete PT_LOAD segment, and writable
    /// segments can be modified. The mappings must outlive this context. Invoke
    /// before executing program code and before loading other objects. The main
    /// program startup owns its own constructors; pending initializers include
    /// only dependencies, while `constructors(main)` remains available. Adopted
    /// mappings are never unmapped, including on failure; failed relocations may
    /// have changed them, so execution must terminate on an error.
    pub unsafe fn load_existing(&mut self, path: &str, bias: usize) -> Result<ObjectId, Error> {
        if !self.objects.is_empty() {
            return Err(Error::Format("main image must be registered first"));
        }
        self.load_inner(path, Some(bias))
    }
    fn load_inner(&mut self, path: &str, adopt_bias: Option<usize>) -> Result<ObjectId, Error> {
        let start = self.objects.len();
        let result = self.load_transaction(path, adopt_bias, start);
        if result.is_err() {
            for object in self.objects.drain(start..) {
                if let Some((address, size)) = object.owned_mapping {
                    self.platform.unmap(address, size);
                }
            }
        }
        result
    }
    fn load_transaction(
        &mut self,
        path: &str,
        adopt_bias: Option<usize>,
        start: usize,
    ) -> Result<ObjectId, Error> {
        let file = self.platform.read_file(None, path)?;
        if let Some(id) = self.find_identity(&file.identity) {
            return Ok(id);
        }
        let root = self.insert(file, adopt_bias)?;
        // Insertion order is breadth-first and is also symbol interposition order.
        let mut cursor = start;
        while cursor < self.objects.len() {
            let needs = self.objects[cursor].parsed.needed.clone();
            for name in needs {
                let file = self
                    .platform
                    .read_file(Some(&self.objects[cursor].identity), &name)?;
                let dependency = match self.find_identity(&file.identity) {
                    Some(id) => id,
                    None => self.insert(file, None)?,
                };
                if !self.objects[cursor].dependencies.contains(&dependency) {
                    self.objects[cursor].dependencies.push(dependency);
                }
            }
            cursor += 1;
        }
        // Precompute every patch before applying any. Undefined symbols or a bad
        // later relocation therefore cannot partially alter an adopted main.
        let mut patches = Vec::new();
        for i in start..self.objects.len() {
            self.relocation_patches(ObjectId(i), &mut patches)?;
        }
        for (address, value) in patches {
            self.platform.write(address, &value.to_le_bytes())?;
        }
        for i in start..self.objects.len() {
            if !self.objects[i].initialized {
                self.constructors(ObjectId(i))?;
            }
        }
        for i in start..self.objects.len() {
            self.protect(ObjectId(i))?;
        }
        Ok(root)
    }
    fn find_identity(&self, identity: &str) -> Option<ObjectId> {
        self.objects
            .iter()
            .position(|o| o.identity == identity)
            .map(ObjectId)
    }
    fn insert(&mut self, file: File, adopt: Option<usize>) -> Result<ObjectId, Error> {
        if self.objects.len() >= MAX_OBJECTS {
            return Err(Error::Unsupported("too many shared objects"));
        }
        let parsed = Parsed::parse(&file.bytes, self.machine)?;
        if parsed.executable && adopt.is_none() {
            return Err(Error::Unsupported("ET_EXEC requires load_existing"));
        }
        let (bias, owned_mapping) = if let Some(bias) = adopt {
            if parsed.executable && bias != 0 {
                return Err(Error::Format("ET_EXEC has nonzero load bias"));
            }
            if bias % parsed.align != 0 {
                return Err(Error::Format("load bias violates segment alignment"));
            }
            bias.checked_add(parsed.max_vaddr).ok_or(Error::Overflow)?;
            (bias, None)
        } else {
            // Prefix slack preserves each segment's p_align even if the minimum
            // virtual address is not itself aligned to the largest p_align.
            let prefix = parsed.min_vaddr % parsed.align;
            let size = parsed
                .max_vaddr
                .checked_sub(parsed.min_vaddr)
                .and_then(|s| s.checked_add(prefix))
                .ok_or(Error::Overflow)?;
            let base = self.platform.map(size, parsed.align)?;
            let image_base = match base.checked_add(prefix) {
                Some(v) => v,
                None => {
                    self.platform.unmap(base, size);
                    return Err(Error::Overflow);
                }
            };
            if base % parsed.align != 0
                || image_base < parsed.min_vaddr
                || base.checked_add(size).is_none()
            {
                self.platform.unmap(base, size);
                return Err(Error::Platform(
                    "map returned invalid or insufficiently aligned address".to_string(),
                ));
            }
            let bias = image_base - parsed.min_vaddr;
            for segment in &parsed.segments {
                if segment.file_size != 0 {
                    let address = bias.checked_add(segment.vaddr).ok_or(Error::Overflow)?;
                    if let Err(error) = self.platform.write(
                        address,
                        &file.bytes[segment.offset..segment.offset + segment.file_size],
                    ) {
                        self.platform.unmap(base, size);
                        return Err(error);
                    }
                }
            }
            (bias, Some((base, size)))
        };
        let id = ObjectId(self.objects.len());
        self.objects.push(Object {
            identity: file.identity,
            parsed,
            bias,
            owned_mapping,
            dependencies: Vec::new(),
            initialized: adopt.is_some(),
        });
        Ok(id)
    }
    fn object(&self, id: ObjectId) -> Result<&Object, Error> {
        self.objects
            .get(id.0)
            .ok_or(Error::Format("invalid object handle"))
    }
    pub fn entry(&self, id: ObjectId) -> Result<usize, Error> {
        let object = self.object(id)?;
        let entry = object.parsed.entry;
        if !object
            .parsed
            .segments
            .iter()
            .any(|s| s.permissions.execute && s.contains(entry, 1))
        {
            return Err(Error::Format("entry is not in an executable segment"));
        }
        object.bias.checked_add(entry).ok_or(Error::Overflow)
    }
    /// Search the handle's breadth-first dependency scope. Undefined weak symbols
    /// are not returned, but defined symbols whose address is zero are supported.
    pub fn lookup(&self, id: ObjectId, name: &str) -> Result<Option<usize>, Error> {
        self.object(id)?;
        let mut scope = vec![id];
        let mut cursor = 0;
        while cursor < scope.len() {
            let object = self.object(scope[cursor])?;
            if let Some(symbol) = object.parsed.export(name.as_bytes()) {
                return Ok(Some(symbol_address(object, symbol)?));
            }
            for dependency in &object.dependencies {
                if !scope.contains(dependency) {
                    scope.push(*dependency);
                }
            }
            cursor += 1;
        }
        Ok(self
            .exports
            .iter()
            .find(|(n, _)| n == name.as_bytes())
            .map(|(_, a)| *a))
    }
    /// Search the complete process-wide interposition scope.
    pub fn lookup_global(&self, name: &str) -> Result<Option<usize>, Error> {
        self.resolve_global(name.as_bytes())
    }
    fn resolve_global(&self, name: &[u8]) -> Result<Option<usize>, Error> {
        for object in &self.objects {
            if let Some(symbol) = object.parsed.export(name) {
                return Ok(Some(symbol_address(object, symbol)?));
            }
        }
        Ok(self
            .exports
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, a)| *a))
    }
    fn resolve_symbol(&self, object: &Object, index: usize) -> Result<usize, Error> {
        if index == 0 {
            return Ok(0);
        }
        let symbol = object
            .parsed
            .symbols
            .get(index)
            .ok_or(Error::Format("relocation symbol index is out of bounds"))?;
        if symbol.defined && (symbol.binding == 0 || symbol.visibility != 0) {
            return symbol_address(object, symbol);
        }
        if let Some(address) = self.resolve_global(&symbol.name)? {
            return Ok(address);
        }
        if symbol.defined {
            return symbol_address(object, symbol);
        }
        if symbol.binding == 2 {
            return Ok(0);
        }
        Err(Error::MissingSymbol(
            String::from_utf8_lossy(&symbol.name).into_owned(),
        ))
    }
    fn relocation_patches(
        &self,
        id: ObjectId,
        patches: &mut Vec<(usize, u64)>,
    ) -> Result<(), Error> {
        let object = self.object(id)?;
        for relocation in &object.parsed.relocations {
            if relocation.kind == 0 {
                continue;
            }
            let relative = match self.machine {
                Machine::Aarch64 => relocation.kind == 1027,
                Machine::Riscv64 => relocation.kind == 3,
            };
            let base = if relative {
                object.bias
            } else {
                self.resolve_symbol(object, relocation.symbol)?
            };
            // ELF64 relocation arithmetic is modulo 2^64, including negative
            // addends. Range validation applies to writes, not encoded values.
            let value = if self.machine == Machine::Riscv64 && relocation.kind == 5 {
                base as u64
            } else {
                (base as u64).wrapping_add(relocation.addend as u64)
            };
            let address = object
                .bias
                .checked_add(relocation.offset)
                .ok_or(Error::Overflow)?;
            patches.push((address, value));
        }
        Ok(())
    }
    fn protect(&mut self, id: ObjectId) -> Result<(), Error> {
        let object = self.object(id)?;
        let mut ranges: Vec<(usize, usize, Permissions)> = Vec::new();
        let mut page = object.parsed.min_vaddr;
        while page < object.parsed.max_vaddr {
            let mut permissions = Permissions::default();
            let mut loaded = false;
            for segment in &object.parsed.segments {
                if page < segment.vaddr + segment.memory_size && page + PAGE_SIZE > segment.vaddr {
                    loaded = true;
                    permissions.read |= segment.permissions.read;
                    permissions.write |= segment.permissions.write;
                    permissions.execute |= segment.permissions.execute;
                }
            }
            if let Some((begin, end)) = object.parsed.relro {
                // Partial trailing pages retain their segment permissions.
                if page >= begin && page < end {
                    permissions.write = false;
                }
            }
            if object.owned_mapping.is_some() || loaded {
                let address = object.bias.checked_add(page).ok_or(Error::Overflow)?;
                if let Some((start, size, previous)) = ranges.last_mut() {
                    if *previous == permissions && *start + *size == address {
                        *size += PAGE_SIZE;
                    } else {
                        ranges.push((address, PAGE_SIZE, permissions));
                    }
                } else {
                    ranges.push((address, PAGE_SIZE, permissions));
                }
            }
            page += PAGE_SIZE;
        }
        if let Some((base, _)) = object.owned_mapping {
            let image_base = object.bias + object.parsed.min_vaddr;
            if image_base > base {
                ranges.insert(0, (base, image_base - base, Permissions::default()));
            }
        }
        for (address, size, permissions) in ranges {
            self.platform.protect(address, size, permissions)?;
        }
        Ok(())
    }
    /// Return this object's DT_INIT then DT_INIT_ARRAY addresses. Does not execute
    /// untrusted code. Zero and all-ones sentinel entries are omitted.
    pub fn constructors(&mut self, id: ObjectId) -> Result<Vec<usize>, Error> {
        let object = self.object(id)?;
        let bias = object.bias;
        let init = object.parsed.init;
        let array = object.parsed.init_array;
        let mut result = Vec::new();
        if let Some(init) = init
            && init != 0
        {
            result.push(bias.checked_add(init).ok_or(Error::Overflow)?);
        }
        if let Some((array, count)) = array {
            for i in 0..count {
                let address = bias
                    .checked_add(array)
                    .and_then(|a| a.checked_add(i * 8))
                    .ok_or(Error::Overflow)?;
                let mut bytes = [0u8; 8];
                self.platform.read(address, &mut bytes)?;
                let target =
                    usize::try_from(u64::from_le_bytes(bytes)).map_err(|_| Error::Overflow)?;
                if target != 0 && target != usize::MAX {
                    result.push(target);
                }
            }
        }
        for target in &result {
            if !self.objects.iter().any(|o| {
                o.parsed.segments.iter().any(|s| {
                    s.permissions.execute
                        && target.checked_sub(o.bias).is_some_and(|v| s.contains(v, 1))
                })
            }) {
                return Err(Error::Format(
                    "constructor is outside executable loaded segments",
                ));
            }
        }
        Ok(result)
    }
    /// Mark and return dependency-first initializers once. Cycles are visited
    /// once; a failure leaves every pending object's initialized state unchanged.
    /// The caller must execute the returned functions before exposing loaded code.
    pub fn take_pending_initializers(&mut self) -> Result<Vec<usize>, Error> {
        let mut state = vec![0u8; self.objects.len()];
        let mut order = Vec::new();
        for i in 0..self.objects.len() {
            self.visit_initializers(ObjectId(i), &mut state, &mut order);
        }
        let mut result = Vec::new();
        for id in &order {
            result.extend(self.constructors(*id)?);
        }
        for id in order {
            self.objects[id.0].initialized = true;
        }
        Ok(result)
    }
    fn visit_initializers(&self, id: ObjectId, state: &mut [u8], order: &mut Vec<ObjectId>) {
        if state[id.0] != 0 || self.objects[id.0].initialized {
            return;
        }
        state[id.0] = 1;
        for dependency in &self.objects[id.0].dependencies {
            self.visit_initializers(*dependency, state, order);
        }
        state[id.0] = 2;
        order.push(id);
    }
}
impl<P: Platform> Drop for LoaderContext<P> {
    fn drop(&mut self) {
        for object in self.objects.drain(..) {
            if let Some((address, size)) = object.owned_mapping {
                self.platform.unmap(address, size);
            }
        }
    }
}
fn symbol_address(object: &Object, symbol: &Symbol) -> Result<usize, Error> {
    if symbol.absolute {
        Ok(symbol.value)
    } else {
        object.bias.checked_add(symbol.value).ok_or(Error::Overflow)
    }
}

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod tests;
