//! Userspace composition of stock servicemanager and labeled Java peers.
use std::{
    environment::{Environment, HandleMapping, VfsView},
    println,
    shield::Policy,
};

pub fn run(
    env: &Environment,
    view: &VfsView,
    handles: &[HandleMapping<'_>],
    policy: Policy,
) -> Result<(), &'static str> {
    let launch: std::vec::Vec<&str> = option_env!("SCARLET_ART_ARGS")
        .unwrap_or("")
        .split('\u{1f}')
        .collect();
    let environment: std::vec::Vec<&str> = option_env!("SCARLET_ART_ENV")
        .unwrap_or("")
        .split('\u{1f}')
        .collect();
    if launch.first().copied() != Some("/system/bin/app_process64")
        || launch.last().copied() != Some("ScarletFrameworkService")
    {
        return Err("Java service launch");
    }
    let mut children = std::vec::Vec::new();
    for (label, mode) in [(1, "manager"), (2, "owner"), (3, "client"), (4, "denied")] {
        policy.apply(label).map_err(|_| "Java service label")?;
        let args = if mode == "manager" {
            std::vec!["/system/bin/servicemanager"]
        } else {
            let mut args = launch.clone();
            args.push(mode);
            args
        };
        let file = view
            .open(args[0], 0)
            .map_err(|_| "Java service executable")?;
        let pid = env
            .spawn_with_abi("android-aarch64", &file, &args, &environment, "/", handles)
            .map_err(|_| "Java service spawn")?;
        children.push((mode, pid));
        println!("SCARLET_FRAMEWORK_SERVICES_SPAWN {} {}", mode, pid);
    }
    drop(policy);
    let mut passed = true;
    for &(mode, pid) in &children[1..] {
        let (waited, status) = std::task::waitpid(pid as i32, 0);
        println!("SCARLET_FRAMEWORK_SERVICES_EXIT {} {}", mode, status);
        passed &= waited == pid as i32 && status == 0;
    }
    let manager = children[0].1;
    let stopped =
        unsafe { std::syscall::syscall2(std::syscall::Syscall::Kill, manager as usize, 9) };
    let (waited, status) = std::task::waitpid(manager as i32, 0);
    println!("SCARLET_FRAMEWORK_SERVICES_EXIT manager {}", status);
    if !passed || stopped != 0 || waited != manager as i32 || status == 0 {
        return Err("Java service integration or shutdown");
    }
    println!("SCARLET_FRAMEWORK_SERVICES_OK");
    Ok(())
}
