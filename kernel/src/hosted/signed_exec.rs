//! Resolve an executable file capability to a trusted, bundled iOS image.

use crate::{
    arch::Trapframe,
    fs::{VfsManager, vfs_v2::core::VfsFileObject},
    library::std::usercopy::copy_to_user,
    object::{KernelObject, capability::FileObject},
    task::{
        Task,
        elf_loader::{
            ElfTlsImage, ExecutionMode, LoadElfResult, ProgramHeadersInfo, setup_native_exec_stack,
        },
    },
};
use alloc::{collections::BTreeMap, format, string::String, sync::Arc, vec};
use core::sync::atomic::Ordering;
use scarlet_hosted_runtime::signed_image::Image;
use sha2::{Digest, Sha256};
use std::sync::{LazyLock, Mutex};

type Catalog = BTreeMap<String, Arc<Image>>;
static CATALOG: LazyLock<Mutex<Catalog>> = LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// Publish a trusted bundle image under the digest of the original ELF.
pub fn register_signed_image(image: Arc<Image>) -> Result<(), &'static str> {
    use alloc::collections::btree_map::Entry;
    let mut catalog = CATALOG.lock().unwrap();
    match catalog.entry(image.elf_sha256().into()) {
        Entry::Vacant(entry) => {
            entry.insert(image);
            Ok(())
        }
        Entry::Occupied(_) => Err("ELF digest already registered"),
    }
}

fn executable_digest(file: &dyn FileObject) -> Result<String, &'static str> {
    let size = file
        .metadata()
        .map_err(|_| "Cannot inspect executable")?
        .size;
    if size == 0 || size > 128 * 1024 * 1024 {
        return Err("Invalid executable file size");
    }
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 16384.min(size)];
    let mut offset = 0;
    while offset < size {
        let length = buffer.len().min(size - offset);
        let count = file
            .read_at(offset as u64, &mut buffer[..length])
            .map_err(|_| "Cannot read executable for signed image lookup")?;
        if count == 0 || count > length {
            return Err("Executable changed during signed image lookup");
        }
        digest.update(&buffer[..count]);
        offset += count;
    }
    if file
        .read_at(size as u64, &mut buffer[..1])
        .map_err(|_| "Cannot check executable EOF")?
        != 0
    {
        return Err("Executable changed during signed image lookup");
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn library_digest(vfs: Option<&VfsManager>, name: &str) -> Result<String, &'static str> {
    let vfs = vfs.ok_or("Missing dependency filesystem")?;
    let path = if name.contains('/') {
        name.into()
    } else {
        format!("/lib/{name}")
    };
    let object = vfs
        .open_executable(&path)
        .map_err(|_| "Cannot open shared library")?;
    let file = object.as_file().ok_or("Invalid shared library")?;
    file.check_image_execution()?;
    executable_digest(file)
}

fn file_path(vfs: &VfsManager, file: &dyn FileObject) -> Option<String> {
    let file = file.as_any().downcast_ref::<VfsFileObject>()?;
    Some(vfs.build_absolute_path(file.get_vfs_entry(), file.get_mount_point()))
}

pub(super) fn resolve_library(
    vfs: Option<&VfsManager>,
    requester: Option<&str>,
    name: &str,
) -> std::io::Result<(String, Arc<Image>)> {
    use std::io::Error;

    let vfs = vfs.ok_or_else(|| Error::other("Missing dependency filesystem"))?;
    let object = if name.contains('/') {
        vfs.open_executable(name)
    } else {
        let adjacent = requester
            .filter(|path| path.starts_with('/'))
            .and_then(|path| path.rsplit_once('/'))
            .and_then(|(parent, _)| vfs.open_executable(&format!("{parent}/{name}")).ok());
        match adjacent {
            Some(object) => Ok(object),
            None => vfs.open_executable(&format!("/lib/{name}")),
        }
    }
    .map_err(|_| Error::other("Cannot open shared library"))?;
    let file = object
        .as_file()
        .ok_or_else(|| Error::other("Invalid shared library"))?;
    file.check_image_execution().map_err(Error::other)?;
    let identity =
        file_path(vfs, file).ok_or_else(|| Error::other("Missing guest library pathname"))?;
    let digest = executable_digest(file).map_err(Error::other)?;
    let image = CATALOG
        .lock()
        .unwrap()
        .get(&digest)
        .cloned()
        .ok_or_else(|| Error::other("Shared library is absent from the signed iOS catalog"))?;
    Ok((identity, image))
}

