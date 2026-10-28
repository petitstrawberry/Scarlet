#![no_std]
#![no_main]

extern crate scarlet_std as std;
#[path = "../../../../user/bin/src/bootstrap.rs"]
mod bootstrap;

use std::{environment::HandleMapping, println};

fn check(stdio: &[std::handle::Handle; 3]) -> Result<(), &'static str> {
    let backing = bootstrap::backing("root=/dev/vblk1", true)?;
    let (environment, _) = bootstrap::environment(backing)?;
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
    let mut failed = false;
    let programs: &[&str] = if option_env!("SCARLET_ANDROID_DYNAMIC") == Some("1") {
        &["/bin/bionic-static", "/bin/bionic-dynamic"]
    } else {
        &["/bin/bionic-static"]
    };
    for program in programs {
        let executable = android.open(program, 0).map_err(|_| "probe executable")?;
        for case in [
            "libc", "thread", "shared", "signal", "dynamic", "exec", "binder",
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
    match check(&stdio) {
        Ok(()) => println!("SCARLET_ANDROID_DIRECT_OK"),
        Err(error) => {
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
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
