//! Demonstrate a native-only Environment using a view the caller can access.
//!
//! Filesystem data is shared; this example does not claim filesystem, PID,
//! network or IPC isolation. Private roots require delegated view-management
//! authority and explicit view construction, not unprivileged pivot_root.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{
    environment::{Environment, HandleMapping},
    handle::{Handle, HandleError, HandleResult},
    println,
    syscall::{Syscall, syscall1},
};

fn stdio(raw: usize) -> HandleResult<Handle> {
    // SAFETY: duplication borrows this descriptor for the syscall only.
    let duplicate = unsafe { syscall1(Syscall::HandleDuplicate, raw) };
    if duplicate == usize::MAX {
        return Err(HandleError::SystemError(-1));
    }
    // SAFETY: the syscall returned a new, exclusively owned descriptor.
    unsafe { Handle::from_raw(duplicate as i32) }
}

fn run() -> HandleResult<i32> {
    let current = Environment::current()?;
    let native = current.root("scarlet")?;
    let executable = native.open("/bin/hello", 0)?;
    let environment = Environment::create()?;
    environment.set_root("scarlet", &native)?;
    environment.seal()?;

    let streams = [stdio(0)?, stdio(1)?, stdio(2)?];
    let handles = [
        HandleMapping {
            source: &streams[0],
            target: 0,
        },
        HandleMapping {
            source: &streams[1],
            target: 1,
        },
        HandleMapping {
            source: &streams[2],
            target: 2,
        },
    ];
    println!("Creating a native-only Environment; filesystem data remains shared.");
    let pid = environment.spawn(
        &executable,
        &["/bin/hello"],
        &["PATH=/bin:/usr/bin", "HOME=/root"],
        "/",
        &handles,
    )?;
    let (waited, status) = std::task::waitpid(pid as i32, 0);
    if waited < 0 {
        return Err(HandleError::SystemError(-1));
    }
    println!("Environment child {} exited with status {}", pid, status);
    Ok(status)
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    match run() {
        Ok(status) => status,
        Err(error) => {
            println!("container-demo: {:?}", error);
            1
        }
    }
}
