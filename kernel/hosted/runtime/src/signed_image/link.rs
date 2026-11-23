//! Bind the ELF objects selected by this exec into private process mappings.
use super::{Access, Image, Instance, Kind, Lease, PAGE};
use scarlet_loader_core::{Error, File, LoaderContext, Machine, Permissions, Platform};
use std::{collections::BTreeMap, io, ptr, sync::Arc};

struct ProcessLink<'a> {
    lookup: &'a mut dyn FnMut(&str) -> io::Result<Arc<Image>>,
    names: BTreeMap<String, Arc<Image>>,
    pending: Option<Arc<Image>>,
    modules: Vec<Arc<Lease>>,
}

impl ProcessLink<'_> {
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

impl Platform for ProcessLink<'_> {
    fn page_size(&self) -> usize {
        PAGE
    }

    fn read_file(&mut self, requester: Option<&str>, name: &str) -> Result<File, Error> {
        let main = &self.modules[0].image;
        if requester.is_none() {
            return Ok(File {
                identity: main.manifest.hash.clone(),
                bytes: main.template.clone(),
            });
        }
        if name.is_empty()
            || matches!(name, "." | "..")
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
        {
            return Err(Error::Unsupported("DT_NEEDED requires a library basename"));
        }
        if !self.names.contains_key(name) {
            let image = (self.lookup)(name).map_err(|e| Error::Platform(format!("{name}: {e}")))?;
            if image.0.manifest.kind != Kind::Dso {
                return Err(Error::Format("dependency is not a signed shared object"));
            }
            self.names.insert(name.to_owned(), image);
        }
        let image = self.names.get(name).unwrap();
        self.pending = Some(image.clone());
        Ok(File {
            identity: image.elf_sha256().to_owned(),
            bytes: image.0.template.clone(),
        })
    }

    fn map(&mut self, size: usize, align: usize) -> Result<usize, Error> {
        let image = self
            .pending
            .take()
            .ok_or(Error::Format("missing shared object"))?;
        if align != PAGE || image.0.manifest.span != size {
            return Err(Error::Format("ELF and signed mapping disagree on layout"));
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
        self.segment(address, bytes.len())?;
        unsafe {
            ptr::copy_nonoverlapping(
                ptr::with_exposed_provenance(address),
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

    fn unmap(&mut self, _: usize, _: usize) {
        // Leases own these mappings through rollback or transfer to SignedText.
    }
}

impl Image {
    /// Each lookup resolves a guest /lib basename in the task's current VFS.
    pub fn instantiate_with(
        &self,
        mut lookup: impl FnMut(&str) -> io::Result<Arc<Image>>,
    ) -> io::Result<Instance> {
        match self.0.manifest.kind {
            Kind::Static => return self.instantiate(),
            Kind::Dso => return Err(io::Error::other("Cannot exec a shared object")),
            Kind::Pie => {}
        }
        let main = self.0.acquire()?;
        let bias = main.base;
        self.0.initialize_segments(bias)?;
        let platform = ProcessLink {
            lookup: &mut lookup,
            names: BTreeMap::new(),
            pending: None,
            modules: vec![main],
        };
        let mut loader = LoaderContext::new(platform, Machine::Aarch64);
        // The template and segment layout come from the same immutable signed image.
        let id = unsafe { loader.load_existing("<main>", bias) }.map_err(io::Error::other)?;
        if loader.entry(id).map_err(io::Error::other)? != bias + self.0.manifest.entry {
            return Err(io::Error::other("ELF and signed main entry disagree"));
        }
        let constructors = loader
            .take_pending_initializers()
            .map_err(io::Error::other)?;
        for &pc in &constructors {
            let (_, segment) = loader.platform().segment(pc, 4).map_err(io::Error::other)?;
            if pc % 4 != 0 || segment.access != Access::Execute {
                return Err(io::Error::other("Invalid constructor address"));
            }
        }
        let modules = std::mem::take(&mut loader.platform_mut().modules);
        for module in &modules {
            module.image.finish_instance(module.base)?;
        }
        Instance::from_modules(modules, constructors)
    }
}
