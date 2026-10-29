//! Userspace orchestration fixture: one stock manager and three labeled peers.
use std::{
    environment::{Environment, HandleMapping, VfsView},
    println,
    shield::{Policy, Rule},
};
pub fn configure(android: &VfsView) -> Result<Policy, &'static str> {
    let labels = [
        (1, "u:r:scarlet_manager:s0"),
        (2, "u:r:scarlet_publisher:s0"),
        (3, "u:r:scarlet_client:s0"),
        (4, "u:r:scarlet_denied:s0"),
        (5, "u:object_r:scarlet_service:s0"),
        (6, "u:object_r:scarlet_manager_service:s0"),
    ];
    let rules: std::vec::Vec<Rule> = [
        (1, 6, 3),
        (2, 5, 3),
        (2, 6, 2),
        (3, 5, 2),
        (3, 6, 2),
        (3, 1, 4),
    ]
    .iter()
    .map(|&(source, target, permissions)| Rule {
        source,
        target,
        class: 1,
        reserved: 0,
        permissions,
    })
    .collect();
    let policy = Policy::create(&labels, &rules).map_err(|_| "service policy")?;
    let fs = VfsView::create(
        "android-selinux",
        &std::format!("policy={}", policy.as_handle().as_raw()),
    )
    .map_err(|_| "service security view")?;
    android
        .bind("/sys/fs/selinux", &fs, "/")
        .map_err(|_| "service security mount")?;
    policy.apply(1).map_err(|_| "manager label")?;
    println!("\nSCARLET_SERVICES_CONFIGURED");
    Ok(policy)
}
pub fn run(
    env: &Environment,
    android: &VfsView,
    handles: &[HandleMapping<'_>],
    policy: Policy,
) -> Result<(), &'static str> {
    let mut children = std::vec::Vec::new();
    for (label, mode) in [(1, "manager"), (2, "owner"), (3, "client"), (4, "denied")] {
        policy.apply(label).map_err(|_| "service child label")?;
        let path = if mode == "manager" {
            "/system/bin/servicemanager"
        } else {
            "/system/bin/scarlet-program-probe"
        };
        let file = android.open(path, 0).map_err(|_| "service executable")?;
        let args = if mode == "manager" {
            std::vec![path]
        } else {
            std::vec![path, mode]
        };
        let pid = env
            .spawn_with_abi(
                "android-aarch64",
                &file,
                &args,
                &["ANDROID_LOG_TAGS=*:v"],
                "/",
                handles,
            )
            .map_err(|_| "service spawn")?;
        children.push((mode, pid));
        println!("SCARLET_SERVICES_SPAWN {} {}", mode, pid);
    }
    drop(policy);
    let _logger = std::thread::Builder::new()
        .spawn(|| {
            if let Ok(mut log) = std::fs::File::open("/dev/kmsg") {
                let mut bytes = [0; 8192];
                while let Ok(n) = log.read(&mut bytes) {
                    if n == 0 {
                        break;
                    }
                    std::print!("{}", std::string::String::from_utf8_lossy(&bytes[..n]));
                }
            }
        })
        .map_err(|_| "service log thread")?;
    let mut passed = true;
    for &(mode, pid) in &children[1..] {
        let (waited, status) = std::task::waitpid(pid as i32, 0);
        println!("SCARLET_SERVICES_EXIT {} {}", mode, status);
        passed &= waited == pid as i32 && status == 0;
    }
    let manager = children[0].1;
    // Terminate only the manager this fixture spawned, then reap it.
    let stopped = unsafe { std::syscall::syscall2(std::syscall::Syscall::Kill, manager as usize, 9) };
    let (waited, status) = std::task::waitpid(manager as i32, 0);
    println!("SCARLET_SERVICES_EXIT manager {}", status);
    if !passed || stopped != 0 || waited != manager as i32 || status == 0 {
        return Err("service integration or shutdown");
    }
    println!("SCARLET_SERVICES_OK");
    Ok(())
}
