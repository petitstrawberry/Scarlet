//! Lifecycle of the window's private Android InputManager client. Only the
//! native window receives the writer; Android applications receive neither end.
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::{
    environment::{Environment, HandleMapping, MountNamespace, VfsView},
    handle::Handle,
    string::String,
    sync::Arc,
    time::Duration,
    vec::Vec,
};

pub struct Bridge {
    stop: Arc<AtomicBool>,
    session: Arc<AtomicU32>,
    thread: Option<std::thread::JoinHandle>,
}

fn bridge_label(view: &VfsView) -> Result<u32, &'static str> {
    let file = view
        .open("/scarlet/boot/shield.policy", 0)
        .map_err(|_| "input bridge policy")?;
    let stream = file.as_stream().map_err(|_| "input bridge policy stream")?;
    let mut bytes = std::vec![0; crate::boot_profile::MAX_POLICY_BYTES + 1];
    let mut count = 0;
    loop {
        if count == bytes.len() {
            return Err("input bridge policy bound");
        }
        let n = stream
            .read(&mut bytes[count..])
            .map_err(|_| "input bridge policy read")?;
        if n == 0 {
            break;
        }
        count += n;
    }
    let text = core::str::from_utf8(&bytes[..count]).map_err(|_| "input bridge policy encoding")?;
    let mut label = None;
    for line in text.lines() {
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        if let ["label", id, "u:r:scarlet_input_bridge:s0"] = fields.as_slice() {
            if label
                .replace(id.parse().map_err(|_| "input bridge label number")?)
                .is_some()
            {
                return Err("input bridge label duplicate");
            }
        }
    }
    label.ok_or("input bridge label missing")
}

fn environment_values(view: &VfsView) -> Result<Vec<String>, &'static str> {
    let file = view
        .open("/init.environ.rc", 0)
        .map_err(|_| "input bridge environment")?;
    let stream = file
        .as_stream()
        .map_err(|_| "input bridge environment stream")?;
    let mut bytes = std::vec![0; 65537];
    let mut count = 0;
    loop {
        if count == bytes.len() {
            return Err("input bridge environment bound");
        }
        let n = stream
            .read(&mut bytes[count..])
            .map_err(|_| "input bridge environment read")?;
        if n == 0 {
            break;
        }
        count += n;
    }
    let text =
        core::str::from_utf8(&bytes[..count]).map_err(|_| "input bridge environment encoding")?;
    let names = [
        "ANDROID_ROOT",
        "ANDROID_DATA",
        "ANDROID_ART_ROOT",
        "ANDROID_I18N_ROOT",
        "ANDROID_TZDATA_ROOT",
        "BOOTCLASSPATH",
    ];
    let mut values = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_ascii_whitespace();
        if fields.next() != Some("export") {
            continue;
        }
        let Some(name) = fields.next() else {
            continue;
        };
        if !names.contains(&name) {
            continue;
        }
        let value = fields.next().ok_or("input bridge environment value")?;
        if fields.next().is_some()
            || !value.starts_with('/')
            || value.contains('$')
            || values
                .iter()
                .any(|entry: &String| entry.starts_with(&std::format!("{name}=")))
        {
            return Err("input bridge environment record");
        }
        values.push(std::format!("{name}={value}"));
    }
    if values.len() != names.len() {
        return Err("input bridge environment incomplete");
    }
    values.push(String::from("CLASSPATH=/scarlet/lib/input-bridge.jar"));
    values.push(String::from("PATH=/system/bin:/apex/com.android.art/bin"));
    Ok(values)
}

