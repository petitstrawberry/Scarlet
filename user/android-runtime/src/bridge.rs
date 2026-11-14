//! Enter the manager-selected namespace before executing the Android client.
use scarlet_os::{
    Handle,
    environment::{Environment, HandleMapping, MountNamespace},
};

pub fn run(args: &[String]) -> Result<(), &'static str> {
    if args.len() != 3 {
        return Err("input bridge launch arguments");
    }
    let values: Vec<u32> = args
        .iter()
        .map(|value| value.parse())
        .collect::<Result<_, _>>()
        .map_err(|_| "input bridge launch numbers")?;
    if values[0] < 3
        || values[1] < 3
        || values[0] == values[1]
        || values[..2].iter().any(|value| *value > i32::MAX as u32)
    {
        return Err("input bridge launch handles");
    }
    // SAFETY: the launcher transfers distinct namespace and reader descriptors.
    let namespace = unsafe { Handle::from_raw(values[0] as i32) }
        .map_err(|_| "input bridge namespace handle")?;
    let namespace =
        MountNamespace::from_handle(namespace).map_err(|_| "input bridge namespace type")?;
    let reader =
        unsafe { Handle::from_raw(values[1] as i32) }.map_err(|_| "input bridge reader handle")?;
    let environment = Environment::current().map_err(|_| "input bridge Environment")?;
    namespace
        .join()
        .map_err(|_| "input bridge namespace join")?;
    scarlet_os::shield::Policy::current()
        .map_err(|_| "input bridge policy")?
        .transition(values[2])
        .map_err(|_| "input bridge subject")?;
    let android = environment
        .root("android-aarch64")
        .map_err(|_| "input bridge Android View")?;
    let executable = android
        .open("/system/bin/app_process64", 0)
        .map_err(|_| "input bridge executable")?;
    // Standard streams remain available to report an unsuccessful exec. The
    // borrowed inherited descriptors are not closed by these wrappers.
    let stdio = [0, 1, 2].map(|fd| {
        unsafe { Handle::from_raw(fd) }
            .map(core::mem::ManuallyDrop::new)
            .map_err(|_| "input bridge standard stream")
    });
    let [stdin, stdout, stderr] = stdio;
    let (stdin, stdout, stderr) = (stdin?, stdout?, stderr?);
    let handles = [
        HandleMapping {
            source: &stdin,
            target: 0,
        },
        HandleMapping {
            source: &stdout,
            target: 1,
        },
        HandleMapping {
            source: &stderr,
            target: 2,
        },
        HandleMapping {
            source: &reader,
            target: 3,
        },
    ];
    let env: Vec<String> = std::env::vars()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let env: Vec<_> = env.iter().map(String::as_str).collect();
    eprintln!("SCARLET_ANDROID_INPUT_BRIDGE_NAMESPACE_JOINED");
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
        .map_err(|_| "input bridge Android exec")?;
    Ok(())
}
