//! Explicit userspace policy loaded from the manager's verified boot inputs.
use std::{environment::VfsView, shield::{Policy, Rule, Transition, ExecTransition, FileCreate, FileRelabel, LabelSet, SetMember, SetFileRule, LabelSelector, LabelSetUpdate}, vec::Vec};

// Stock service rules exceed the original fixture size. Native record counts
// are checked independently below before the policy is installed.
pub const MAX_POLICY_BYTES: usize = 1024 * 1024;
const SUPPORTED_POLICY_VERSION: u32 = 30;

pub fn policy(android: &VfsView) -> Result<(Policy, Option<u32>), &'static str> {
    let file = android.open("/scarlet/boot/shield.policy", 0).map_err(|_| "boot policy open")?;
    let stream = file.as_stream().map_err(|_| "boot policy stream")?;
    let mut bytes = std::vec![0; MAX_POLICY_BYTES + 1];
    let mut length = 0;
    loop {
        if length == bytes.len() { return Err("boot policy size bound"); }
        let count = stream.read(&mut bytes[length..]).map_err(|_| "boot policy read")?;
        if count == 0 { break; }
        length += count;
        if length > MAX_POLICY_BYTES { return Err("boot policy size bound"); }
    }
    let text = core::str::from_utf8(&bytes[..length]).map_err(|_| "boot policy encoding")?;
    let mut lines = text.lines();
    if !matches!(lines.next(), Some("scarlet-android-boot-policy-v1" | "scarlet-android-boot-policy-v2")) { return Err("boot policy version"); }
    let mut labels = Vec::new();
    let mut rules = Vec::new();
    let mut transitions = Vec::new();
    let mut exec_transitions = Vec::new();
    let mut files = Vec::new();
    let mut policy_version = None;
    let mut file_default=None;let mut create_rules=Vec::new();let mut relabel_rules=Vec::new();
    let mut label_sets=Vec::new();let mut set_members=Vec::new();let mut set_files=Vec::new();
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
            ["socket-create",_,_] => rules.push(Rule {source:number(1)?,target:number(2)?,class:std::shield::SOCKET_LABEL_CLASS,reserved:0,permissions:std::shield::SOCKET_LABEL_CREATE}),
            ["service-allow", _, _, _] => rules.push(Rule {source:number(1)?,target:number(2)?,class:1,reserved:0,permissions:number(3)? as u64}),
            ["keystore-allow", _, _, _] => rules.push(Rule {source:number(1)?,target:number(2)?,class:5,reserved:0,permissions:number(3)? as u64}),
            ["transition", _, _, _] => transitions.push(Transition {
                source: number(1)?, target: number(2)?, principal: number(3)?, reserved: 0,
            }),
            ["file-exec", _, _, _, _] => exec_transitions.push(ExecTransition {
                source:number(1)?,resource:number(2)?,target:number(3)?,principal:number(4)?,reserved:0,
            }),
            ["file-label", _, path] => files.push((number(1)?, *path)),
            ["file-default", _] => {if file_default.replace(number(1)?).is_some() {return Err("duplicate file default");}},
            ["file-create",_,_,_,_,_] => create_rules.push(FileCreate {source:number(1)?,parent:number(2)?,target:number(3)?,kind:number(4)?,flags:number(5)?,reserved:0}),
            ["file-relabel",_,_,_,_] => relabel_rules.push(FileRelabel {source:number(1)?,old:number(2)?,target:number(3)?,kind:number(4)?}),
            ["label-set",_] => label_sets.push(LabelSet {id:number(1)?,reserved:0}),
            ["set-member",_,_] => set_members.push(SetMember {set:number(1)?,label:number(2)?}),
            ["file-set",_,_,_,_,_,_] => set_files.push(SetFileRule {source:LabelSelector::label(number(1)?),origin:LabelSelector::set(number(2)?),target:LabelSelector::set(number(3)?),kind:number(4)?,action:number(5)?,flags:number(6)?,reserved:0}),
            ["policy-version", _] => {
                let version = number(1)?;
                if version != SUPPORTED_POLICY_VERSION || policy_version.replace(version).is_some() { return Err("boot policy version metadata"); }
            },
            _ => return Err("unknown boot policy record"),
        }
        // Keep in sync with the native Shield bounds; Android 13's stock
        // service inventory needs more exact labels than the original 512.
        if labels.len() > std::shield::MAX_LABELS || rules.len() > std::shield::MAX_RULES || transitions.len() > std::shield::MAX_RULES || exec_transitions.len() > std::shield::MAX_RULES || create_rules.len()>std::shield::MAX_RULES || relabel_rules.len()>std::shield::MAX_RULES || files.len() > std::shield::MAX_RULES { return Err("boot policy record bound"); }
        if label_sets.len()>std::shield::MAX_LABEL_SETS || set_members.len()>std::shield::MAX_SET_MEMBERS || set_files.len()>std::shield::MAX_RULES {return Err("boot policy set bound");}
    }
    if !labels.contains(&(1, "u:r:init:s0")) { return Err("boot init subject missing"); }
    let policy = Policy::create_with_exec_transitions(&labels, &rules, &transitions, &exec_transitions).map_err(|_| "boot Shield policy")?;
    let policy=if let Some(default)=file_default {policy.with_file_rules(default,&create_rules,&relabel_rules).map_err(|_| "boot file policy")?}else {policy};
    if !label_sets.is_empty() || !set_members.is_empty() || !set_files.is_empty() {
        let generation=policy.generation().map_err(|_|"boot policy generation")?;
        policy.extend_with_sets(generation,&[],&[],&[],&[],&[],&[],LabelSetUpdate {sets:&label_sets,members:&set_members,grants:&[],files:&set_files}).map_err(|_|"boot policy sets")?;
    }
    std::println!("SCARLET_ANDROID_FILE_POLICY create={} relabel={}",create_rules.len(),relabel_rules.len());
    for (label, path) in &files {
        let file = android.open(path, 0).map_err(|_| "boot file-label open")?;
        policy.bind_file(&file, *label).map_err(|_| "boot file-label registration")?;
    }
    std::println!("SCARLET_ANDROID_BOOT_FILES labels={} exec_rules={}", files.len(), exec_transitions.len());
    std::println!("SCARLET_ANDROID_BOOT_POLICY labels={} property_grants={} transitions={}", labels.len(), rules.len(), transitions.len());
    Ok((policy, policy_version))
}
