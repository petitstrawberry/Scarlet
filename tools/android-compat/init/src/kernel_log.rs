//! Explicit native diagnostics outside the restricted Android process tree.
use std::println;

pub struct Collector(i32);

impl Collector {
    pub fn start() -> Result<Option<Self>, &'static str> {
        if option_env!("SCARLET_ANDROID_INIT_BOOTSTRAP") != Some("1")
            || option_env!("SCARLET_ANDROID_DIAG_KERNEL_LOG") != Some("1") {
            return Ok(None);
        }
        // A separate native process retains its initial diagnostic authority.
        // The manager remains single-threaded for Shield subject assignment,
        // then restricts itself and every Android descendant as usual.
        match std::task::fork() {
            -1 => Err("kernel log collector fork"),
            0 => {
                collect();
                std::task::exit(0);
            }
            pid => {
                println!("SCARLET_ANDROID_KERNEL_LOG_STARTED pid={}", pid);
                Ok(Some(Self(pid)))
            }
        }
    }
}

pub fn collect() {
    if option_env!("SCARLET_ANDROID_INIT_BOOTSTRAP") == Some("1") {
        collect_fault_snapshots();
        return;
    }
    let Ok(mut log) = std::fs::File::open("/dev/kmsg") else {
        println!("SCARLET_ANDROID_KERNEL_LOG_FAIL: open");
        return;
    };
    let mut bytes = [0; 8192];
    loop {
        match log.read(&mut bytes) {
            Ok(0) => break,
            Ok(count) => std::print!("{}", std::string::String::from_utf8_lossy(&bytes[..count])),
            Err(error) => {
                println!("SCARLET_ANDROID_KERNEL_LOG_FAIL: {}", error);
                break;
            }
        }
    }
}

fn collect_fault_snapshots() {
    // The current log byte writer does not wake readers after every message.
    // Read bounded fresh snapshots so a collector cannot sleep through a fault.
    let mut bytes = std::vec![0; 256 * 1024];
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let snapshot = std::fs::File::open("/dev/kmsg")
            .and_then(|mut log| log.read(&mut bytes));
        let count = match snapshot {
            Ok(count) => count,
            Err(error) => { println!("SCARLET_ANDROID_KERNEL_LOG_FAIL: {}", error); return; }
        };
        for line in std::string::String::from_utf8_lossy(&bytes[..count]).lines() {
            if line.contains("[fault]") || line.contains("caused") || line.contains("ANDROID_VM_TRACE")
                || line.contains("SCARLET_EXEC_FAILURE")
                || line.contains("[user-bt]") || line.contains("fatal signal=")
                || line.contains("sigreturn failed") || line.contains("signal frame failed")
                || line.starts_with("ESR_EL1:") || line.starts_with("FAR_EL1:")
                || line.starts_with("ELR_EL1:") || line.starts_with("INSN_ELR:")
                || line.starts_with('x') && line.contains('=') {
                if seen.len() >= 4096 { println!("SCARLET_ANDROID_KERNEL_LOG_LIMIT"); return; }
                if seen.insert(std::string::String::from(line)) {
                    println!("SCARLET_ANDROID_KERNEL_FAULT {}", line);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        // This PID belongs only to the collector created by this guard.
        let stopped = unsafe { std::syscall::syscall2(std::syscall::Syscall::Kill, self.0 as usize, 9) };
        let (waited, status) = std::task::waitpid(self.0, 0);
        println!("SCARLET_ANDROID_KERNEL_LOG_STOPPED pid={} waited={} status={} stop_result={}", self.0, waited, status, stopped);
    }
}
