use std::cmp::Ordering;
use std::env;
use std::fmt;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use scarlet_sys::{Syscall, syscall0, syscall1, syscall2};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const TASK_NAME_CAP: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskState {
    NotInitialized,
    Ready,
    Running,
    BlockedInterruptible,
    BlockedUninterruptible,
    Zombie,
    Terminated,
}

impl TaskState {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Ready,
            2 => Self::Running,
            3 => Self::BlockedInterruptible,
            4 => Self::BlockedUninterruptible,
            5 => Self::Zombie,
            6 => Self::Terminated,
            _ => Self::NotInitialized,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskType {
    Kernel,
    User,
}

impl TaskType {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::User,
            _ => Self::Kernel,
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct RawTaskInfo {
    pid: usize,
    ppid: usize,
    state: u8,
    task_type: u8,
    cpu_id: u8,
    _reserved: u8,
    exit_status: i32,
    tgid: usize,
    name: [u8; 64],
    cpu_time_ns: u64,
    sched_util_avg: u32,
    sched_util_min: u32,
    sched_required_capacity: u32,
    core_preference: u8,
    _reserved2: [u8; 3],
    sched_migration_count: u64,
    sched_nice: i32,
    sched_weight: u32,
    sched_vruntime: u64,
    sched_deadline: u64,
}

#[derive(Debug, Clone)]
struct TaskInfo {
    pid: usize,
    state: TaskState,
    task_type: TaskType,
    cpu: u8,
    tgid: usize,
    name: TaskName,
    cpu_time_ns: u64,
    sched_util_avg: u32,
    sched_util_min: u32,
    sched_required_capacity: u32,
    core_preference: u8,
    sched_migration_count: u64,
    sched_nice: i32,
    sched_weight: u32,
    sched_vruntime: u64,
    sched_deadline: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TaskName {
    bytes: [u8; TASK_NAME_CAP],
    len: usize,
}

impl TaskName {
    fn from_raw(raw: &[u8; TASK_NAME_CAP]) -> Self {
        let len = raw.iter().position(|&byte| byte == 0).unwrap_or(raw.len());
        let mut bytes = [0; TASK_NAME_CAP];
        bytes[..len].copy_from_slice(&raw[..len]);
        Self { bytes, len }
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or("<invalid>")
    }

