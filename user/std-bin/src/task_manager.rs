use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use scarlet_sys::{Syscall, syscall0, syscall1, syscall2};
use scarlet_ui::prelude::*;
use scarlet_ui::{Color, ProgressView, ScrollView, hstack, vstack};
use scarlet_ui_macros::View;

const APP_ID: &str = "org.scarlet-os.desktop.task-manager";
const APP_TITLE: &str = "Task Manager";
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const WINDOW_WIDTH: f32 = 980.0;
const WINDOW_HEIGHT: f32 = 660.0;
const UTIL_SCALE: u32 = 1024;
const MAX_VISIBLE_TASKS: usize = 10;
const MAX_VISIBLE_CPUS: usize = 8;

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
}

#[derive(Debug, Clone)]
struct TaskInfo {
    pid: usize,
    state: TaskState,
    task_type: TaskType,
    cpu: u8,
    tgid: usize,
    name: String,
    cpu_time_ns: u64,
    sched_util_avg: u32,
}

impl RawTaskInfo {
    fn decode(&self) -> TaskInfo {
        let end = self
            .name
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(self.name.len());
        let name = std::str::from_utf8(&self.name[..end])
            .unwrap_or("<invalid>")
            .to_string();

        TaskInfo {
            pid: self.pid,
            state: TaskState::from_u8(self.state),
            task_type: TaskType::from_u8(self.task_type),
            cpu: self.cpu_id,
            tgid: self.tgid,
            name,
            cpu_time_ns: self.cpu_time_ns,
            sched_util_avg: self.sched_util_avg,
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
    busy_time_ns: u64,
    idle_time_ns: u64,
    usage_per_mille: u32,
}

#[derive(Clone, Default)]
struct SchedulerStats {
    migrations: u64,
    promotions: u64,
    demotions: u64,
    cooldown_skips: u64,
    work_steals: u64,
}

#[derive(Clone)]
struct CpuRow {
    id: usize,
    class_name: String,
    capacity: u32,
    util_avg: u32,
    util_min: u32,
    runnable: u32,
    cur_freq_khz: u32,
    target_freq_khz: u32,
    max_freq_khz: u32,
}

impl Default for CpuRow {
    fn default() -> Self {
        Self {
            id: 0,
            class_name: String::from("unknown"),
            capacity: 0,
            util_avg: 0,
            util_min: 0,
            runnable: 0,
            cur_freq_khz: 0,
            target_freq_khz: 0,
            max_freq_khz: 0,
        }
    }
}

#[derive(Clone)]
struct TaskRow {
    pid: usize,
    tgid: usize,
    cpu: u8,
    state: TaskState,
    task_type: TaskType,
    name: String,
    cpu_time_ns: u64,
    cpu_per_mille: u64,
    util_avg: u32,
}

#[derive(Clone, Default)]
struct TaskCounts {
    total: usize,
    running: usize,
    sleeping: usize,
    stopped: usize,
    zombie: usize,
    user: usize,
    kernel: usize,
}

#[derive(Clone)]
struct Snapshot {
    counts: TaskCounts,
    busy_per_mille: u64,
    idle_per_mille: u64,
    cpus: Vec<CpuRow>,
    tasks: Vec<TaskRow>,
    scheduler: SchedulerStats,
    status: String,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            counts: TaskCounts::default(),
            busy_per_mille: 0,
            idle_per_mille: 1000,
            cpus: Vec::new(),
            tasks: Vec::new(),
            scheduler: SchedulerStats::default(),
            status: String::from("Collecting scheduler samples..."),
        }
    }
}

#[derive(View, Clone)]
struct TaskManagerApp {
    snapshot: State<Snapshot>,
}

impl TaskManagerApp {
    fn new() -> Self {
        Self {
            snapshot: State::new(StateId::new(1), Snapshot::default()),
        }
    }
}

impl Application for TaskManagerApp {
    fn scenes(&self) -> impl Scene {
        let snapshot = self.snapshot.get();
        WindowGroup::new(
            "main",
            Window::new(
                APP_TITLE,
                app_content(snapshot).frame(f32::INFINITY, f32::INFINITY),
            )
            .app_id(APP_ID)
            .background_color(bg())
            .size(Size::new(WINDOW_WIDTH, WINDOW_HEIGHT)),
        )
    }

