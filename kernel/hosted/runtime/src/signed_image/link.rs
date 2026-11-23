//! Bind the ELF objects selected by this exec into private process mappings.
use super::{Access, ENVIRON_OFFSET, Image, Instance, Kind, Lease, PAGE, linked_text};
use crate::in_process::{SignedText, link_exports};
pub use scarlet_loader_core::ObjectId;
use scarlet_loader_core::{Error, File, LoaderContext, Machine, Permissions, Platform};
use std::{io, ptr, sync::Arc};

type Resolve = Box<dyn FnMut(Option<&str>, &str) -> io::Result<(String, Arc<Image>)> + Send + Sync>;

struct ProcessLink {
    lookup: Option<Resolve>,
    main: Option<(String, Arc<super::Inner>)>,
    pending: Option<Arc<Image>>,
    modules: Vec<Arc<Lease>>,
    published: usize,
    budget: Option<(usize, usize)>,
    tls: usize,
}

impl ProcessLink {
    fn parked(modules: Vec<Arc<Lease>>) -> Self {
        Self {
            lookup: None,
            main: None,
            pending: None,
            modules,
            published: 0,
            budget: None,
            tls: 0,
        }
    }

    fn segment(
        &self,
        address: usize,
        size: usize,
    ) -> Result<(&Lease, &super::manifest::Segment), Error> {
        self.modules
            .iter()
            .find_map(|module| {
                let offset = address.checked_sub(module.base)?;
                module
                    .image
                    .manifest
                    .segments
                    .iter()
                    .find(|s| s.contains(offset, size))
                    .map(|s| (module.as_ref(), s))
            })
            .ok_or(Error::Format("loader access outside selected ELF segments"))
    }
}

impl Platform for ProcessLink {
    fn page_size(&self) -> usize {
        PAGE
    }

    fn read_file(&mut self, requester: Option<&str>, name: &str) -> Result<File, Error> {
        if requester.is_none()
            && let Some((identity, main)) = self.main.take()
        {
            return Ok(File {
                identity,
                bytes: main.template.clone(),
            });
        }
        if name.is_empty() {
            return Err(Error::Format("empty shared object name"));
        }
        let lookup = self
            .lookup
            .as_mut()
            .ok_or(Error::Format("missing file resolver"))?;
        let (identity, image) =
            lookup(requester, name).map_err(|e| Error::Platform(format!("{name}: {e}")))?;
        if image.0.manifest.kind != Kind::Dso {
            return Err(Error::Format("dependency is not a signed shared object"));
        }
        let bytes = image.0.template.clone();
        self.pending = Some(image);
        Ok(File { identity, bytes })
    }

    fn map(&mut self, size: usize, align: usize) -> Result<usize, Error> {
        let image = self
            .pending
            .take()
            .ok_or(Error::Format("missing shared object"))?;
        if align != PAGE || image.0.manifest.span != size {
            return Err(Error::Format("ELF and signed mapping disagree on layout"));
        }
        if let Some((text, data)) = &mut self.budget {
            let manifest = &image.0.manifest;
            let code = manifest.rx_pages().map(|range| range.len()).sum();
            let writable = manifest
                .segments
                .iter()
                .filter(|segment| segment.access == Access::ReadWrite)
                .map(|segment| segment.pages().len())
                .sum();
            *text = text
                .checked_sub(code)
                .ok_or(Error::Platform("Signed text limit exceeded".into()))?;
            *data = data
                .checked_sub(writable)
                .ok_or(Error::Platform("Signed data limit exceeded".into()))?;
        }
        let lease = image
            .0
            .acquire()
            .map_err(|e| Error::Platform(e.to_string()))?;
        let base = lease.base;
        image
            .0
            .initialize_segments(base)
            .map_err(|e| Error::Platform(e.to_string()))?;
        self.modules.push(lease);
        Ok(base)
    }