impl Bridge {
    pub fn set_session(&self, pid: u32) { self.session.store(pid, Ordering::Release); }
    pub fn start(
        environment: &Environment,
        reader: Handle,
        stdio: &[HandleMapping<'_>],
    ) -> Result<Self, &'static str> {
        let environment = Environment::from_handle(
            environment
                .as_handle()
                .duplicate()
                .map_err(|_| "input bridge Environment duplicate")?,
        )
        .map_err(|_| "input bridge Environment")?;
        let view = environment
            .root("android-aarch64")
            .map_err(|_| "input bridge View")?;
        // A missing bundle component is an assembly error, not a reason to
        // silently fall back to accelerated relative mouse motion.
        let _jar = view
            .open("/scarlet/lib/input-bridge.jar", 0)
            .map_err(|_| "input bridge bundle missing")?;
        let env = environment_values(&view)?;
        let label = bridge_label(&view)?;
        let stdio: Vec<_> = stdio
            .iter()
            .map(|mapping| {
                mapping
                    .source
                    .duplicate()
                    .map(|source| (source, mapping.target))
                    .map_err(|_| "input bridge stdio duplicate")
            })
            .collect::<Result<_, _>>()?;
        let stop = Arc::new(AtomicBool::new(false));
        let session = Arc::new(AtomicU32::new(0));
        let thread_session = session.clone();
        let thread_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("android-input-bridge")
            .spawn(move || {
                // Zygote runs in init's completed default mount namespace.
                // Capture that namespace from this managed session's process
                // tree; the outer View cannot observe its private mounts.
                let mut capture_failure_reported = false;
                let namespace = loop {
                    if thread_stop.load(Ordering::Acquire) { return; }
                    let root = thread_session.load(Ordering::Acquire) as usize;
                    let tasks = std::task::info();
                    let selected = tasks.iter().find(|task| {
                        if root == 0 || task.pid() != task.tgid()
                            || !matches!(task.name(), "/system/bin/app_process64" | "zygote64" | "zygote") { return false; }
                        if !tasks.iter().any(|parent| parent.pid() == task.ppid() && parent.name() == "/system/bin/init") {
                            return false;
                        }
                        let mut id = task.ppid();
                        let mut seen = std::collections::BTreeSet::new();
                        while id != 0 && seen.insert(id) {
                            if id == root { return true; }
                            let Some(parent) = tasks.iter().find(|parent| parent.pid() == id) else { break; };
                            id = parent.ppid();
                        }
                        false
                    });
                    if let Some(task) = selected {
                        if let Ok(pid) = u32::try_from(task.pid()) {
                            match MountNamespace::capture_descendant(pid) {
                                Ok(namespace) => {
                                    std::println!("SCARLET_ANDROID_INPUT_NAMESPACE_CAPTURED pid={}", pid);
                                    break namespace;
                                }
                                Err(error) if !capture_failure_reported => {
                                    std::println!("SCARLET_ANDROID_INPUT_NAMESPACE_CAPTURE_FAIL pid={} error={:?}", pid, error);
                                    capture_failure_reported = true;
                                }
                                Err(_) => (),
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(250));
                };
                let result = (|| -> Result<(), &'static str> {
                    let executable = environment.root("scarlet").map_err(|_| "input bridge native View")?
                        .open("/bin/android-runtime", 0).map_err(|_| "input bridge native launcher")?;
                    let mut handles: Vec<_> = stdio
                        .iter()
                        .map(|(source, target)| HandleMapping {
                            source,
                            target: *target,
                        })
                        .collect();
                    handles.push(HandleMapping {
                        source: namespace.as_handle(),
                        target: 3,
                    });
                    handles.push(HandleMapping {source: &reader, target: 4});
                    let env: Vec<_> = env.iter().map(|value| value.as_str()).collect();
                    let label = std::format!("{label}");
                    let pid = environment.spawn(&executable,
                        &["/bin/android-runtime", "--input-bridge", "3", "4", &label],
                        &env, "/", &handles).map_err(|_| "input bridge native spawn")?;
                    // The child inherits only stdio and the read endpoint, not the
                    // manager's policy capability or the window's injection writer.
                    std::println!("SCARLET_ANDROID_INPUT_BRIDGE_STARTED pid={}", pid);
                    loop {
                        let (waited, status) =
                            std::task::waitpid(pid as i32, std::task::WAIT_NOHANG);
                        if waited == pid as i32 {
                            std::println!("SCARLET_ANDROID_INPUT_BRIDGE_EXIT status={}", status);
                            return if status == 0 {
                                Ok(())
                            } else {
                                Err("input bridge exit")
                            };
                        }
                        if thread_stop.load(Ordering::Acquire) {
                            // SAFETY: this thread exclusively owns this child PID.
                            unsafe {
                                std::syscall::syscall2(
                                    std::syscall::Syscall::Kill,
                                    pid as usize,
                                    15,
                                );
                            }
                            let _ = super::wait_for_program(pid as i32);
                            return Ok(());
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                })();
                if let Err(error) = result {
                    std::println!("SCARLET_ANDROID_INPUT_BRIDGE_FAIL {}", error);
                }
            })
            .map_err(|_| "input bridge supervisor")?;
        Ok(Self {
            stop,
            session,
            thread: Some(thread),
        })
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
