use scarlet_aarch64_user_macos_full_bsp::{
    audio, block::RootBlock, gpu, network, root_image::RootImage, video, window,
};
use std::{env, path::Path, process::ExitCode, sync::Arc};

fn inspect_images(initramfs_path: &Path, root_path: &Path) -> Result<(), String> {
    use scarlet::{
        device::{Device, manager::DeviceManager},
        fs::vfs_v2::{
            drivers::{ext2::Ext2FileSystem, initramfs::init_initramfs},
            manager::VfsManager,
        },
        interrupt::{CpuId, InterruptId, InterruptManager},
        vm::vmem::MemoryArea,
    };

    const BSP_CPU: CpuId = 0;
    const ROOT_COMPLETION_IRQ: InterruptId = 16;

    scarlet_module_prototype::force_link();
    let _platform = scarlet::hosted::prepare(scarlet::hosted::DEFAULT_VCPUS, 64 * 1024 * 1024)
        .map_err(str::to_owned)?;
    let initramfs = std::fs::read(initramfs_path)
        .map_err(|error| format!("{}: {error}", initramfs_path.display()))?;
    let base = initramfs.as_ptr().addr();
    let last = base
        .checked_add(initramfs.len().checked_sub(1).ok_or("Empty initramfs")?)
        .ok_or("Initramfs address range overflow")?;
    let boot_vfs = Arc::new(VfsManager::new());
    init_initramfs(&boot_vfs, MemoryArea::new(base, last))?;
    let entry = scarlet::hosted::inspect_executable(boot_vfs, "/init")?;
    println!("Scarlet prepared /init at {entry:#x} from the initramfs");

    let root = RootImage::open_read_only(root_path)
        .map_err(|error| format!("{}: {error}", root_path.display()))?;
    let interrupts = InterruptManager::global();
    interrupts.init_controllers();
    interrupts.init_controllers_for_cpu(BSP_CPU);
    let device = Arc::new(
        RootBlock::attach(root, BSP_CPU, ROOT_COMPLETION_IRQ).map_err(|error| error.to_string())?,
    );
    let id = DeviceManager::get_manager().register_device_with_name(device.name().into(), device);
    let filesystem =
        Ext2FileSystem::new_from_device_id(id).map_err(|error| format!("{error:?}"))?;
    let vfs = Arc::new(VfsManager::new_with_root(filesystem));
    let entry = scarlet::hosted::inspect_executable(vfs, "/bin/stemd")?;
    println!("Scarlet prepared /bin/stemd and its interpreter at {entry:#x} from the ext2 rootfs");
    Ok(())
}

thread_local! {
    static DISPLAY: std::cell::RefCell<Option<window::BootDisplay>> = const { std::cell::RefCell::new(None) };
    static GPU: std::cell::RefCell<Option<gpu::Endpoint>> = const { std::cell::RefCell::new(None) };
    static ROOT: std::cell::RefCell<Option<RootImage>> = const { std::cell::RefCell::new(None) };
    static TERMINAL: std::cell::RefCell<Option<scarlet_aarch64_user_macos_full_bsp::terminal::HostTerminal>> = const { std::cell::RefCell::new(None) };
}

fn populate(
    manager: &scarlet::device::manager::DeviceManager,
    priority: scarlet::device::manager::DriverPriority,
) {
    use scarlet::device::{Device, manager::DriverPriority};
    use scarlet_aarch64_user_macos_full_bsp::terminal::HostTerminal;
    if priority != DriverPriority::Core {
        return;
    }
    if let Some(root) = ROOT.take() {
        let block = Arc::new(RootBlock::attach(root, 0, 16).expect("Cannot attach root device"));
        manager.register_device_with_name(block.name().into(), block);
        let stop = match HostTerminal::attach_console(0, 17, |result| {
            std::process::exit(i32::from(result.is_err()))
        }) {
            Ok(terminal) => {
                let stop = terminal.shutdown_handle();
                TERMINAL.set(Some(terminal));
                Some(stop)
            }
            Err(error) => {
                eprintln!("scarlet: host terminal: {error}");
                None
            }
        };
        if let Some(display) = DISPLAY.take() {
            if let Err(error) = display.register(manager, stop) {
                eprintln!("scarlet: window devices: {error}");
                if let Some(terminal) = TERMINAL.take() {
                    let _ = terminal.close();
                }
                std::process::exit(1);
            }
        }
        if let Some(endpoint) = GPU.take() {
            endpoint.register().expect("Cannot register headless SGFX device");
        }
    }
}

fn boot_images(
    initramfs: &Path,
    root: &Path,
    display: Option<window::BootDisplay>,
) -> Result<(), String> {
    scarlet_module_prototype::force_link();
    let image = std::fs::read(initramfs).map_err(|e| format!("{}: {e}", initramfs.display()))?;
    if image.is_empty() {
        return Err("Empty initramfs".into());
    }
    DISPLAY.set(display);
    ROOT.set(Some(
        RootImage::open(root).map_err(|e| format!("{}: {e}", root.display()))?,
    ));
    let platform =
        scarlet::hosted::prepare(scarlet::hosted::DEFAULT_VCPUS, 8 * 1024 * 1024 * 1024)?;
    platform.boot(
        &image,
        "root=/dev/hostroot rootfstype=ext2 rootwait",
        scarlet::DeviceSource::Hosted(populate),
    )
}

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if matches!(args.as_slice(), [command] if command == scarlet_hosted_runtime::worker::WORKER_ARG)
    {
        return match scarlet_hosted_runtime::worker::run_worker() {
            Ok(()) => ExitCode::SUCCESS,
            // The confined worker has closed stderr. Its fd number may now
            // belong to a task endpoint or guest backing.
            Err(_) => ExitCode::FAILURE,
        };
    }
    audio::link_boot_output();
    network::link_boot_ethernet();
    video::link_boot_decoder();
    match args.as_slice() {
        [command, initramfs, rootfs] if command == "--inspect-root" => {
            if let Err(error) = inspect_images(Path::new(initramfs), Path::new(rootfs)) {
                eprintln!("scarlet: {error}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        [initramfs, rootfs] => {
            let initramfs = std::path::PathBuf::from(initramfs);
            let rootfs = std::path::PathBuf::from(rootfs);
            match window::run(move |display| {
                if display.is_some() {
                    return boot_images(&initramfs, &rootfs, display);
                }
                gpu::run_offscreen(move |endpoint| {
                    GPU.set(endpoint);
                    boot_images(&initramfs, &rootfs, None)
                })
            }) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("scarlet: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("usage: scarlet [--inspect-root] INITRAMFS ROOTFS");
            ExitCode::FAILURE
        }
    }
}
