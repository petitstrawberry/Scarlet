//! Lifecycle of the window's private Android InputManager client. Only the
//! native window receives the writer; Android applications receive neither end.
use core::sync::atomic::{AtomicBool, Ordering};
use std::{
    environment::{Environment, HandleMapping, VfsView},
    handle::Handle,
    string::String,
    sync::Arc,
    time::Duration,
    vec::Vec,
};

pub struct Bridge {
    stop: Arc<AtomicBool>,
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
        let thread_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("android-input-bridge")
            .spawn(move || {
                // Depend on the real service socket and generated linker namespace,
                // never a fixed boot delay or a change to Android's boot actions.
                while !thread_stop.load(Ordering::Acquire) {
                    if view.open("/dev/socket/zygote", 0).is_ok()
                        && view.open("/linkerconfig/default/ld.config.txt", 0).is_ok()
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                if thread_stop.load(Ordering::Acquire) {
                    return;
                }
                let result = (|| -> Result<(), &'static str> {
                    let executable = view
                        .open("/system/bin/app_process64", 0)
                        .map_err(|_| "input bridge app_process")?;
                    let mut handles: Vec<_> = stdio
                        .iter()
                        .map(|(source, target)| HandleMapping {
                            source,
                            target: *target,
                        })
                        .collect();
                    handles.push(HandleMapping {
                        source: &reader,
                        target: 3,
                    });
                    let env: Vec<_> = env.iter().map(|value| value.as_str()).collect();
                    // fork gives the client an exclusive process in which to
                    // take its delegated label before exec. The manager and
                    // its other children keep their original subjects.
                    let pid = std::task::fork();
                    if pid < 0 {
                        return Err("input bridge fork");
                    }
                    if pid == 0 {
                        let result = (|| -> Result<(), &'static str> {
                            std::shield::Policy::current()
                                .map_err(|_| "input bridge policy query")?
                                .transition(label)
                                .map_err(|_| "input bridge label transition")?;
                            environment
                                .exec_with_abi(
                                    "android-aarch64",
                                    &executable,
                                    &[
                                        "/system/bin/app_process64",
                                        "/system/bin",
                                        "org.scarlet.android.input.InputBridge",
                                    ],
                                    &env,
                                    "/",
                                    &handles,
                                )
                                .map_err(|_| "input bridge exec")?;
                            Ok(())
                        })();
                        if let Err(error) = result {
                            std::println!("SCARLET_ANDROID_INPUT_BRIDGE_FAIL {}", error);
                        }
                        std::task::exit(127);
                    }
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
