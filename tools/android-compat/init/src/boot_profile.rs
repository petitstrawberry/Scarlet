//! Explicit userspace policy loaded from the manager's verified boot inputs.
use std::{environment::VfsView, shield::{Policy, Rule, Transition, ExecTransition}, vec::Vec};

pub fn policy(android: &VfsView) -> Result<Policy, &'static str> {
    let file = android.open("/scarlet/boot/shield.policy", 0).map_err(|_| "boot policy open")?;
    let stream = file.as_stream().map_err(|_| "boot policy stream")?;
    let mut bytes = std::vec![0; 128 * 1024];
    let mut length = 0;
    loop {
        if length == bytes.len() { return Err("boot policy size bound"); }
        let count = stream.read(&mut bytes[length..]).map_err(|_| "boot policy read")?;
        if count == 0 { break; }
        length += count;
    }
    let text = core::str::from_utf8(&bytes[..length]).map_err(|_| "boot policy encoding")?;
    let mut lines = text.lines();
    if !matches!(lines.next(), Some("scarlet-android-boot-policy-v1" | "scarlet-android-boot-policy-v2")) { return Err("boot policy version"); }
    let mut labels = Vec::new();
    let mut rules = Vec::new();
    let mut transitions = Vec::new();
    let mut exec_transitions = Vec::new();
    let mut files = Vec::new();
    for line in lines {
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        let number = |index: usize| -> Result<u32, &'static str> {
            fields.get(index).ok_or("boot policy field")?.parse().map_err(|_| "boot policy number")
        };
        match fields.as_slice() {
            ["label", _, name] => labels.push((number(1)?, *name)),
            ["property-set", _, _] => rules.push(Rule {
                source: number(1)?, target: number(2)?, class: 2, reserved: 0, permissions: 1,
            }),
            ["transition", _, _, _] => transitions.push(Transition {
                source: number(1)?, target: number(2)?, principal: number(3)?, reserved: 0,
            }),
            ["file-exec", _, _, _, _] => exec_transitions.push(ExecTransition {
                source:number(1)?,resource:number(2)?,target:number(3)?,principal:number(4)?,reserved:0,
            }),
            ["file-label", _, path] => files.push((number(1)?, *path)),
            _ => return Err("unknown boot policy record"),
        }
        if labels.len() > 512 || rules.len() > 4096 || transitions.len() > 4096 || exec_transitions.len() > 4096 || files.len() > 4096 { return Err("boot policy record bound"); }
    }
    if !labels.contains(&(1, "u:r:init:s0")) { return Err("boot init subject missing"); }
    let policy = Policy::create_with_exec_transitions(&labels, &rules, &transitions, &exec_transitions).map_err(|_| "boot Shield policy")?;
    for (label, path) in &files {
        let file = android.open(path, 0).map_err(|_| "boot file-label open")?;
        policy.bind_file(&file, *label).map_err(|_| "boot file-label registration")?;
    }
    std::println!("SCARLET_ANDROID_BOOT_FILES labels={} exec_rules={}", files.len(), exec_transitions.len());
    std::println!("SCARLET_ANDROID_BOOT_POLICY labels={} property_grants={} transitions={}", labels.len(), rules.len(), transitions.len());
    Ok(policy)
}