    fn write(&mut self, address: usize, bytes: &[u8]) -> Result<(), Error> {
        let (module, s) = self.segment(address, bytes.len())?;
        if self.modules[..self.published]
            .iter()
            .any(|existing| existing.base == module.base)
        {
            return Err(Error::Format("loader write to a published module"));
        }
        if s.access != Access::ReadWrite {
            // LoaderContext copies every file-backed DSO segment on initial load.
            // Its signed counterpart (including translated instructions) is already mapped.
            if module.image.manifest.kind == Kind::Dso
                && address == module.base + s.address
                && bytes == &module.image.template[s.source.clone()]
            {
                return Ok(());
            }
            return Err(Error::Format("ELF relocation into immutable segment"));
        }
        // Only unpublished, private writable ELF data is reachable here.
        unsafe {
            ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                ptr::with_exposed_provenance_mut(address),
                bytes.len(),
            )
        };
        Ok(())
    }

    fn read(&mut self, address: usize, bytes: &mut [u8]) -> Result<(), Error> {
        let (module, _) = self.segment(address, bytes.len())?;
        let alias = module.kernel_alias(address);
        unsafe {
            ptr::copy_nonoverlapping(
                ptr::with_exposed_provenance(alias),
                bytes.as_mut_ptr(),
                bytes.len(),
            )
        };
        Ok(())
    }

    fn protect(&mut self, address: usize, size: usize, _: Permissions) -> Result<(), Error> {
        // The manifest describes the same authenticated ELF. finish_instance seals
        // all RO/RELRO before publication; executable pages stay immutable throughout.
        if self.modules.iter().any(|m| {
            address
                .checked_sub(m.base)
                .and_then(|start| start.checked_add(size))
                .is_some_and(|end| end <= m.image.manifest.span)
        }) {
            Ok(())
        } else {
            Err(Error::Format("protection outside signed ELF"))
        }
    }

    fn initialize_static_tls(&mut self) -> Result<(), Error> {
        // finish_instance copies the relocated main TLS before constructors run.
        Ok(())
    }

    fn static_tls_address(&self, offset: usize) -> Result<usize, Error> {
        if self.tls == 0 || offset >= scarlet_abi::tls::NATIVE_TLS_CLEANUP_OFFSET {
            return Err(Error::Unsupported("calling thread static TLS unavailable"));
        }
        self.tls.checked_add(offset).ok_or(Error::Overflow)
    }

    fn unmap(&mut self, base: usize, size: usize) {
        self.modules.retain(|module| {
            module.base != base
                || module.image.manifest.kind != Kind::Dso
                || module.image.manifest.span != size
        });
    }
}

/// The ELF symbol scope belongs to the address space and outlives exec.
pub(crate) struct Namespace(LoaderContext<ProcessLink>);

impl Namespace {
    pub(crate) fn rebind(&self, modules: Vec<Arc<Lease>>) -> Self {
        // Only signed text supplies these leases, at the same guest addresses.
        Self(unsafe { self.0.copy_namespace(ProcessLink::parked(modules)) })
    }
}

pub struct LoadedLibrary {
    pub text: Arc<SignedText>,
    pub object: ObjectId,
    pub constructors: Vec<usize>,
    pub text_bytes: usize,
}