/// Build the new stack and execution state in the executor's unpublished Task.
/// TransparentExecutor owns the commit and retires the old lease after it.
pub(crate) fn prepare(
    object: &KernelObject,
    argv: &[&str],
    envp: &[&str],
    task: &Task,
    tf: &mut Trapframe,
) -> Result<Option<ElfTlsImage>, &'static str> {
    let file = object.as_file().ok_or("Invalid executable file object")?;
    file.check_image_execution()?;
    let digest = executable_digest(file)?;
    let image = CATALOG
        .lock()
        .unwrap()
        .get(&digest)
        .cloned()
        .ok_or("Executable is absent from the signed iOS catalog")?;
    let vfs = task.get_vfs();
    for (name, expected) in image.dependencies() {
        if library_digest(vfs.as_deref(), name)? != *expected {
            return Err("Dependency differs from its signed iOS image");
        }
    }
    let path = vfs
        .as_deref()
        .and_then(|vfs| file_path(vfs, file))
        .unwrap_or_else(|| String::from("<main>"));
    let instance = image
        .instantiate_with(path, move |requester, name| {
            resolve_library(vfs.as_deref(), requester, name)
        })
        .map_err(|error| {
            std::eprintln!("[hosted] signed image {digest}: {error}");
            "Cannot instantiate signed iOS image"
        })?;
    let (phdr, phnum) = instance
        .program_headers()
        .ok_or("Signed image has no mapped ELF headers")?;
    let result = LoadElfResult {
        mode: ExecutionMode::Static,
        page_size: super::executable_page_size(),
        entry_point: instance.entry() as u64,
        original_entry_point: None,
        base_address: None,
        interpreter_base: None,
        program_headers: ProgramHeadersInfo {
            phdr_addr: phdr as u64,
            phdr_size: 56,
            phdr_count: phnum as u64,
        },
        tls: instance.tls_template().map(|tls| ElfTlsImage {
            address: tls.address,
            file_size: tls.file_size,
            memory_size: tls.memory_size,
            alignment: tls.alignment,
        }),
    };
    if instance.text_size() > task.max_text_size
        || instance.writable_size() > task.max_data_size
        || instance.stack_size() > task.max_stack_size
    {
        return Err("Signed image exceeds task resource limits");
    }
    super::install_signed_text(task, instance.text()).map_err(|_| "Cannot attach signed image")?;
    task.vm_manager
        .hosted_image()
        .set_guest_page_size(result.page_size);
    let (sp, argv_ptr) =
        setup_native_exec_stack(task, object, argv, envp, instance.stack_top(), &result)?;
    if let Some(slot) = instance.environ_slot() {
        let envp = argv_ptr + (argv.len() + 1) * size_of::<usize>();
        copy_to_user(task, slot, &envp.to_ne_bytes()).map_err(|_| "Cannot initialize environ")?;
    }
    *task.name.write() = argv.first().copied().unwrap_or("Unnamed Task").into();
    task.text_size
        .store(instance.text_size(), Ordering::Relaxed);
    task.data_size
        .store(instance.writable_size(), Ordering::Relaxed);
    task.stack_size
        .store(instance.stack_size(), Ordering::Relaxed);
    task.brk.store(instance.heap_start(), Ordering::SeqCst);
    task.set_entry_point(instance.startup());
    let mut vcpu = task.vcpu.lock();
    vcpu.reset_iregs();
    vcpu.set_tls_pointer(instance.tls());
    vcpu.set_tpidrro_el0(0);
    vcpu.set_sp(sp);
    vcpu.iregs.reg[0] = argv.len();
    vcpu.iregs.reg[1] = argv_ptr;
    vcpu.iregs.reg[2] = instance.constructor_list();
    vcpu.switch(tf);
    Ok(result.tls)
}
