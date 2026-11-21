use scarlet::{
    device::{
        Device,
        manager::{DeviceManager, DriverPriority},
    },
    interrupt::{CpuId, InterruptId},
};
use scarlet_aarch64_user_linux_minimum_bsp::{
    block::RootBlock, root_image::RootImage, terminal::HostTerminal,
};
use scarlet_hosted_runtime::worker::{WORKER_ARG, run_worker};
use std::{cell::RefCell, env, fs, path::Path, process::ExitCode, sync::Arc};

const BOOT_CPU: CpuId = 0;
const ROOT_IRQ: InterruptId = 16;
const CONSOLE_IRQ: InterruptId = 17;
const RAM_BYTES: usize = 512 * 1024 * 1024;

thread_local! {
    static ROOT: RefCell<Option<RootImage>> = const { RefCell::new(None) };
    static TERMINAL: RefCell<Option<HostTerminal>> = const { RefCell::new(None) };
}

fn populate(manager: &DeviceManager, priority: DriverPriority) {
    if priority != DriverPriority::Core {
        return;
    }
    if let Some(root) = ROOT.take() {
        let result = RootBlock::attach(root, BOOT_CPU, ROOT_IRQ).and_then(|block| {
            manager.register_device_with_name(block.name().into(), Arc::new(block));
            HostTerminal::attach_console(BOOT_CPU, CONSOLE_IRQ, |result| {
                if let Err(error) = result {
                    eprintln!("scarlet: cannot restore host terminal: {error}");
                    std::process::exit(1);
                }
                std::process::exit(0);
            })
        });
        match result {
            Ok(terminal) => TERMINAL.set(Some(terminal)),
            Err(error) => {
                eprintln!("scarlet: cannot attach hosted boot devices: {error}");
                std::process::exit(1);
            }
        }
    }
}

fn boot(initramfs: &Path, rootfs: &Path) -> Result<(), String> {
    let image = fs::read(initramfs).map_err(|error| format!("{}: {error}", initramfs.display()))?;
    if image.is_empty() {
        return Err("Empty initramfs".into());
    }
    ROOT.set(Some(
        RootImage::open(rootfs).map_err(|error| format!("{}: {error}", rootfs.display()))?,
    ));
    let platform = scarlet::hosted::prepare(scarlet::hosted::DEFAULT_VCPUS, RAM_BYTES)?;
    platform.boot(
        &image,
        "root=/dev/hostroot rootfstype=ext2 rootwait",
        scarlet::DeviceSource::Hosted(populate),
    )
}

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if matches!(args.as_slice(), [command] if command == WORKER_ARG) {
        // Once confined, the worker's original stderr descriptor may be reused.
        return if run_worker().is_ok() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    let result = match args.as_slice() {
        [initramfs, rootfs] => boot(Path::new(initramfs), Path::new(rootfs)),
        _ => Err("usage: scarlet INITRAMFS ROOTFS".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("scarlet: {error}");
            ExitCode::FAILURE
        }
    }
}