impl SignedText {
    /// Prepare new leases and an extended namespace. The old image remains
    /// authoritative until the caller publishes the result with the guest parked.
    ///
    /// The temporary platform may resolve files while the old mappings are
    /// active. ELF relocation and sealing are limited to new private leases.
    pub fn load_library(
        self: &Arc<Self>,
        path: &str,
        max_text: usize,
        max_data: usize,
        lookup: impl FnMut(Option<&str>, &str) -> io::Result<(String, Arc<Image>)>
        + Send
        + Sync
        + 'static,
    ) -> io::Result<LoadedLibrary> {
        let remaining_text = max_text
            .checked_sub(self.committed_text_size())
            .ok_or_else(|| io::Error::other("Signed text limit exceeded"))?;
        let remaining_data = max_data
            .checked_sub(self.committed_data_size())
            .ok_or_else(|| io::Error::other("Signed data limit exceeded"))?;
        let mut namespace = self
            .namespace()
            .ok_or_else(|| io::Error::other("No signed loader"))?
            .rebind(self.leases().to_vec());
        let loader = &mut namespace.0;
        let platform = loader.platform_mut();
        platform.lookup = Some(Box::new(lookup));
        platform.published = self.leases().len();
        platform.budget = Some((remaining_text, remaining_data));
        let object = loader.load(path).map_err(io::Error::other)?;
        let constructors = take_constructors(loader)?;
        let modules = loader.platform().modules.clone();
        for module in &modules[self.leases().len()..] {
            module.image.finish_instance(module.base)?;
        }
        *loader.platform_mut() = ProcessLink::parked(modules.clone());
        let (text, text_bytes, _) = linked_text(modules, Some(namespace))?;
        Ok(LoadedLibrary {
            text,
            object,
            constructors,
            text_bytes,
        })
    }

    pub fn lookup_symbol(
        &self,
        object: Option<ObjectId>,
        name: &str,
        tls: usize,
    ) -> io::Result<Option<usize>> {
        let mut namespace = self
            .namespace()
            .ok_or_else(|| io::Error::other("No signed loader"))?
            .rebind(self.leases().to_vec());
        namespace.0.platform_mut().tls = tls;
        match object {
            Some(id) => namespace.0.lookup(id, name),
            None => namespace.0.lookup_global(name),
        }
        .map_err(io::Error::other)
    }
}

fn take_constructors(loader: &mut LoaderContext<ProcessLink>) -> io::Result<Vec<usize>> {
    let constructors = loader
        .take_pending_initializers()
        .map_err(io::Error::other)?;
    for &pc in &constructors {
        let (_, segment) = loader.platform().segment(pc, 4).map_err(io::Error::other)?;
        if pc % 4 != 0 || segment.access != Access::Execute {
            return Err(io::Error::other("Invalid constructor address"));
        }
    }
    Ok(constructors)
}

impl Image {
    /// Each lookup uses the requester's guest pathname in the task's VFS.
    pub fn instantiate_with(
        &self,
        path: String,
        lookup: impl FnMut(Option<&str>, &str) -> io::Result<(String, Arc<Image>)>
        + Send
        + Sync
        + 'static,
    ) -> io::Result<Instance> {
        match self.0.manifest.kind {
            Kind::Static => return self.instantiate(),
            Kind::Dso => return Err(io::Error::other("Cannot exec a shared object")),
            Kind::Pie => {}
        }
        let main = self.0.acquire()?;
        let bias = main.base;
        self.0.initialize_segments(bias)?;
        let mut platform = ProcessLink::parked(vec![main]);
        platform.lookup = Some(Box::new(lookup));
        platform.main = Some((path, self.0.clone()));
        let mut loader = LoaderContext::new(platform, Machine::Aarch64);
        for (name, address) in link_exports() {
            loader.add_symbol(name, address);
        }
        loader.add_symbol("environ", bias + self.0.manifest.span + ENVIRON_OFFSET);
        // The template and segment layout come from the same immutable signed image.
        let id = unsafe { loader.load_existing("<main>", bias) }.map_err(io::Error::other)?;
        if loader.entry(id).map_err(io::Error::other)? != bias + self.0.manifest.entry {
            return Err(io::Error::other("ELF and signed main entry disagree"));
        }
        let constructors = take_constructors(&mut loader)?;
        let modules = loader.platform().modules.clone();
        for module in &modules {
            module.image.finish_instance(module.base)?;
        }
        *loader.platform_mut() = ProcessLink::parked(modules.clone());
        Instance::from_modules(modules, constructors, Some(Namespace(loader)))
    }
}
