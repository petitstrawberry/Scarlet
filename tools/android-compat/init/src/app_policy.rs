//! Native Android manager: retain the administrator capability outside Android
//! exec, and translate the package manager's installed-UID inventory into exact
//! label delegations. Android processes receive no management capability.
use core::sync::atomic::{AtomicBool, Ordering};
use scarlet_android_app_policy::Contexts;
use std::{
    collections::{BTreeMap, BTreeSet},
    environment::VfsView,
    shield::{FileCreate, FileRelabel, Policy, Rule, Transition},
    string::String,
    sync::Arc,
    vec::Vec,
};

fn read(view: &VfsView, path: &str, bound: usize) -> Result<Option<String>, &'static str> {
    let Ok(file) = view.open(path, 0) else {
        return Ok(None);
    };
    let stream = file.as_stream().map_err(|_| "manager input stream")?;
    let mut bytes = std::vec![0;bound];
    let mut length = 0;
    loop {
        if length == bound {
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
    labels: BTreeMap<String, u32>,
    next: u32,
    seen: BTreeSet<(String, u32, String)>,
    data_labels: BTreeSet<u32>,
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
            let file = self.label(&mapping.file, &mut added)?;
            subjects.insert(domain);
            files.insert(file);
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
        let mut rules = Vec::new();
        // Discovery of public framework endpoints; each service retains its
        // own Android permissions and caller-UID checks. Applications cannot add.
        for source in subjects {
            for name in [
                "activity",
                "activity_task",
                "package",
                "package_native",
                "window",
                "display",
                "surfaceflinger",
                "gpu",
                "user",
                "permission",
                "permissionmgr",
                "permission_checker",
                "appops",
                "power",
                "content",
                "input",
                "input_method",
                "notification",
                "statusbar",
                "wallpaper",
                "alarm",
                "role",
                "clipboard",
                "content_capture",
                "legacy_permission",
                "game",
                "netstats",
                "sensorservice",
                "device_state",
                "sensor_privacy",
                "launcherapps",
            ] {
                let label = std::format!("u:object_r:{}_service:s0", name);
                if let Some(target) = self.labels.get(&label) {
                    rules.push(Rule {
                        source,
                        target: *target,
                        class: 1,
                        reserved: 0,
                        permissions: 2,
                    });
                }
            }
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
        let mut old = self.data_labels.clone();
        old.extend(files.iter().copied());
        for name in [
            "u:object_r:unlabeled:s0",
            "u:object_r:system_data_file:s0",
            "u:object_r:system_data_root_file:s0",
        ] {
            if let Some(id) = self.labels.get(name) {
                old.insert(*id);
            }
        }
        let mut relabel = Vec::new();
        let mut create = Vec::new();
        for target in files.iter().copied() {
            for parent in &old {
                for kind in [1, 2] {
                    if *parent != target {
                        for source in [installer, zygote] {
                            relabel.push(FileRelabel { source, old: *parent, target, kind });
                        }
                    }
                    create.push(FileCreate {
                        source: installer,
                        parent: *parent,
                        target,
                        kind,
                        flags: 0,
                        reserved: 0,
                    });
                }
            }
            for parent in &old {
                if *parent != target {
                    relabel.push(FileRelabel { source: zygote, old: *parent, target, kind: 3 });
                }
            }
        }
        for target in [
            "u:object_r:system_data_root_file:s0",
            "u:object_r:system_data_file:s0",
            "u:object_r:unlabeled:s0",
        ]
        .into_iter()
        .filter_map(|name| self.labels.get(name).copied())
        .filter(|target| !files.contains(target))
        {
            for parent in &old {
                if *parent != target {
                    for kind in [1, 2] {
                        relabel.push(FileRelabel { source: zygote, old: *parent, target, kind });
                    }
                    relabel.push(FileRelabel { source: zygote, old: *parent, target, kind: 3 });
                }
            }
        }
        let references: Vec<_> = added
            .iter()
            .map(|(id, name)| (*id, name.as_str()))
            .collect();
        let before = self
            .policy
            .generation()
            .map_err(|_| "manager generation query")?;
        self.policy
            .extend(
                before,
                &references,
                &rules,
                &transitions,
                &[],
                &create,
                &relabel,
            )
            .map_err(|_| "manager policy publication")?;
        let after = self
            .policy
            .generation()
            .map_err(|_| "manager committed generation")?;
        self.data_labels.extend(files);
        for (key, domain, file) in registrations {
            std::println!(
                "SCARLET_ANDROID_APP_POLICY package={} uid={} domain={} file={} generation={}",
                key.0,
                key.1,
                domain,
                file,
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
    let text =
        read(&view, "/scarlet/boot/shield.policy", 128 * 1024)?.ok_or("manager boot policy")?;
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
                labels,
                next,
                seen: BTreeSet::new(),
                data_labels: BTreeSet::new(),
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