    fn init(&mut self) {
        start_sampler(self.snapshot.clone());
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

fn app_content(snapshot: Snapshot) -> impl View + Clone {
    vstack! {
        header_view(&snapshot),
        Spacer::new().frame_height(12.0),
        hstack! {
            section_title("CPU Cores"),
            Spacer::new(),
            body_text(format!(
                "migrations {}  promotions {}  demotions {}  skips {}  steals {}",
                snapshot.scheduler.migrations,
                snapshot.scheduler.promotions,
                snapshot.scheduler.demotions,
                snapshot.scheduler.cooldown_skips,
                snapshot.scheduler.work_steals,
            )),
        },
        Spacer::new().frame_height(8.0),
        hstack! {
            cpu_tile(&snapshot, 0),
            cpu_tile(&snapshot, 1),
            cpu_tile(&snapshot, 2),
            cpu_tile(&snapshot, 3),
        },
        hstack! {
            cpu_tile(&snapshot, 4),
            cpu_tile(&snapshot, 5),
            cpu_tile(&snapshot, 6),
            cpu_tile(&snapshot, 7),
        },
        Spacer::new().frame_height(14.0),
        hstack! {
            section_title("Tasks"),
            Spacer::new(),
            body_text(format!(
                "showing top {} of {}",
                snapshot.tasks.len().min(MAX_VISIBLE_TASKS),
                snapshot.counts.total,
            )),
        },
        table_header(),
        ScrollView::new(vstack! {
            task_row(&snapshot, 0),
            task_row(&snapshot, 1),
            task_row(&snapshot, 2),
            task_row(&snapshot, 3),
            task_row(&snapshot, 4),
            task_row(&snapshot, 5),
            task_row(&snapshot, 6),
            task_row(&snapshot, 7),
            task_row(&snapshot, 8),
            task_row(&snapshot, 9),
        })
        .frame_height(250.0),
    }
    .padding(16.0)
}

fn header_view(snapshot: &Snapshot) -> impl View + Clone + use<> {
    vstack! {
        hstack! {
            vstack! {
                Text::new(APP_TITLE)
                    .font_size(28.0)
                    .color(text_primary()),
                Text::new(snapshot.status.clone())
                    .font_size(12.0)
                    .color(text_secondary()),
            },
            Spacer::new(),
            vstack! {
                Text::new(format!(
                    "CPU {}% busy / {}% idle",
                    format_percent(snapshot.busy_per_mille),
                    format_percent(snapshot.idle_per_mille),
                ))
                .font_size(14.0)
                .color(text_primary()),
                ProgressView::new(progress(snapshot.busy_per_mille, 1000))
                    .frame_width(220.0),
            },
        },
        Spacer::new().frame_height(10.0),
        hstack! {
            summary_cell("total", snapshot.counts.total),
            summary_cell("running", snapshot.counts.running),
            summary_cell("sleeping", snapshot.counts.sleeping),
            summary_cell("stopped", snapshot.counts.stopped),
            summary_cell("zombie", snapshot.counts.zombie),
            summary_cell("user", snapshot.counts.user),
            summary_cell("kernel", snapshot.counts.kernel),
        },
    }
    .padding(14.0)
    .background(surface())
}

fn summary_cell(label: &'static str, value: usize) -> impl View + Clone {
    vstack! {
        Text::new(value.to_string())
            .font_size(18.0)
            .color(text_primary()),
        Text::new(label)
            .font_size(11.0)
            .color(text_secondary()),
    }
    .frame_width(92.0)
}

fn section_title(title: &'static str) -> impl View + Clone {
    Text::new(title).font_size(18.0).color(text_primary())
}

fn body_text(text: String) -> impl View + Clone {
    Text::new(text).font_size(12.0).color(text_secondary())
}

fn cpu_tile(snapshot: &Snapshot, index: usize) -> impl View + Clone + use<> {
    let cpu = snapshot.cpus.get(index);
    let id = cpu.map(|cpu| cpu.id).unwrap_or(index);
    let class_name = cpu
        .map(|cpu| cpu.class_name.clone())
        .unwrap_or_else(|| String::from("offline"));
    let capacity = cpu.map(|cpu| cpu.capacity).unwrap_or(0);
    let util_avg = cpu.map(|cpu| cpu.util_avg).unwrap_or(0);
    let util_min = cpu.map(|cpu| cpu.util_min).unwrap_or(0);
    let runnable = cpu.map(|cpu| cpu.runnable).unwrap_or(0);
    let cur_freq = cpu.map(|cpu| cpu.cur_freq_khz).unwrap_or(0);
    let target_freq = cpu.map(|cpu| cpu.target_freq_khz).unwrap_or(0);
    let max_freq = cpu.map(|cpu| cpu.max_freq_khz).unwrap_or(0);
    let title_color = if index < snapshot.cpus.len() {
        text_primary()
    } else {
        muted_text()
    };

    vstack! {
        hstack! {
            Text::new(format!("CPU{}", id))
                .font_size(15.0)
                .color(title_color),
            Spacer::new(),
            Text::new(class_name)
                .font_size(11.0)
                .color(text_secondary()),
        },
        ProgressView::new(progress(util_avg as u64, UTIL_SCALE as u64))
            .frame_width(205.0),
        hstack! {
            metric_text(format!("util {}", util_avg)),
            Spacer::new(),
            metric_text(format!("min {}", util_min)),
            Spacer::new(),
            metric_text(format!("run {}", runnable)),
        },
        hstack! {
            metric_text(format!("cap {}", capacity)),
            Spacer::new(),
            metric_text(freq_text(cur_freq, target_freq, max_freq)),
        },
    }
    .padding(10.0)
    .background(surface())
    .frame_width(224.0)
}

fn metric_text(text: String) -> impl View + Clone {
    Text::new(text).font_size(11.0).color(text_secondary())
}

fn table_header() -> impl View + Clone {
    hstack! {
        header_cell("PID", 54.0),
        header_cell("CPU", 48.0),
        header_cell("%CPU", 54.0),
        header_cell("UTIL", 52.0),
        header_cell("STAT", 48.0),
        header_cell("TYPE", 52.0),
        header_cell("TIME", 82.0),
        header_cell("TGID", 54.0),
        header_cell("COMMAND", 300.0),
    }
    .padding(8.0)
    .background(header_bg())
}

fn header_cell(text: &'static str, width: f32) -> impl View + Clone {
    Text::new(text)
        .font_size(11.0)
        .color(text_secondary())
        .frame_width(width)
}

fn task_row(snapshot: &Snapshot, index: usize) -> impl View + Clone + use<> {
    let task = snapshot.tasks.get(index);
    let is_visible = task.is_some();
    let row_bg = if index % 2 == 0 { surface() } else { row_alt() };
    let text_color = if is_visible {
        text_primary()
    } else {
        muted_text()
    };
    let pid = task.map(|task| task.pid.to_string()).unwrap_or_default();
    let cpu = task
        .map(|task| format!("CPU{}", task.cpu))
        .unwrap_or_default();
    let percent = task
        .map(|task| format_percent(task.cpu_per_mille))
        .unwrap_or_default();
    let util = task
        .map(|task| task.util_avg.to_string())
        .unwrap_or_default();
    let state = task
        .map(|task| format_stat(task.state).to_string())
        .unwrap_or_default();
    let task_type = task
        .map(|task| format_type(task.task_type).to_string())
        .unwrap_or_default();
    let time = task
        .map(|task| format_time_ns(task.cpu_time_ns))
        .unwrap_or_default();
    let tgid = task.map(|task| task.tgid.to_string()).unwrap_or_default();
    let name = task
        .map(|task| truncate(&task.name, 34))
        .unwrap_or_default();

    hstack! {
        row_cell(pid, 54.0, text_color),
        row_cell(cpu, 48.0, text_color),
        row_cell(percent, 54.0, text_color),
        row_cell(util, 52.0, text_color),
        row_cell(state, 48.0, text_color),
        row_cell(task_type, 52.0, text_color),
        row_cell(time, 82.0, text_color),
        row_cell(tgid, 54.0, text_color),
        row_cell(name, 300.0, text_color),
    }
    .padding(7.0)
    .background(row_bg)
}

fn row_cell(text: String, width: f32, color: Color) -> impl View + Clone {
    Text::new(text)
        .font_size(12.0)
        .color(color)
        .frame_width(width)
}

fn start_sampler(snapshot: State<Snapshot>) {
    thread::spawn(move || {
        let mut previous_tasks = task_info();
        let mut previous_cpu = cpu_usage();

        loop {
            let started_at = Instant::now();
            thread::sleep(SAMPLE_INTERVAL);
            let elapsed = started_at.elapsed();

            let current_tasks = task_info();
            let current_cpu = cpu_usage();
            let sampled = collect_snapshot(
                &previous_tasks,
                &current_tasks,
                previous_cpu,
                current_cpu,
                elapsed,
            );
            snapshot.set(sampled);

            previous_tasks = current_tasks;
            previous_cpu = current_cpu;
        }
    });
}

fn collect_snapshot(
    previous_tasks: &[TaskInfo],
    current_tasks: &[TaskInfo],
    previous_cpu: Option<CpuUsageInfo>,
    current_cpu: Option<CpuUsageInfo>,
    elapsed: Duration,
) -> Snapshot {
    let (cpus, scheduler, cpuinfo_status) = read_cpuinfo();
    let (busy_per_mille, idle_per_mille) = cpu_per_mille(previous_cpu, current_cpu);
    let elapsed_ns = elapsed.as_nanos().max(1);

    let mut rows = Vec::new();
    let mut counts = TaskCounts::default();
    for task in current_tasks {
        if is_idle_task(task) {
            continue;
        }

        counts.total += 1;
        match task.state {
            TaskState::Running | TaskState::Ready => counts.running += 1,
            TaskState::BlockedInterruptible | TaskState::BlockedUninterruptible => {
                counts.sleeping += 1;
            }
            TaskState::Terminated => counts.stopped += 1,
            TaskState::Zombie => counts.zombie += 1,
            TaskState::NotInitialized => {}
        }
        match task.task_type {
            TaskType::Kernel => counts.kernel += 1,
            TaskType::User => counts.user += 1,
        }

        let previous = previous_cpu_time(previous_tasks, task).unwrap_or(task.cpu_time_ns);
        let delta = task.cpu_time_ns.saturating_sub(previous);
        let cpu_per_mille = ((delta as u128 * 1000) / elapsed_ns) as u64;

        rows.push(TaskRow {
            pid: task.pid,
            tgid: task.tgid,
            cpu: task.cpu,
            state: task.state,
            task_type: task.task_type,
            name: task.name.clone(),
            cpu_time_ns: task.cpu_time_ns,
            cpu_per_mille,
            util_avg: task.sched_util_avg,
        });
    }

    rows.sort_by(|a, b| {
        b.cpu_per_mille
            .cmp(&a.cpu_per_mille)
            .then_with(|| b.util_avg.cmp(&a.util_avg))
            .then_with(|| a.pid.cmp(&b.pid))
    });

    if rows.len() > MAX_VISIBLE_TASKS {
        rows.truncate(MAX_VISIBLE_TASKS);
    }

    let status = cpuinfo_status.unwrap_or_else(|| {
        format!(
            "{} tasks, {} running, {} CPUs visible, {} cooldown skips",
            counts.total,
            counts.running,
            cpus.len().min(MAX_VISIBLE_CPUS),
            scheduler.cooldown_skips,
        )
    });

    Snapshot {
        counts,
        busy_per_mille,
        idle_per_mille,
        cpus,
        tasks: rows,
        scheduler,
        status,
    }
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
            busy_time_ns: raw.busy_time_ns,
            idle_time_ns: raw.idle_time_ns,
            usage_per_mille: raw.usage_per_mille,
        })
    }
}

