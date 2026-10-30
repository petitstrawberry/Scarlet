#![no_std]
#![no_main]

extern crate scarlet_std as std;
#[path = "../../../../user/bin/src/bootstrap.rs"]
mod bootstrap;
mod boot_profile;
mod app_policy;
mod framework_services;
mod services;
mod native_sws;
mod shield;
mod stock;

use std::{environment::HandleMapping, println};

fn check(stdio: &[std::handle::Handle; 3]) -> Result<(), &'static str> {
    if option_env!("SCARLET_ANDROID_INIT_BOOTSTRAP") == Some("1") {
        std::vm::configure_layout(24,16).map_err(|_| "Android address layout")?;
        println!("SCARLET_ANDROID_MMAP_RANDOMIZED");
        std::shield::hide_kernel_addresses().map_err(|_| "Shield kernel address constraint")?;
        println!("SCARLET_SHIELD_KERNEL_ADDRESSES_HIDDEN");
        let resource = std::resource_group::ResourceGroup::create_root().map_err(|_| "Android resource root")?;
        resource.enter(0).map_err(|_| "Android resource membership")?;
        println!("SCARLET_ANDROID_RESOURCE_ROOT");
    }
    let backing = bootstrap::backing("root=/dev/vblk1", true)?;
    let (environment, policy,boot_policy) = if option_env!("SCARLET_ANDROID_STOCK") == Some("1") {
        stock::environment(backing)?
    } else {
        (bootstrap::environment(backing)?.0, None,None)
    };
    let android = environment
        .root("android-aarch64")
        .map_err(|_| "Android view")?;
    let handles = [
        HandleMapping {
            source: &stdio[0],
            target: 0,
        },
        HandleMapping {
            source: &stdio[1],
            target: 1,
        },
        HandleMapping {
            source: &stdio[2],
            target: 2,
        },
    ];
    let _compositor = native_sws::Compositor::start(&environment, &handles)?;
    if let Some(policy) = policy {
        stock::prepare(&environment, &android, &handles)?;
        return if option_env!("SCARLET_ANDROID_FRAMEWORK_SERVICES") == Some("1") {
            framework_services::run(&environment, &android, &handles, policy)
        } else {
            services::run(&environment, &android, &handles, policy)
        };
    }
    if option_env!("SCARLET_ANDROID_ART") == Some("1") {
        // Keep startup faults visible even when ART never reaches Java or exit.
        // The host runner owns the timeout and the private QEMU process.
        let _logger = if option_env!("SCARLET_ANDROID_INIT_BOOTSTRAP") == Some("1") { None } else { Some(std::thread::Builder::new()
            .spawn(|| {
                let Ok(mut log) = std::fs::File::open("/dev/kmsg") else {
                    println!("SCARLET_ANDROID_ART_FAIL: kernel log open");
                    return;
                };
                let mut bytes = [0; 8192];
                while let Ok(count) = log.read(&mut bytes) {
                    if count == 0 {
                        break;
                    }
                    std::print!("{}", std::string::String::from_utf8_lossy(&bytes[..count]));
                }
            })
            .map_err(|_| "kernel log thread")?) };
        if option_env!("SCARLET_ANDROID_STOCK") == Some("1") {
            stock::prepare(&environment, &android, &handles)?;
        }
        let diagnostic = option_env!("SCARLET_ANDROID_PROGRAM") == Some("1");
        let program = if option_env!("SCARLET_ANDROID_FRAMEWORK") == Some("1") {
            "/system/bin/app_process64"
        } else if diagnostic {
            "/system/bin/scarlet-program-probe"
        } else {
            "/apex/com.android.art/bin/dalvikvm64"
        };
        let executable = android.open(program, 0).map_err(|_| "ART executable")?;
        let argv: std::vec::Vec<&str> = option_env!("SCARLET_ART_ARGS")
            .unwrap_or("")
            .split('\u{1f}')
            .collect();
        let env: std::vec::Vec<&str> = option_env!("SCARLET_ART_ENV")
            .unwrap_or("")
            .split('\u{1f}')
            .collect();
        if argv.first().copied() != Some(program) || (!diagnostic && !argv.contains(&"-Xint")) {
            return Err("ART interpreter arguments");
        }
        let label = if diagnostic { "PROGRAM" } else { "ART" };
        let app_manager=if option_env!("SCARLET_ANDROID_APP_POLICY")==Some("1") {
            Some(app_policy::start(boot_policy.ok_or("manager policy capability missing")?,&android)?)
        } else {None};
        println!("\nSCARLET_ANDROID_{}_START", label);
        let pid = environment
            .spawn(&executable, &argv, &env, "/", &handles)
            .map_err(|_| "ART spawn")?;
        let (waited, status) = std::task::waitpid(pid as i32, 0);
        if let Some(manager)=app_manager {manager.finish()?;}
        println!("SCARLET_ANDROID_{}_EXIT {}", label, status);
        if diagnostic && waited == pid as i32 {
            return Ok(());
        }
        return if waited == pid as i32 && status == 0 {
            Ok(())
        } else {
            Err("ART wait or exit")
        };
    }
    let mut failed = false;
    let programs: &[&str] = if option_env!("SCARLET_ANDROID_DYNAMIC") == Some("1") {
        &["/bin/bionic-static", "/bin/bionic-dynamic"]
    } else {
        &["/bin/bionic-static"]
    };
    for program in programs {
        let executable = android.open(program, 0).map_err(|_| "probe executable")?;
        for case in [
            "libc", "thread", "shared", "signal", "dynamic", "exec", "xattr", "mremap", "binder",
        ] {
            if case == "binder" && option_env!("SCARLET_ANDROID_BINDER") != Some("1") {
                continue;
            }
            if case == "dynamic" && program.ends_with("static") {
                continue;
            }
            println!("\nSCARLET_ANDROID_DIRECT_CASE {} {}", program, case);
            let pid = environment
                .spawn(
                    &executable,
                    &[program, case],
                    &[
                        "PATH=/bin",
                        "LD_LIBRARY_PATH=/lib:/apex/com.android.runtime/lib64/bionic",
                    ],
                    "/",
                    &handles,
                )
                .map_err(|_| "probe spawn")?;
            let (waited, status) = std::task::waitpid(pid as i32, 0);
            println!(
                "SCARLET_ANDROID_DIRECT_EXIT {} {} {}",
                program, case, status
            );
            failed |= waited != pid as i32 || status != 0;
        }
    }
    // The same executable, explicitly selected as Linux, must not gain Binder
    // just because its libc was built by the Android NDK.
    let control = environment
        .root("linux-aarch64")
        .map_err(|_| "Linux control view")?;
    let executable = control
        .open("/bin/bionic-static", 0)
        .map_err(|_| "Linux control executable")?;
    println!("\nSCARLET_ANDROID_DIRECT_CASE /bin/bionic-static binder-absent");
    let pid = environment
        .spawn_with_abi(
            "linux-aarch64",
            &executable,
            &["/bin/bionic-static", "binder-absent"],
            &[],
            "/",
            &handles,
        )
        .map_err(|_| "Linux control spawn")?;
    let (waited, status) = std::task::waitpid(pid as i32, 0);
    println!(
        "SCARLET_ANDROID_DIRECT_EXIT /bin/bionic-static binder-absent {}",
        status
    );
    failed |= waited != pid as i32 || status != 0;
    if failed {
        Err("one or more Bionic cases failed")
    } else {
        Ok(())
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let stdio = match bootstrap::console() {
        Ok(handles) => handles,
        Err(_) => loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        },
    };
    let art = option_env!("SCARLET_ANDROID_ART") == Some("1");
    let diagnostic = option_env!("SCARLET_ANDROID_PROGRAM") == Some("1");
    match check(&stdio) {
        Ok(()) => {
            if diagnostic {
                println!("SCARLET_ANDROID_PROGRAM_DONE");
            } else if art {
                println!("SCARLET_ANDROID_ART_OK");
            } else {
                println!("SCARLET_ANDROID_DIRECT_OK");
            }
        }
        Err(error) => {
            if option_env!("SCARLET_ANDROID_FRAMEWORK_SERVICES") == Some("1") {
                println!("SCARLET_FRAMEWORK_SERVICES_FAIL: {}", error);
            } else if diagnostic {
                println!("SCARLET_ANDROID_PROGRAM_FAIL: {}", error);
            } else if art {
                println!("SCARLET_ANDROID_ART_FAIL: {}", error);
            } else {
                // Fault diagnostics normally remain in Scarlet's kernel log ring.
                // One bounded read captures them before the host sees the final marker.
                if let Ok(mut log) = std::fs::File::open("/dev/kmsg") {
                    let mut bytes = std::vec![0; 256 * 1024];
                    if let Ok(count) = log.read(&mut bytes) {
                        println!("{}", std::string::String::from_utf8_lossy(&bytes[..count]));
                    }
                }
                println!("SCARLET_ANDROID_DIRECT_FAIL: {}", error);
            }
        }
    }
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
