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
    let executable = views[0]
        .open("/bin/stemd", 0)
        .map_err(|_| "cannot open /bin/stemd")?;
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
    println!("init: starting stemd in the default Environment");
    environment
        .exec(
            &executable,
            &["/bin/stemd"],
            &["PATH=/bin:/usr/bin", "HOME=/root"],
            "/",
            &handles,
        )
        .map_err(|_| "Environment exec of /bin/stemd failed")
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