fn previous_cpu_time(tasks: &[TaskInfo], task: &TaskInfo) -> Option<u64> {
    tasks
        .iter()
        .find(|previous| {
            previous.pid == task.pid && previous.tgid == task.tgid && previous.name == task.name
        })
        .map(|previous| previous.cpu_time_ns)
}

fn is_idle_task(task: &TaskInfo) -> bool {
    task.task_type == TaskType::Kernel && task.name.starts_with("idle")
}

fn cpu_per_mille(before: Option<CpuUsageInfo>, after: Option<CpuUsageInfo>) -> (u64, u64) {
    let Some(after) = after else {
        return (0, 1000);
    };
    let Some(before) = before else {
        let busy = after.usage_per_mille as u64;
        return (busy, 1000u64.saturating_sub(busy));
    };

    let busy_delta = after.busy_time_ns.saturating_sub(before.busy_time_ns);
    let idle_delta = after.idle_time_ns.saturating_sub(before.idle_time_ns);
    let total_delta = busy_delta.saturating_add(idle_delta);
    if total_delta == 0 {
        let busy = after.usage_per_mille as u64;
        return (busy, 1000u64.saturating_sub(busy));
    }

    let busy = ((busy_delta as u128 * 1000) / total_delta as u128) as u64;
    (busy, 1000u64.saturating_sub(busy))
}

