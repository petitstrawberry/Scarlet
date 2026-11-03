//! Native Android manager: retain the administrator capability outside Android
//! exec, and translate the package manager's installed-UID inventory into exact
//! label delegations. Android processes receive no management capability.
use core::sync::atomic::{AtomicBool, Ordering};
use scarlet_android_app_policy::{Contexts, services::Services, groups::FileBatch};
use std::{
    collections::{BTreeMap, BTreeSet},
    environment::VfsView,
    shield::{Policy, Transition, LabelSet, SetMember, LabelSelector, SetGrant, LabelSetUpdate},
    string::String,
    sync::Arc,
    vec::Vec,
};

fn read(view: &VfsView, path: &str, bound: usize) -> Result<Option<String>, &'static str> {
    let Ok(file) = view.open(path, 0) else {
        return Ok(None);
    };
    let stream = file.as_stream().map_err(|_| "manager input stream")?;
    let mut bytes = std::vec![0;bound + 1];
    let mut length = 0;
    loop {
        if length > bound {
            return Err("manager input bound");
        }
        let count = stream
            .read(&mut bytes[length..])
            .map_err(|_| "manager input read")?;
        if count == 0 {
            break;
        }
        length += count;
    }
    bytes.truncate(length);
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "manager input encoding")
}
struct Manager {
    policy: Policy,
    contexts: Contexts,
    services: Services,
    labels: BTreeMap<String, u32>,
    next: u32,
    seen: BTreeSet<(String, u32, String)>,
    data_labels: BTreeSet<u32>,
    domain_sets:BTreeMap<String,u32>,
    next_set:u32,
}
impl Manager {
    fn label(&mut self, name: &str, added: &mut Vec<(u32, String)>) -> Result<u32, &'static str> {
        if let Some(id) = self.labels.get(name) {
            return Ok(*id);
        }
        let id = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or("manager label identity exhaustion")?;
        self.labels.insert(name.into(), id);
        added.push((id, name.into()));
        Ok(id)
    }
    fn register(&mut self, text: &str) -> Result<(), &'static str> {
        if !text.ends_with('\n') {
            return Err("incomplete package inventory");
        }
        let mut added = Vec::new();
        let mut transitions = BTreeSet::new();
        let mut files = BTreeSet::new();
        let mut subjects = BTreeSet::new();
        let mut registrations = Vec::new();
        let mut seen_names = BTreeSet::new();
        for (index, line) in text.lines().enumerate() {
            if index >= 1024 {
                return Err("package inventory bound");
            }
            let fields: Vec<_> = line.split_ascii_whitespace().collect();
            if fields.len() < 5 || !seen_names.insert(fields[0]) {
                return Err("package inventory record");
            }
            let uid = fields[1].parse::<u32>().map_err(|_| "package UID")?;
            let key = (String::from(fields[0]), uid, String::from(fields[4]));
            if self.seen.contains(&key) {
                continue;
            }
            let mapping = self
                .contexts
                .classify(fields[0], uid, fields[4])
                .map_err(|error| {
                    std::println!(
                        "SCARLET_ANDROID_APP_POLICY_REJECT package={} uid={} seinfo={} reason={}",
                        fields[0],
                        uid,
                        fields[4],
                        error
                    );
                    error
                })?;
            let domain = self.label(&mapping.domain, &mut added)?;
            subjects.insert(domain);
            if let Some(name) = &mapping.file {
                files.insert(self.label(name, &mut added)?);
            }
            transitions.insert((domain, uid));
            registrations.push((key, mapping.domain, mapping.file));
        }
        if registrations.is_empty() {
            return Ok(());
        }
        let zygote = *self
            .labels
            .get("u:r:zygote:s0")
            .ok_or("manager Zygote subject")?;
        let installer = *self
            .labels
            .get("u:r:installd:s0")
            .ok_or("manager installer subject")?;
        let mut sets = Vec::new();
        let mut members = Vec::new();
        let mut rules = Vec::new();
        // The image's compiled policy supplies exact domain-specific rules.
        // No privileged-app blanket grants or invented service-name list.
        for &source in &subjects {
            let context = self.labels.iter().find(|(_, id)| **id == source)
                .map(|(name, _)| name.clone()).ok_or("manager subject label")?;
            let grants: Vec<_> = self.services.grants(&context)?.iter()
                .map(|grant| (grant.target.clone(), grant.class, grant.permissions)).collect();
            let domain = context.split(':').nth(2).ok_or("manager domain type")?;
            let group = if let Some(id) = self.domain_sets.get(domain) { *id } else {
                let id = self.next_set;
                self.next_set = self.next_set.checked_add(1).ok_or("manager label set identity")?;
                sets.push(LabelSet {id,reserved:0});
                for (name,class,permissions) in grants {
                    let target=self.label(&name,&mut added)?;
                    rules.push(SetGrant {source:LabelSelector::set(id),target:LabelSelector::label(target),class,reserved:0,permissions});
                }
                self.domain_sets.insert(String::from(domain),id);
                id
            };
            members.push(SetMember {set:group,label:source});
        }
        let transitions: Vec<_> = transitions
            .into_iter()
            .map(|(target, principal)| Transition {
                source: zygote,
                target,
                principal,
                reserved: 0,
            })
            .collect();
        let mut base = BTreeSet::new();
        for name in [
            "u:object_r:unlabeled:s0",
            "u:object_r:system_data_file:s0",
            "u:object_r:system_data_root_file:s0",
        ] {
            if let Some(id) = self.labels.get(name) {
                base.insert(*id);
            }
        }
        let batch = FileBatch::new(&self.data_labels,&files,&base,self.next_set,installer,zygote)?;
        self.next_set=batch.next_set;
        sets.extend(batch.sets);
        members.extend(batch.members);
        let references: Vec<_> = added
            .iter()
            .map(|(id, name)| (*id, name.as_str()))
            .collect();
        let before = self
            .policy
            .generation()
            .map_err(|_| "manager generation query")?;
        self.policy
            .extend_with_sets(
                before,
                &references,
                &[],
                &transitions,
                &[],
                &[],
                &[],
                LabelSetUpdate {sets:&sets,members:&members,grants:&rules,files:&batch.rules},
            )
            .map_err(|error| {
                std::println!("SCARLET_ANDROID_APP_POLICY_UPDATE_FAIL generation={} labels={} rules={} transitions={} sets={} members={} file_rules={} error={:?}",
                    before, references.len(), rules.len(), transitions.len(), sets.len(), members.len(), batch.rules.len(), error);
                "manager policy publication"
            })?;
        let after = self
            .policy
            .generation()
            .map_err(|_| "manager committed generation")?;
        std::println!("SCARLET_ANDROID_APP_POLICY_SETS generation={} sets={} members={} shared_grants={} file_rules={}",
            after,sets.len(),members.len(),rules.len(),batch.rules.len());
        self.data_labels.extend(files);
        for (key, domain, file) in registrations {
            std::println!(
                "SCARLET_ANDROID_APP_POLICY package={} uid={} domain={} file={} generation={}",
                key.0,
                key.1,
                domain,
                file.as_deref().unwrap_or("<no app-data rule>"),
                after
            );
            self.seen.insert(key);
        }
        Ok(())
    }
}
pub struct Worker {
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle>,
}
impl Worker {
    pub fn finish(mut self) -> Result<(), &'static str> {
        self.stop.store(true, Ordering::Release);
        self.thread
            .take()
            .unwrap()
            .join()
            .map_err(|_| "manager thread join")?;
        if self.failed.load(Ordering::Acquire) {
            Err("Android app policy manager failed")
        } else {
            Ok(())
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
pub fn start(policy: Policy, android: &VfsView) -> Result<Worker, &'static str> {
    let view = VfsView::from_handle(
        android
            .as_handle()
            .duplicate()
            .map_err(|_| "manager view capability")?,
    )
    .map_err(|_| "manager view")?;
    let contexts = read(&view, "/system/etc/selinux/plat_seapp_contexts", 128 * 1024)?
        .ok_or("manager seapp input")?;
    let contexts = Contexts::parse(&contexts)?;
    let services = read(&view, "/scarlet/boot/service-policy.txt", 1024 * 1024)?
        .ok_or("manager compiled service policy missing")?;
    let services = Services::parse(&services)?;
    let text =
        read(&view, "/scarlet/boot/shield.policy", crate::boot_profile::MAX_POLICY_BYTES)?.ok_or("manager boot policy")?;
    let mut labels = BTreeMap::new();
    let mut next = 1u32;
    for line in text.lines() {
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        if let ["label", id, name] = fields.as_slice() {
            let id = id.parse::<u32>().map_err(|_| "manager label number")?;
            next = next.max(id.checked_add(1).ok_or("manager label number")?);
            labels.insert(String::from(*name), id);
        }
    }
    policy
        .generation()
        .map_err(|_| "manager update capability API")?;
    let stop = Arc::new(AtomicBool::new(false));
    let failed = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread_failed = failed.clone();
    let thread = std::thread::Builder::new()
        .name("android-policy-manager")
        .spawn(move || {
            let mut manager = Manager {
                policy,
                contexts,
                services,
                labels,
                next,
                seen: BTreeSet::new(),
                data_labels: BTreeSet::new(),
                domain_sets:BTreeMap::new(),
                next_set:1,
            };
            let mut previous = String::new();
            while !thread_stop.load(Ordering::Acquire) {
                let result = (|| -> Result<(), &'static str> {
                    if let Some(text) = read(&view, "/data/system/packages.list", 1024 * 1024)? {
                        if text != previous {
                            manager.register(&text)?;
                            previous = text;
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    std::println!("SCARLET_ANDROID_APP_POLICY_FAIL {}", error);
                    thread_failed.store(true, Ordering::Release);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        })
        .map_err(|_| "manager thread")?;
    std::println!("SCARLET_ANDROID_APP_POLICY_MANAGER_STARTED");
    Ok(Worker {
        stop,
        failed,
        thread: Some(thread),
    })
}