    fn len(&self) -> usize {
        self.as_str().len()
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.as_str().starts_with(prefix)
    }
}

impl Ord for TaskName {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialOrd for TaskName {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for TaskName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl RawTaskInfo {
    fn decode(&self) -> TaskInfo {
        TaskInfo {
            pid: self.pid,
            state: TaskState::from_u8(self.state),
            task_type: TaskType::from_u8(self.task_type),
            cpu: self.cpu_id,
            tgid: self.tgid,
            name: TaskName::from_raw(&self.name),
            cpu_time_ns: self.cpu_time_ns,
            sched_util_avg: self.sched_util_avg,
            sched_util_min: self.sched_util_min,
            sched_required_capacity: self.sched_required_capacity,
            core_preference: self.core_preference,
            sched_migration_count: self.sched_migration_count,
            sched_nice: self.sched_nice,
            sched_weight: self.sched_weight,
            sched_vruntime: self.sched_vruntime,
            sched_deadline: self.sched_deadline,
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct RawCpuUsageInfo {
    online_cpus: usize,
    busy_time_ns: u64,
    idle_time_ns: u64,
    total_time_ns: u64,
    usage_per_mille: u32,
    _reserved: u32,
}

#[derive(Debug, Clone, Copy)]
struct CpuUsageInfo {
    online_cpus: usize,
    busy_time_ns: u64,
    idle_time_ns: u64,
}

#[derive(Clone, Copy)]
enum SortKey {
    Pid,
    PercentCpu,
    Cpu,
    Name,
    State,
    Type,
}

#[derive(Clone)]
struct TaskSample {
    task: TaskInfo,
    cpu_per_mille: u64,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--check-instant") {
        check_instant();
        return ExitCode::SUCCESS;
    }

    let mut sort_key = SortKey::PercentCpu;
    let show_idle = args.iter().any(|arg| arg == "--idle");

    for (index, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "-p" => sort_key = SortKey::Pid,
            "-n" => sort_key = SortKey::Name,
            "-c" => sort_key = SortKey::Cpu,
            "-s" => sort_key = SortKey::State,
            "-m" => sort_key = SortKey::Pid,
            "--sort" => {
                if let Some(value) = args.get(index + 1) {
                    sort_key = parse_sort_key(value);
                }
            }
            _ => {}
        }
    }

    let before_tasks = task_info();
    let before_cpu = cpu_usage();
    let sample_started_at = Instant::now();
    thread::sleep(SAMPLE_INTERVAL);
    let elapsed = sample_started_at.elapsed();
    let mut tasks = task_info();
    let after_cpu = cpu_usage();

    if !show_idle {
        tasks.retain(|task| !is_idle_task(task));
    }

    if tasks.is_empty() {
        println!("No tasks found.");
        return ExitCode::SUCCESS;
    }

    let total = tasks.len();
    let running = tasks
        .iter()
        .filter(|task| task.state == TaskState::Running)
        .count();
    let sleeping = tasks
        .iter()
        .filter(|task| task.state == TaskState::BlockedInterruptible)
        .count();
    let stopped = tasks
        .iter()
        .filter(|task| task.state == TaskState::Terminated)
        .count();
    let zombies = tasks
        .iter()
        .filter(|task| task.state == TaskState::Zombie)
        .count();
    let user_tasks = tasks
        .iter()
        .filter(|task| task.task_type == TaskType::User)
        .count();
    let kernel_tasks = tasks
        .iter()
        .filter(|task| task.task_type == TaskType::Kernel)
        .count();

    let (busy_per_mille, idle_per_mille) = cpu_per_mille(before_cpu, after_cpu);
    let elapsed_ns = elapsed.as_nanos().max(1);

    println!(
        "Tasks: {:>3} total, {:>3} running, {:>3} sleeping, {:>3} stopped, {:>3} zombie",
        total, running, sleeping, stopped, zombies
    );
    println!("       {:>3} user,  {:>3} kernel", user_tasks, kernel_tasks);
    println!(
        "%Cpu(s): {:>5} busy, {:>5} idle",
        Percent(busy_per_mille),
        Percent(idle_per_mille),
    );
    println!();

    let mut sorted: Vec<TaskSample> = tasks
        .into_iter()
        .map(|task| {
            let previous = previous_cpu_time(&before_tasks, &task).unwrap_or(task.cpu_time_ns);
            let delta = task.cpu_time_ns.saturating_sub(previous);
            let cpu_per_mille = ((delta as u128 * 1000) / elapsed_ns) as u64;
            TaskSample {
                task,
                cpu_per_mille,
            }
        })
        .collect();
    apply_sort(&mut sorted, sort_key);
    print_table(&sorted);

    ExitCode::SUCCESS
}

fn check_instant() {
    let mono_start = monotonic_time_ns();
    let instant_start = Instant::now();
    thread::sleep(SAMPLE_INTERVAL);
    let instant_elapsed = instant_start.elapsed();
    let mono_elapsed = monotonic_time_ns().saturating_sub(mono_start);

    println!(
        "Instant elapsed: {} ns ({} ms)",
        instant_elapsed.as_nanos(),
        instant_elapsed.as_millis()
    );
    println!(
        "Monotonic elapsed: {} ns ({} ms)",
        mono_elapsed,
        mono_elapsed / 1_000_000
    );
}

fn monotonic_time_ns() -> u64 {
    syscall0(Syscall::MonotonicTime) as u64
}

fn task_info() -> Vec<TaskInfo> {
    let total = syscall0(Syscall::GetTaskInfoCount);
    let mut raw = vec![
        RawTaskInfo {
            pid: 0,
            ppid: 0,
            state: 0,
            task_type: 0,
            cpu_id: 0,
            _reserved: 0,
            exit_status: 0,
            tgid: 0,
            name: [0; 64],
            cpu_time_ns: 0,
            sched_util_avg: 0,
            sched_util_min: 0,
            sched_required_capacity: 0,
            core_preference: 0,
            _reserved2: [0; 3],
            sched_migration_count: 0,
            sched_nice: 0,
            sched_weight: 0,
            sched_vruntime: 0,
            sched_deadline: 0,
        };
        total
    ];
    let written = syscall2(
        Syscall::GetTaskInfoList,
        raw.as_mut_ptr() as usize,
        raw.len(),
    );
    if written == usize::MAX {
        return Vec::new();
    }
    raw.truncate(written.min(raw.len()));
    raw.iter().map(RawTaskInfo::decode).collect()
}

fn cpu_usage() -> Option<CpuUsageInfo> {
    let mut raw = RawCpuUsageInfo {
        online_cpus: 0,
        busy_time_ns: 0,
        idle_time_ns: 0,
        total_time_ns: 0,
        usage_per_mille: 0,
        _reserved: 0,
    };
    let result = syscall1(
        Syscall::GetCpuUsageInfo,
        &mut raw as *mut RawCpuUsageInfo as usize,
    );
    if result == usize::MAX {
        None
    } else {
        Some(CpuUsageInfo {
            online_cpus: raw.online_cpus,
            busy_time_ns: raw.busy_time_ns,
            idle_time_ns: raw.idle_time_ns,
        })
    }
}

fn cpu_per_mille(before: Option<CpuUsageInfo>, after: Option<CpuUsageInfo>) -> (u64, u64) {
    let (Some(before), Some(after)) = (before, after) else {
        return (0, 0);
    };
    let _online_cpus = after.online_cpus;
    let busy_delta = after.busy_time_ns.saturating_sub(before.busy_time_ns);
    let idle_delta = after.idle_time_ns.saturating_sub(before.idle_time_ns);
    let total_delta = busy_delta.saturating_add(idle_delta);
    if total_delta == 0 {
        return (0, 0);
    }

    let busy = ((busy_delta as u128 * 1000) / total_delta as u128) as u64;
    let idle = ((idle_delta as u128 * 1000) / total_delta as u128) as u64;
    (busy, idle)
}

fn previous_cpu_time(tasks: &[TaskInfo], task: &TaskInfo) -> Option<u64> {
    tasks
        .iter()
        .find(|previous| {
            previous.pid == task.pid && previous.tgid == task.tgid && previous.name == task.name
        })
        .map(|previous| previous.cpu_time_ns)
}

fn print_table(samples: &[TaskSample]) {
    let widths = TableWidths::from_samples(samples);

    println!(
        "{:>pid$} {:>tgid$} {:<user$} {:<priority$} {:>nice$} {:>weight$} {:<state$} {:<cpu$} {:>percent$} {:>util$} {:>util_min$} {:>required$} {:<preference$} {:>migrations$} {:>vruntime$} {:>deadline$} {:>time$} {:<command$}",
        "PID",
        "TGID",
        "USER",
        "PR",
        "NI",
        "WEIGHT",
        "STAT",
        "CPU",
        "%CPU",
        "UTIL",
        "MIN",
        "REQ",
        "PREF",
        "MIG",
        "VRUNTIME",
        "DEADLINE",
        "TIME+",
        "COMMAND",
        pid = widths.pid,
        tgid = widths.tgid,
        user = widths.user,
        priority = widths.priority,
        nice = widths.nice,
        weight = widths.weight,
        state = widths.state,
        cpu = widths.cpu,
        percent = widths.percent,
        util = widths.util,
        util_min = widths.util_min,
        required = widths.required,
        preference = widths.preference,
        migrations = widths.migrations,
        vruntime = widths.vruntime,
        deadline = widths.deadline,
        time = widths.time,
        command = widths.command,
    );

    for sample in samples {
        let task = &sample.task;
        let user = match task.task_type {
            TaskType::Kernel => "K",
            TaskType::User => "U",
        };
        println!(
            "{:>pid$} {:>tgid$} {:<user$} {:<priority$} {:>nice$} {:>weight$} {:<state$} {:<cpu$} {:>percent$} {:>util$} {:>util_min$} {:>required$} {:<preference$} {:>migrations$} {:>vruntime$} {:>deadline$} {:>time$} {:<command$}",
            task.pid,
            task.tgid,
            user,
            "-",
            task.sched_nice,
            task.sched_weight,
            format_stat(task.state),
            CpuLabel(task.cpu),
            Percent(sample.cpu_per_mille),
            task.sched_util_avg,
            task.sched_util_min,
            task.sched_required_capacity,
            format_core_preference(task.core_preference),
            task.sched_migration_count,
            task.sched_vruntime,
            task.sched_deadline,
            TimeNs(task.cpu_time_ns),
            task.name,
            pid = widths.pid,
            tgid = widths.tgid,
            user = widths.user,
            priority = widths.priority,
            nice = widths.nice,
            weight = widths.weight,
            state = widths.state,
            cpu = widths.cpu,
            percent = widths.percent,
            util = widths.util,
            util_min = widths.util_min,
            required = widths.required,
            preference = widths.preference,
            migrations = widths.migrations,
            vruntime = widths.vruntime,
            deadline = widths.deadline,
            time = widths.time,
            command = widths.command,
        );
    }
}

struct TableWidths {
    pid: usize,
    user: usize,
    priority: usize,
    nice: usize,
    weight: usize,
    state: usize,
    cpu: usize,
    percent: usize,
    util: usize,
    util_min: usize,
    required: usize,
    preference: usize,
    migrations: usize,
    vruntime: usize,
    deadline: usize,
    time: usize,
    command: usize,
    tgid: usize,
}

impl TableWidths {
    fn from_samples(samples: &[TaskSample]) -> Self {
        let mut widths = Self {
            pid: "PID".len(),
            user: "USER".len(),
            priority: "PR".len(),
            nice: "NI".len(),
            weight: "WEIGHT".len(),
            state: "STAT".len(),
            cpu: "CPU".len(),
            percent: "%CPU".len(),
            util: "UTIL".len(),
            util_min: "MIN".len(),
            required: "REQ".len(),
            preference: "PREF".len(),
            migrations: "MIG".len(),
            vruntime: "VRUNTIME".len(),
            deadline: "DEADLINE".len(),
            time: "TIME+".len(),
            command: "COMMAND".len(),
            tgid: "TGID".len(),
        };

        for sample in samples {
            let task = &sample.task;
            widths.pid = widths.pid.max(decimal_digits_usize(task.pid));
            widths.nice = widths.nice.max(signed_decimal_width_i32(task.sched_nice));
            widths.weight = widths
                .weight
                .max(decimal_digits_u64(u64::from(task.sched_weight)));
            widths.state = widths.state.max(format_stat(task.state).len());
            widths.cpu = widths.cpu.max(cpu_label_width(task.cpu));
            widths.percent = widths.percent.max(percent_width(sample.cpu_per_mille));
            widths.util = widths
                .util
                .max(decimal_digits_u64(u64::from(task.sched_util_avg)));
            widths.util_min = widths
                .util_min
                .max(decimal_digits_u64(u64::from(task.sched_util_min)));
            widths.required = widths
                .required
                .max(decimal_digits_u64(u64::from(task.sched_required_capacity)));
            widths.preference = widths
                .preference
                .max(format_core_preference(task.core_preference).len());
            widths.migrations = widths
                .migrations
                .max(decimal_digits_u64(task.sched_migration_count));
            widths.vruntime = widths.vruntime.max(decimal_digits_u64(task.sched_vruntime));
            widths.deadline = widths.deadline.max(decimal_digits_u64(task.sched_deadline));
            widths.time = widths.time.max(time_width_ns(task.cpu_time_ns));
            widths.command = widths.command.max(task.name.len());
            widths.tgid = widths.tgid.max(decimal_digits_usize(task.tgid));
        }

        widths
    }
}

fn parse_sort_key(value: &str) -> SortKey {
    match value {
        "pid" => SortKey::Pid,
        "pcpu" | "%cpu" | "cpu%" => SortKey::PercentCpu,
        "cpu" | "c" => SortKey::Cpu,
        "name" | "n" => SortKey::Name,
        "state" | "s" => SortKey::State,
        "type" => SortKey::Type,
        _ => SortKey::PercentCpu,
    }
}

fn apply_sort(tasks: &mut [TaskSample], key: SortKey) {
    tasks.sort_by(|a, b| {
        let primary = match key {
            SortKey::Pid => a.task.pid.cmp(&b.task.pid),
            SortKey::PercentCpu => b.cpu_per_mille.cmp(&a.cpu_per_mille),
            SortKey::Cpu => a.task.cpu.cmp(&b.task.cpu),
            SortKey::Name => a.task.name.cmp(&b.task.name),
            SortKey::State => state_rank(a.task.state).cmp(&state_rank(b.task.state)),
            SortKey::Type => type_rank(a.task.task_type).cmp(&type_rank(b.task.task_type)),
        };
        primary.then_with(|| a.task.pid.cmp(&b.task.pid))
    });
}

fn format_stat(state: TaskState) -> &'static str {
    match state {
        TaskState::Running => "R",
        TaskState::Ready => "R<",
        TaskState::BlockedInterruptible => "S",
        TaskState::BlockedUninterruptible => "D",
        TaskState::Zombie => "Z",
        TaskState::Terminated => "T",
        TaskState::NotInitialized => "?",
    }
}

fn format_core_preference(preference: u8) -> &'static str {
    match preference {
        1 => "E",
        2 => "P",
        _ => "-",
    }
}

fn state_rank(state: TaskState) -> u8 {
    match state {
        TaskState::NotInitialized => 0,
        TaskState::Ready => 1,
        TaskState::Running => 2,
        TaskState::BlockedInterruptible => 3,
        TaskState::BlockedUninterruptible => 4,
        TaskState::Zombie => 5,
        TaskState::Terminated => 6,
    }
}

fn type_rank(task_type: TaskType) -> u8 {
    match task_type {
        TaskType::Kernel => 0,
        TaskType::User => 1,
    }
}

fn is_idle_task(task: &TaskInfo) -> bool {
    task.task_type == TaskType::Kernel && task.name.starts_with("idle")
}

struct Percent(u64);

impl fmt::Display for Percent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:01}", self.0 / 10, self.0 % 10)
    }
}

struct CpuLabel(u8);

impl fmt::Display for CpuLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CPU{}", self.0)
    }
}