fn read_cpuinfo() -> (Vec<CpuRow>, SchedulerStats, Option<String>) {
    let Ok(content) = fs::read_to_string("/dev/cpuinfo") else {
        return (
            Vec::new(),
            SchedulerStats::default(),
            Some(String::from("/dev/cpuinfo unavailable")),
        );
    };

    let mut cpus = Vec::new();
    let mut stats = SchedulerStats::default();
    let mut current: Option<CpuRow> = None;

    for line in content.lines() {
        if let Some(value) = field(line, "scheduler migrations") {
            stats.migrations = parse_u64(value);
            continue;
        }
        if let Some(value) = field(line, "scheduler promotions") {
            stats.promotions = parse_u64(value);
            continue;
        }
        if let Some(value) = field(line, "scheduler demotions") {
            stats.demotions = parse_u64(value);
            continue;
        }
        if let Some(value) = field(line, "scheduler cooldown skips") {
            stats.cooldown_skips = parse_u64(value);
            continue;
        }
        if let Some(value) = field(line, "scheduler work steals") {
            stats.work_steals = parse_u64(value);
            continue;
        }

        if let Some(value) = field(line, "processor") {
            if let Some(cpu) = current.take() {
                cpus.push(cpu);
            }
            current = Some(CpuRow {
                id: parse_usize(value),
                ..CpuRow::default()
            });
            continue;
        }

        let Some(cpu) = current.as_mut() else {
            continue;
        };
        if let Some(value) = field(line, "core class") {
            cpu.class_name = value.to_string();
        } else if let Some(value) = field(line, "cpu capacity") {
            cpu.capacity = parse_u32(value);
        } else if let Some(value) = field(line, "util avg") {
            cpu.util_avg = parse_u32(value);
        } else if let Some(value) = field(line, "util min") {
            cpu.util_min = parse_u32(value);
        } else if let Some(value) = field(line, "runnable") {
            cpu.runnable = parse_u32(value);
        } else if let Some(value) = field(line, "cur freq kHz") {
            cpu.cur_freq_khz = parse_u32(value);
        } else if let Some(value) = field(line, "target freq kHz") {
            cpu.target_freq_khz = parse_u32(value);
        } else if let Some(value) = field(line, "policy target kHz") {
            cpu.target_freq_khz = parse_u32(value);
        } else if let Some(value) = field(line, "max freq kHz") {
            cpu.max_freq_khz = parse_u32(value);
        }
    }

    if let Some(cpu) = current {
        cpus.push(cpu);
    }
    cpus.sort_by(|a, b| a.id.cmp(&b.id));

    (cpus, stats, None)
}

