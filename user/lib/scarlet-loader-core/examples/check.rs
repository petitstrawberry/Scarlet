//! Validate actual cross-linked images on the host without executing target code.
//! cargo run --example check -- <fixture-staging-directory> <aarch64|riscv64>
use scarlet_loader_core::{Error, File, LoaderContext, Machine, Permissions, Platform};
use std::{collections::BTreeMap, path::PathBuf};

struct Host {
    root: PathBuf,
    mappings: BTreeMap<usize, Vec<u8>>,
}
impl Platform for Host {
    fn read_file(&mut self, _: Option<&str>, name: &str) -> Result<File, Error> {
        let path = if name.starts_with('/') {
            self.root.join(name.trim_start_matches('/'))
        } else if name.contains('/') || name == "init" {
            self.root.join(name)
        } else {
            self.root.join("system/lib").join(name)
        };
        let bytes = std::fs::read(&path)
            .map_err(|e| Error::Platform(format!("{}: {e}", path.display())))?;
        let identity = std::fs::canonicalize(&path)
            .map_err(|e| Error::Platform(e.to_string()))?
            .to_string_lossy()
            .into_owned();
        Ok(File { identity, bytes })
    }
    fn map(&mut self, size: usize, align: usize) -> Result<usize, Error> {
        let next = self
            .mappings
            .last_key_value()
            .map_or(0x1000_0000, |(base, bytes)| base + bytes.len());
        let base = (next + align - 1) & !(align - 1);
        self.mappings.insert(base, vec![0; size]);
        Ok(base)
    }
    fn read(&mut self, address: usize, bytes: &mut [u8]) -> Result<(), Error> {
        for (base, map) in &self.mappings {
            if let Some(offset) = address.checked_sub(*base)
                && let Some(source) = map.get(offset..offset.saturating_add(bytes.len()))
            {
                bytes.copy_from_slice(source);
                return Ok(());
            }
        }
        Err(Error::Platform("unmapped read".into()))
    }
    fn write(&mut self, address: usize, bytes: &[u8]) -> Result<(), Error> {
        for (base, map) in &mut self.mappings {
            if let Some(offset) = address.checked_sub(*base)
                && let Some(target) = map.get_mut(offset..offset.saturating_add(bytes.len()))
            {
                target.copy_from_slice(bytes);
                return Ok(());
            }
        }
        Err(Error::Platform("unmapped write".into()))
    }
    fn protect(
        &mut self,
        address: usize,
        size: usize,
        permissions: Permissions,
    ) -> Result<(), Error> {
        if !address.is_multiple_of(4096)
            || !size.is_multiple_of(4096)
            || (permissions.write && permissions.execute)
        {
            return Err(Error::Platform("invalid final page permissions".into()));
        }
        if !self
            .mappings
            .iter()
            .any(|(base, map)| address >= *base && address.saturating_add(size) <= base + map.len())
        {
            return Err(Error::Platform("unmapped protection range".into()));
        }
        Ok(())
    }
    fn unmap(&mut self, address: usize, _: usize) {
        self.mappings.remove(&address);
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let root = args
        .next()
        .ok_or("expected fixture staging directory")?
        .into();
    let machine = match args.next().as_deref() {
        Some("aarch64") => Machine::Aarch64,
        Some("riscv64") => Machine::Riscv64,
        _ => return Err("expected aarch64 or riscv64".into()),
    };
    let mut loader = LoaderContext::new(
        Host {
            root,
            mappings: BTreeMap::new(),
        },
        machine,
    );
    for (i, name) in ["dlopen", "dlsym", "dlclose", "dlerror"].iter().enumerate() {
        loader.add_symbol(name, 0xf000_0000 + i * 8);
    }
    let main = loader.load("init")?;
    let dependency = loader.load("libsmoke-dependency.so")?;
    let answer = loader.load("libsmoke-answer.so")?;
    let mut expected = loader.constructors(dependency)?;
    expected.extend(loader.constructors(answer)?);
    let initializers = loader.take_pending_initializers()?;
    assert_eq!(initializers.len(), 2, "startup dependency constructors");
    assert_eq!(
        initializers, expected,
        "dependency must initialize before answer"
    );
    println!(
        "main entry={:#x}, objects={}, initializers={}",
        loader.entry(main)?,
        loader.object_count(),
        initializers.len()
    );
    assert!(loader.lookup(main, "answer")?.is_some());
    assert!(loader.lookup(main, "executable_value")?.is_some());
    let plugin = loader.load("libsmoke-plugin.so")?;
    let initializers = loader.take_pending_initializers()?;
    assert_eq!(initializers.len(), 1, "plugin constructor");
    assert_eq!(initializers, loader.constructors(plugin)?);
    println!(
        "plugin objects={}, initializers={}",
        loader.object_count(),
        initializers.len()
    );
    assert!(loader.lookup(plugin, "plugin_answer")?.is_some());
    assert!(loader.take_pending_initializers()?.is_empty());
    Ok(())
}