struct TimeNs(u64);

impl fmt::Display for TimeNs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (hours, mins, secs, ms) = split_time_ns(self.0);
        if hours > 0 {
            write!(f, "{}:{:02}:{:02}.{:03}", hours, mins, secs, ms)
        } else {
            write!(f, "{:02}:{:02}.{:03}", mins, secs, ms)
        }
    }
}

fn split_time_ns(ns: u64) -> (u64, u64, u64, u64) {
    let total_ms = ns / 1_000_000;
    let ms = total_ms % 1000;
    let total_secs = total_ms / 1000;
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = total_secs / 3600;

    (hours, mins, secs, ms)
}

fn time_width_ns(ns: u64) -> usize {
    let (hours, _, _, _) = split_time_ns(ns);
    if hours > 0 {
        decimal_digits_u64(hours) + 10
    } else {
        9
    }
}

fn cpu_label_width(cpu: u8) -> usize {
    3 + decimal_digits_usize(usize::from(cpu))
}

fn percent_width(per_mille: u64) -> usize {
    decimal_digits_u64(per_mille / 10) + 2
}

fn signed_decimal_width_i32(value: i32) -> usize {
    let sign = usize::from(value.is_negative());
    sign + decimal_digits_u64(u64::from(value.unsigned_abs()))
}

fn decimal_digits_usize(mut value: usize) -> usize {
    let mut digits = 1;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

fn decimal_digits_u64(mut value: u64) -> usize {
    let mut digits = 1;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}