fn field<'a>(line: &'a str, expected: &str) -> Option<&'a str> {
    let (key, value) = line.split_once(':')?;
    if key.trim() == expected {
        Some(value.trim())
    } else {
        None
    }
}

fn parse_u32(value: &str) -> u32 {
    value.parse::<u32>().unwrap_or(0)
}

fn parse_u64(value: &str) -> u64 {
    value.parse::<u64>().unwrap_or(0)
}

fn parse_usize(value: &str) -> usize {
    value.parse::<usize>().unwrap_or(0)
}

fn progress(value: u64, max: u64) -> f32 {
    if max == 0 {
        return 0.0;
    }
    ((value.min(max) as f32) / max as f32).clamp(0.0, 1.0)
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

fn format_type(task_type: TaskType) -> &'static str {
    match task_type {
        TaskType::Kernel => "K",
        TaskType::User => "U",
    }
}

fn format_percent(per_mille: u64) -> String {
    format!("{}.{:01}", per_mille / 10, per_mille % 10)
}

fn format_time_ns(ns: u64) -> String {
    let total_ms = ns / 1_000_000;
    let ms = total_ms % 1000;
    let total_secs = total_ms / 1000;
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = total_secs / 3600;

    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, mins, secs)
    } else {
        format!("{:02}:{:02}.{:03}", mins, secs, ms)
    }
}

fn freq_text(cur: u32, target: u32, max: u32) -> String {
    if max == 0 {
        return String::from("freq n/a");
    }
    format!("{} -> {} MHz", cur / 1000, target / 1000)
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut output = String::new();
    for ch in value.chars().take(max_chars.saturating_sub(1)) {
        output.push(ch);
    }
    output.push_str("...");
    output
}

fn bg() -> Color {
    Color::rgb(244, 246, 248)
}

fn surface() -> Color {
    Color::rgb(255, 255, 255)
}

fn row_alt() -> Color {
    Color::rgb(249, 251, 253)
}

fn header_bg() -> Color {
    Color::rgb(235, 239, 244)
}

fn text_primary() -> Color {
    Color::rgb(28, 34, 42)
}

fn text_secondary() -> Color {
    Color::rgb(96, 106, 118)
}

fn muted_text() -> Color {
    Color::rgb(156, 164, 174)
}

fn main() {
    println!("[task_manager] starting");
    let mut app = TaskManagerApp::new();
    if let Err(error) = app.run() {
        println!("[task_manager] error: {}", error);
    }
}
