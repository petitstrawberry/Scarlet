#![no_std]
#![no_main]

extern crate scarlet_std as std;
mod bootstrap;

use std::{environment::HandleMapping, println, vec::Vec};

fn boot() -> Result<core::convert::Infallible, &'static str> {
    let stdio = bootstrap::console()?;
    let args = std::env::args_vec();
    let cmdline = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let backing = bootstrap::backing(cmdline, false)?;
    let (environment, views) = bootstrap::environment(backing)?;
    // Distribution policy is independent of CPU width. A console/recovery
    // image can explicitly select its first program while using the same
    // sealed Environment and handle setup as the full service-manager image.
    let program = bootstrap::cmdline_value(cmdline, "init.exec=").unwrap_or("/bin/stemd");
    if !program.starts_with('/') || program.as_bytes().contains(&0) {
        return Err("init.exec must name an absolute executable path");
    }
    let executable = views[0]
        .open(program, 0)
        .map_err(|_| "cannot open initial program")?;
    let mut handles: Vec<_> = stdio
        .iter()
        .enumerate()
        .map(|(target, source)| HandleMapping {
            source,
            target: target as u32,
        })
        .collect();
    // Retain construction authority in PID 1. These handles remain CLOEXEC, so
    // ordinary service execs do not inherit them.
    handles.extend(views.iter().enumerate().map(|(index, view)| HandleMapping {
        source: view.as_handle(),
        target: index as u32 + 3,
    }));
    println!("init: starting {} in the default Environment", program);
    environment
        .exec(
            &executable,
            &[program],
            &["PATH=/bin:/usr/bin", "HOME=/root"],
            "/",
            &handles,
        )
        .map_err(|_| "Environment exec of initial program failed")
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    if let Err(error) = boot() {
        println!("init: {}", error);
    }
    // A failed transition leaves the bootstrap alive; never start a partial env.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
