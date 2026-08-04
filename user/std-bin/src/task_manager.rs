//! Live task and CPU overview for the Scarlet desktop.
//!
//! The task manager deliberately keeps the data path small and synchronous:
//! the kernel is sampled on a worker thread, the resulting snapshot is sent
//! through one UI state, and the view only formats already-collected data.

use std::collections::BTreeMap;
use std::fs;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use scarlet_sys::{Syscall, syscall0, syscall1, syscall2};
use scarlet_ui::prelude::*;
use scarlet_ui::{
    Canvas, CanvasView, Color, GridView, Icon, IconSize, IconView, LazyVStack, ProgressView,
    ScrollView, TabItem, TabView, hstack, vstack,
};
use scarlet_ui_macros::View;
use std::process::ExitCode;

const APP_ID: &str = "org.scarlet-os.desktop.task-manager";
const APP_TITLE: &str = "Task Manager";

const WINDOW_WIDTH: f32 = 980.0;
const WINDOW_HEIGHT: f32 = 620.0;
const HEADER_HEIGHT: f32 = 100.0;
const PANEL_HEIGHT: f32 = 450.0;
const TAB_BAR_HEIGHT: f32 = 38.0;
const TASK_LIST_HEIGHT: f32 = 348.0;
const TASK_ROW_HEIGHT: f32 = 34.0;
const TASK_TABLE_HEADER_HEIGHT: f32 = 28.0;
const CPU_GRAPH_HEIGHT: f32 = 170.0;
const CPU_SUMMARY_HEIGHT: f32 = 72.0;
const CPU_GRID_HEIGHT: f32 = 302.0;
const CPU_GRID_ROW_HEIGHT: f32 = 141.0;
const CPU_GRID_SPACING: f32 = 10.0;
const CPU_GRID_COLUMNS: usize = 4;
const CPU_GRID_MINIMUM_CELL_WIDTH: f32 = 190.0;
const CPU_CARD_GRAPH_HEIGHT: f32 = 62.0;
const SAMPLE_INTERVAL: Duration = Duration::from_millis(750);
const MAX_TASKS: usize = 8_192;
const CPU_HISTORY_CAPACITY: usize = 60;
const SCHED_UTIL_SCALE: u32 = 1_024;

const TASK_PID_COLUMN_WIDTH: f32 = 48.0;
const TASK_CPU_COLUMN_WIDTH: f32 = 44.0;
const TASK_CPU_PERCENT_COLUMN_WIDTH: f32 = 52.0;
const TASK_UTIL_COLUMN_WIDTH: f32 = 48.0;
const TASK_STATE_COLUMN_WIDTH: f32 = 55.0;
const TASK_TYPE_COLUMN_WIDTH: f32 = 50.0;
const TASK_TIME_COLUMN_WIDTH: f32 = 78.0;
const TASK_COMMAND_COLUMN_WIDTH: f32 = 245.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TaskState {
    #[default]
    NotInitialized,
    Ready,
    Running,
    BlockedInterruptible,
    BlockedUninterruptible,
    Zombie,
    Terminated,
}

impl TaskState {
    fn from_raw(value: u8) -> Self {
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

    fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Running => "Running",
            Self::BlockedInterruptible => "Blocked",
            Self::BlockedUninterruptible => "Blocked",
            Self::Zombie => "Zombie",
            Self::Terminated => "Exited",
            Self::NotInitialized => "Unknown",
        }
    }

    fn is_runnable(self) -> bool {
        matches!(self, Self::Ready | Self::Running)
    }

    fn is_sleeping(self) -> bool {
        matches!(
            self,
            Self::BlockedInterruptible | Self::BlockedUninterruptible
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TaskType {
    #[default]
    Unknown,
    Kernel,
    User,
}

impl TaskType {
    fn from_raw(value: u8) -> Self {
        match value {
            1 => Self::User,
            0 => Self::Kernel,
            _ => Self::Unknown,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Kernel => "Kernel",
            Self::User => "User",
            Self::Unknown => "Other",
        }
    }
}

/// Raw task information returned by the kernel task-info syscall.
#[derive(Clone, Copy, Debug)]
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

impl Default for RawTaskInfo {
    fn default() -> Self {
        Self {
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
        }
    }
}

/// Raw aggregate CPU usage returned by the kernel CPU-info syscall.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
struct RawCpuUsageInfo {
    online_cpus: usize,
    busy_time_ns: u64,
    idle_time_ns: u64,
    total_time_ns: u64,
    usage_per_mille: u32,
    _reserved: u32,
}

#[derive(Clone, Debug)]
struct TaskInfo {
    pid: usize,
    tgid: usize,
    cpu_id: usize,
    state: TaskState,
    task_type: TaskType,
    name: String,
    cpu_time_ns: u64,
    sched_util_avg: u32,
    sched_migration_count: u64,
}

impl TaskInfo {
    fn from_raw(raw: RawTaskInfo) -> Self {
        Self {
            pid: raw.pid,
            tgid: raw.tgid,
            cpu_id: raw.cpu_id as usize,
            state: TaskState::from_raw(raw.state),
            task_type: TaskType::from_raw(raw.task_type),
            name: decode_name(&raw.name),
            cpu_time_ns: raw.cpu_time_ns,
            sched_util_avg: raw.sched_util_avg,
            sched_migration_count: raw.sched_migration_count,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct CpuSnapshot {
    id: usize,
    class_name: String,
    capacity: u32,
    util_avg: u32,
    util_min: u32,
    required_capacity: u32,
    runnable_tasks: u32,
    busy_time_ns: u64,
    idle_time_ns: u64,
    usage_per_mille: u32,
    history: Vec<u32>,
    current_pid: usize,
    current_name: String,
    current_frequency_khz: u32,
    target_frequency_khz: u32,
    max_frequency_khz: u32,
}

#[derive(Clone, Debug, Default)]
struct TaskCounts {
    total: usize,
    running: usize,
    sleeping: usize,
    zombies: usize,
    exited: usize,
}

#[derive(Clone, Debug)]
struct TaskRow {
    pid: usize,
    tgid: usize,
    cpu_id: usize,
    state: TaskState,
    task_type: TaskType,
    name: String,
    cpu_per_mille: u64,
    sched_util_avg: u32,
    cpu_time_ns: u64,
}

#[derive(Clone, Debug, Default)]
struct SchedulerSummary {
    runnable: usize,
}

#[derive(Clone, Debug)]
struct Snapshot {
    cpus: Vec<CpuSnapshot>,
    tasks: Vec<TaskRow>,
    counts: TaskCounts,
    scheduler: SchedulerSummary,
    cpu_history: Vec<u32>,
    cpu_busy_per_mille: u32,
    online_cpus: usize,
    status: String,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            cpus: Vec::new(),
            tasks: Vec::new(),
            counts: TaskCounts::default(),
            scheduler: SchedulerSummary::default(),
            cpu_history: Vec::new(),
            cpu_busy_per_mille: 0,
            online_cpus: 0,
            status: String::from("Collecting scheduler data…"),
        }
    }
}

#[derive(View, Clone)]
struct TaskManagerApp {
    snapshot: State<Arc<Snapshot>>,
    selected_tab: State<usize>,
    cpu_cards: State<Vec<CpuSnapshot>>,
    selected_cpu: State<Option<usize>>,
}

impl TaskManagerApp {
    fn new() -> Self {
        Self::default()
    }
}

impl Application for TaskManagerApp {
    fn init(&mut self) {
        start_sampler(self.snapshot.clone(), self.cpu_cards.clone());
    }

    fn scenes(&self) -> impl Scene {
        WindowGroup::new(
            "main",
            Window::new(
                APP_TITLE,
                task_manager_view(
                    self.snapshot.get(),
                    self.selected_tab.clone(),
                    self.cpu_cards.clone(),
                    self.selected_cpu.clone(),
                ),
            )
            .app_id(APP_ID)
            .size(Size::new(WINDOW_WIDTH, WINDOW_HEIGHT))
            .size_limits(
                Size::new(WINDOW_WIDTH, WINDOW_HEIGHT),
                Size::new(WINDOW_WIDTH, WINDOW_HEIGHT),
            )
            .resizable(false)
            .background_color(ColorPalette::default().window_background()),
        )
    }
}

fn start_sampler(snapshot: State<Arc<Snapshot>>, cpu_cards: State<Vec<CpuSnapshot>>) {
    thread::spawn(move || {
        let mut previous_tasks = read_tasks();
        let mut previous_cpu = read_cpu_usage();
        let mut previous_cpus = read_cpuinfo();
        let mut cpu_history = Vec::with_capacity(CPU_HISTORY_CAPACITY);
        let mut per_cpu_histories = BTreeMap::new();
        let mut sample_number: u64 = 0;

        loop {
            let started = Instant::now();
            thread::sleep(SAMPLE_INTERVAL);

            let current_tasks = read_tasks();
            let current_cpu = read_cpu_usage();
            let cpus = read_cpuinfo();
            sample_number = sample_number.saturating_add(1);

            let next = build_snapshot(
                &previous_tasks,
                &current_tasks,
                previous_cpu,
                current_cpu,
                &previous_cpus,
                cpus,
                sample_number,
                started.elapsed(),
                &mut cpu_history,
                &mut per_cpu_histories,
            );

            previous_tasks = current_tasks;
            previous_cpu = current_cpu;
            previous_cpus = next.cpus.clone();
            cpu_cards.set(next.cpus.clone());
            snapshot.set(Arc::new(next));
        }
    });
}

fn build_snapshot(
    previous_tasks: &[TaskInfo],
    current_tasks: &[TaskInfo],
    previous_cpu: Option<RawCpuUsageInfo>,
    current_cpu: Option<RawCpuUsageInfo>,
    previous_cpus: &[CpuSnapshot],
    mut cpus: Vec<CpuSnapshot>,
    sample_number: u64,
    elapsed: Duration,
    cpu_history: &mut Vec<u32>,
    per_cpu_histories: &mut BTreeMap<usize, Vec<u32>>,
) -> Snapshot {
    let elapsed_ns = elapsed.as_nanos().max(1) as u64;
    let mut counts = TaskCounts::default();
    let mut rows = Vec::with_capacity(current_tasks.len());
    let mut migrations: u64 = 0;
    let mut runnable: usize = 0;
    let previous_by_pid = previous_tasks
        .iter()
        .map(|task| (task.pid, task))
        .collect::<BTreeMap<_, _>>();

    for task in current_tasks {
        if is_idle_task(task) {
            continue;
        }

        counts.total = counts.total.saturating_add(1);
        if matches!(task.state, TaskState::Running) {
            counts.running = counts.running.saturating_add(1);
        }
        if task.state.is_runnable() {
            runnable = runnable.saturating_add(1);
        } else if task.state.is_sleeping() {
            counts.sleeping = counts.sleeping.saturating_add(1);
        }
        if matches!(task.state, TaskState::Zombie) {
            counts.zombies = counts.zombies.saturating_add(1);
        }
        if matches!(task.state, TaskState::Terminated) {
            counts.exited = counts.exited.saturating_add(1);
        }

        let previous = previous_by_pid.get(&task.pid).copied();
        migrations = migrations.saturating_add(
            task.sched_migration_count
                .saturating_sub(previous.map_or(0, |task| task.sched_migration_count)),
        );
        rows.push(TaskRow {
            pid: task.pid,
            tgid: task.tgid,
            cpu_id: task.cpu_id,
            state: task.state,
            task_type: task.task_type,
            name: if task.name.is_empty() {
                String::from("(unnamed)")
            } else {
                task.name.clone()
            },
            cpu_per_mille: task_cpu_per_mille(task, previous, elapsed_ns),
            sched_util_avg: task.sched_util_avg,
            cpu_time_ns: task.cpu_time_ns,
        });
    }

    rows.sort_by(|left, right| {
        right
            .cpu_per_mille
            .cmp(&left.cpu_per_mille)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.pid.cmp(&right.pid))
    });

    update_per_cpu_usage(previous_cpus, &mut cpus);
    update_per_cpu_histories(per_cpu_histories, &mut cpus);
    associate_running_tasks(&mut cpus, current_tasks);
    let (online_cpus, cpu_busy_per_mille) = cpu_delta(previous_cpu, current_cpu);
    push_cpu_history(cpu_history, cpu_busy_per_mille);
    let status = if online_cpus == 0 && cpus.is_empty() {
        String::from("Kernel scheduler data is unavailable")
    } else {
        format!(
            "Sample {} · updated every {} ms · {} migrations · {} zombies · {} exited",
            sample_number,
            SAMPLE_INTERVAL.as_millis(),
            migrations,
            counts.zombies,
            counts.exited,
        )
    };

    Snapshot {
        cpus,
        tasks: rows,
        counts,
        scheduler: SchedulerSummary { runnable },
        cpu_history: cpu_history.clone(),
        cpu_busy_per_mille,
        online_cpus,
        status,
    }
}

fn task_cpu_per_mille(task: &TaskInfo, previous: Option<&TaskInfo>, elapsed_ns: u64) -> u64 {
    let Some(previous) = previous else {
        return 0;
    };

    let delta = task.cpu_time_ns.saturating_sub(previous.cpu_time_ns);
    ((delta.saturating_mul(1_000)) / elapsed_ns).min(1_000)
}

fn update_per_cpu_usage(previous_cpus: &[CpuSnapshot], current_cpus: &mut [CpuSnapshot]) {
    let previous_by_id = previous_cpus
        .iter()
        .map(|cpu| (cpu.id, cpu))
        .collect::<BTreeMap<_, _>>();

    for current in current_cpus {
        let Some(previous) = previous_by_id.get(&current.id).copied() else {
            current.usage_per_mille = 0;
            continue;
        };
        let busy_delta = current.busy_time_ns.saturating_sub(previous.busy_time_ns);
        let idle_delta = current.idle_time_ns.saturating_sub(previous.idle_time_ns);
        let total_delta = busy_delta.saturating_add(idle_delta);
        current.usage_per_mille = if total_delta == 0 {
            0
        } else {
            ((busy_delta.saturating_mul(1_000)) / total_delta).min(1_000) as u32
        };
    }
}

fn update_per_cpu_histories(
    histories: &mut BTreeMap<usize, Vec<u32>>,
    current_cpus: &mut [CpuSnapshot],
) {
    for cpu in current_cpus {
        let history = histories
            .entry(cpu.id)
            .or_insert_with(|| Vec::with_capacity(CPU_HISTORY_CAPACITY));
        push_cpu_history(history, cpu.usage_per_mille);
        cpu.history = history.clone();
    }
}

fn associate_running_tasks(cpus: &mut [CpuSnapshot], tasks: &[TaskInfo]) {
    for cpu in cpus {
        let Some(task) = tasks.iter().find(|task| {
            task.cpu_id == cpu.id && matches!(task.state, TaskState::Running) && !is_idle_task(task)
        }) else {
            continue;
        };
        cpu.current_pid = task.pid;
        cpu.current_name = task.name.clone();
    }
}

fn push_cpu_history(history: &mut Vec<u32>, sample: u32) {
    if history.len() >= CPU_HISTORY_CAPACITY {
        let remove_count = history.len() + 1 - CPU_HISTORY_CAPACITY;
        history.drain(..remove_count);
    }
    history.push(sample.min(1_000));
}

fn cpu_delta(previous: Option<RawCpuUsageInfo>, current: Option<RawCpuUsageInfo>) -> (usize, u32) {
    let Some(current) = current else {
        return (0, 0);
    };
    let Some(previous) = previous else {
        return (current.online_cpus, current.usage_per_mille.min(1_000));
    };

    let busy_delta = current.busy_time_ns.saturating_sub(previous.busy_time_ns);
    let idle_delta = current.idle_time_ns.saturating_sub(previous.idle_time_ns);
    let total_delta = busy_delta.saturating_add(idle_delta);
    if total_delta == 0 {
        return (current.online_cpus, current.usage_per_mille.min(1_000));
    }

    (
        current.online_cpus,
        ((busy_delta.saturating_mul(1_000)) / total_delta).min(1_000) as u32,
    )
}

fn read_tasks() -> Vec<TaskInfo> {
    let count = syscall0(Syscall::GetTaskInfoCount);
    if count == usize::MAX || count == 0 {
        return Vec::new();
    }

    let count = count.min(MAX_TASKS);
    let mut raw = vec![RawTaskInfo::default(); count];
    let returned = syscall2(
        Syscall::GetTaskInfoList,
        raw.as_mut_ptr() as usize,
        raw.len(),
    );
    if returned == usize::MAX {
        return Vec::new();
    }

    raw.truncate(returned.min(raw.len()));
    raw.into_iter().map(TaskInfo::from_raw).collect()
}

fn read_cpu_usage() -> Option<RawCpuUsageInfo> {
    let mut raw = RawCpuUsageInfo::default();
    let result = syscall1(
        Syscall::GetCpuUsageInfo,
        &mut raw as *mut RawCpuUsageInfo as usize,
    );
    (result != usize::MAX).then_some(raw)
}

fn read_cpuinfo() -> Vec<CpuSnapshot> {
    let Ok(contents) = fs::read_to_string("/dev/cpuinfo") else {
        return Vec::new();
    };

    let mut cpus = Vec::new();
    let mut current = CpuSnapshot::default();
    let mut has_cpu = false;

    for line in contents.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        match key {
            "processor" => {
                if has_cpu {
                    cpus.push(current);
                    current = CpuSnapshot::default();
                }
                current.id = parse_usize(value).unwrap_or(cpus.len());
                has_cpu = true;
            }
            "core class" | "class" => current.class_name = value.to_string(),
            "capacity" | "cpu capacity" => current.capacity = parse_u32(value),
            "util avg" | "utilization average" => current.util_avg = parse_u32(value),
            "util min" | "utilization minimum" => current.util_min = parse_u32(value),
            "required capacity" => current.required_capacity = parse_u32(value),
            "runnable" | "runnable tasks" => current.runnable_tasks = parse_u32(value),
            "busy time ns" => current.busy_time_ns = parse_u64(value),
            "idle time ns" => current.idle_time_ns = parse_u64(value),
            "current pid" => current.current_pid = parse_usize(value).unwrap_or(0),
            "current task" | "current name" => current.current_name = value.to_string(),
            "cur freq kHz" | "current frequency" | "current frequency khz" => {
                current.current_frequency_khz = parse_u32(value)
            }
            "target freq kHz"
            | "policy target kHz"
            | "target frequency"
            | "target frequency khz" => current.target_frequency_khz = parse_u32(value),
            "max freq kHz" | "max frequency" | "max frequency khz" => {
                current.max_frequency_khz = parse_u32(value)
            }
            _ => {}
        }
    }

    if has_cpu {
        cpus.push(current);
    }
    cpus.sort_by_key(|cpu| cpu.id);
    cpus
}

fn decode_name(bytes: &[u8; 64]) -> String {
    let length = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..length]).trim().to_string()
}

fn parse_usize(value: &str) -> Option<usize> {
    value
        .split_whitespace()
        .next()
        .and_then(|number| number.parse().ok())
}

fn parse_u32(value: &str) -> u32 {
    parse_usize(value).unwrap_or(0).min(u32::MAX as usize) as u32
}

fn parse_u64(value: &str) -> u64 {
    value
        .split_whitespace()
        .next()
        .and_then(|number| number.parse().ok())
        .unwrap_or(0)
}

fn is_idle_task(task: &TaskInfo) -> bool {
    task.task_type == TaskType::Kernel && task.name.starts_with("idle")
}

fn task_manager_view(
    snapshot: Arc<Snapshot>,
    selected_tab: State<usize>,
    cpu_cards: State<Vec<CpuSnapshot>>,
    selected_cpu: State<Option<usize>>,
) -> impl View + Clone + use<> {
    let palette = ColorPalette::default();
    let overview_snapshot = snapshot.clone();
    let cpu_snapshot = snapshot.clone();
    let task_snapshot = snapshot.clone();
    let tabs = vec![
        TabItem::new("Overview", move || overview_page(overview_snapshot.clone())),
        TabItem::new("CPU", move || {
            cpu_page(
                cpu_snapshot.clone(),
                cpu_cards.clone(),
                selected_cpu.clone(),
            )
        }),
        TabItem::new("Tasks", move || task_page(task_snapshot.clone())),
    ];

    vstack! {
        header_view(snapshot.as_ref()),
        Spacer::new().frame_height(14.0),
        TabView::with_selected_index(tabs, selected_tab)
            .tab_bar_height(TAB_BAR_HEIGHT)
            .tab_padding(18.0)
            .font_size(13.0)
            .frame(f32::INFINITY, PANEL_HEIGHT)
            .background(palette.surface())
            .clip_radius(12.0),
    }
    .padding(18.0)
    .frame(WINDOW_WIDTH, WINDOW_HEIGHT)
    .background(palette.window_background())
}

fn overview_page(snapshot: Arc<Snapshot>) -> impl View + Clone + use<> {
    let palette = ColorPalette::default();
    let peak = snapshot.cpu_history.iter().copied().max().unwrap_or(0);
    let samples = snapshot.cpu_history.len();
    let graph_snapshot = snapshot.clone();

    vstack! {
        hstack! {
            vstack! {
                Text::new("CPU utilization")
                    .font_size(16.0)
                    .color(palette.text_primary()),
                Text::new(format!("{} samples · {} ms interval", samples, SAMPLE_INTERVAL.as_millis()))
                    .font_size(11.0)
                    .color(palette.text_secondary()),
            }
            .spacing(3.0),
            Spacer::new(),
            Text::new(format!("{}% now", format_percent(snapshot.cpu_busy_per_mille)))
                .font_size(17.0)
                .color(palette.primary()),
        },
        cpu_history_graph(graph_snapshot),
        hstack! {
            summary_card("PEAK", format!("{}%", format_percent(peak)), palette.warning()),
            summary_card("TASKS", snapshot.counts.total.to_string(), palette.info()),
            summary_card("RUNNABLE", snapshot.scheduler.runnable.to_string(), palette.success()),
            summary_card("ONLINE", snapshot.online_cpus.to_string(), palette.primary()),
        }
        .spacing(8.0),
    }
    .spacing(10.0)
    .padding(14.0)
    .frame(f32::INFINITY, PANEL_HEIGHT - TAB_BAR_HEIGHT)
    .background(palette.surface())
}

fn cpu_page(
    snapshot: Arc<Snapshot>,
    cpu_cards: State<Vec<CpuSnapshot>>,
    selected_cpu: State<Option<usize>>,
) -> impl View + Clone + use<> {
    let palette = ColorPalette::default();
    let cpu_grid = GridView::new(
        cpu_cards,
        selected_cpu,
        CPU_GRID_COLUMNS,
        CPU_GRID_ROW_HEIGHT,
        move |_, cpu, _| cpu_card(cpu),
    )
    .spacing(CPU_GRID_SPACING)
    .row_spacing(CPU_GRID_SPACING)
    .minimum_cell_width(CPU_GRID_MINIMUM_CELL_WIDTH)
    .frame(f32::INFINITY, CPU_GRID_HEIGHT);

    vstack! {
        cpu_overall_summary(snapshot.as_ref()),
        cpu_grid,
    }
    .spacing(CPU_GRID_SPACING)
    .alignment(Alignment::Leading)
    .padding(14.0)
    .frame(f32::INFINITY, PANEL_HEIGHT - TAB_BAR_HEIGHT)
    .background(palette.surface())
}

fn cpu_overall_summary(snapshot: &Snapshot) -> impl View + Clone + use<> {
    let palette = ColorPalette::default();
    let peak = snapshot.cpu_history.iter().copied().max().unwrap_or(0);
    let history_seconds =
        snapshot.cpu_history.len() as u64 * SAMPLE_INTERVAL.as_millis() as u64 / 1_000;

    hstack! {
        vstack! {
            Text::new("OVERALL CPU")
                .font_size(10.0)
                .color(palette.text_secondary()),
            Text::new(format!("{}%", format_percent(snapshot.cpu_busy_per_mille)))
                .font_size(22.0)
                .color(palette.primary()),
        }
        .spacing(2.0)
        .frame_width(104.0),
        vstack! {
            ProgressView::new(snapshot.cpu_busy_per_mille as f32 / 1_000.0)
                .frame_width(520.0),
            Text::new(format!(
                "{} cores · {} runnable · {} s history",
                snapshot.online_cpus,
                snapshot.scheduler.runnable,
                history_seconds,
            ))
            .font_size(10.0)
            .color(palette.text_secondary()),
        }
        .spacing(7.0),
        Spacer::new(),
        vstack! {
            Text::new("PEAK")
                .font_size(10.0)
                .color(palette.text_secondary()),
            Text::new(format!("{}%", format_percent(peak)))
                .font_size(16.0)
                .color(palette.warning()),
        }
        .spacing(2.0),
    }
    .spacing(12.0)
    .alignment(Alignment::Center)
    .padding(10.0)
    .frame(f32::INFINITY, CPU_SUMMARY_HEIGHT)
    .background(palette.surface_variant())
    .clip_radius(8.0)
}

fn task_page(snapshot: Arc<Snapshot>) -> impl View + Clone + use<> {
    let palette = ColorPalette::default();
    let task_count = snapshot.tasks.len().max(1);
    let task_list = ScrollView::new(LazyVStack::new(task_count, TASK_ROW_HEIGHT, move |index| {
        task_row(snapshot.tasks.get(index).cloned(), index)
    }))
    .frame(f32::INFINITY, TASK_LIST_HEIGHT);

    vstack! {
        task_table_header(),
        task_list,
    }
    .spacing(8.0)
    .alignment(Alignment::Leading)
    .padding(14.0)
    .frame(f32::INFINITY, PANEL_HEIGHT - TAB_BAR_HEIGHT)
    .background(palette.surface())
}

fn cpu_history_graph(snapshot: Arc<Snapshot>) -> impl View + Clone + use<> {
    let palette = ColorPalette::default();
    let background = palette.surface_variant();
    let grid = palette.border().with_opacity(0.45);
    let line = palette.primary();

    CanvasView::new(
        600.0,
        CPU_GRAPH_HEIGHT,
        Rc::new(move |buffer, width, height| {
            let mut canvas = Canvas::new(buffer, width, height);
            draw_usage_history(
                &mut canvas,
                width,
                height,
                &snapshot.cpu_history,
                background,
                grid,
                line,
            );
        }),
    )
    .frame(f32::INFINITY, CPU_GRAPH_HEIGHT)
    .clip_radius(8.0)
}

fn draw_usage_history(
    canvas: &mut Canvas<'_>,
    width: u32,
    height: u32,
    history: &[u32],
    background: Color,
    grid: Color,
    line: Color,
) {
    canvas.fill_rect(0, 0, width, height, background);

    let right = width.saturating_sub(1) as i32;
    let bottom = height.saturating_sub(1) as i32;
    for division in 0..=4 {
        let y = bottom.saturating_mul(division) / 4;
        canvas.draw_line(0, y, right, y, grid);
    }

    if history.len() < 2 || width < 2 || height < 2 {
        return;
    }

    let start = history.len().saturating_sub(width as usize);
    let visible = &history[start..];
    let span = visible.len().saturating_sub(1).max(1);
    let graph_bottom = bottom.saturating_sub(6);
    let graph_top = 6;
    let graph_height = graph_bottom.saturating_sub(graph_top).max(1);
    let mut previous = None;

    for (index, sample) in visible.iter().copied().enumerate() {
        let x = (index as u64 * right as u64 / span as u64) as i32;
        let value = sample.min(1_000) as i32;
        let y = graph_bottom - graph_height * value / 1_000;
        if let Some((previous_x, previous_y)) = previous {
            canvas.draw_line(previous_x, previous_y, x, y, line);
        }
        previous = Some((x, y));
    }
}

fn header_view(snapshot: &Snapshot) -> impl View + Clone + use<> {
    let palette = ColorPalette::default();
    let cpu_percent = format_percent(snapshot.cpu_busy_per_mille);
    let task_count = snapshot.counts.total.to_string();
    let running_count = snapshot.counts.running.to_string();
    let sleeping_count = snapshot.counts.sleeping.to_string();
    let core_count = snapshot.online_cpus.to_string();

    vstack! {
        hstack! {
            IconView::new(Icon::ChartBar)
                .size(IconSize::Large)
                .color(palette.primary()),
            vstack! {
                Text::new(snapshot.status.clone())
                    .font_size(13.0)
                    .color(palette.text_secondary()),
            }
            .spacing(3.0),
            Spacer::new(),
            vstack! {
                Text::new(format!("{}% CPU", cpu_percent))
                    .font_size(20.0)
                    .color(palette.text_primary()),
                ProgressView::new(snapshot.cpu_busy_per_mille as f32 / 1_000.0)
                    .frame_width(150.0),
            }
            .spacing(6.0),
        }
        .spacing(12.0),
        hstack! {
            summary_card("TASKS", task_count, palette.info()),
            summary_card("RUNNING", running_count, palette.success()),
            summary_card("SLEEPING", sleeping_count, palette.secondary()),
            summary_card("ONLINE", core_count, palette.primary()),
            summary_card(
                "RUNNABLE",
                snapshot.scheduler.runnable.to_string(),
                palette.warning(),
            ),
        }
        .spacing(8.0),
    }
    .spacing(12.0)
    .padding(14.0)
    .frame(f32::INFINITY, HEADER_HEIGHT)
    .background(palette.surface())
    .clip_radius(12.0)
}

fn summary_card(label: &'static str, value: String, accent: Color) -> impl View + Clone + use<> {
    vstack! {
        Text::new(label)
            .font_size(10.0)
            .color(ColorPalette::default().text_secondary()),
        Text::new(value)
            .font_size(17.0)
            .color(accent),
    }
    .spacing(2.0)
    .padding(7.0)
    .frame_width(104.0)
    .background(ColorPalette::default().surface_variant())
    .clip_radius(8.0)
}

fn cpu_card(cpu: CpuSnapshot) -> impl View + Clone + use<> {
    let palette = ColorPalette::default();
    let accent = match cpu.id % 4 {
        0 => palette.primary(),
        1 => palette.info(),
        2 => palette.success(),
        _ => palette.warning(),
    };
    let class_name = if cpu.class_name.is_empty() {
        String::from("logical core")
    } else {
        cpu.class_name.clone()
    };
    let frequency_khz = if cpu.current_frequency_khz > 0 {
        cpu.current_frequency_khz
    } else {
        cpu.target_frequency_khz
    };
    let frequency = if frequency_khz > 0 {
        format_frequency_value(frequency_khz)
    } else {
        String::from("—")
    };
    let current = if cpu.current_pid > 0 {
        format!("{} {}", cpu.current_pid, truncate(&cpu.current_name, 12))
    } else {
        String::from("Idle")
    };
    let history = cpu.history.clone();

    vstack! {
        hstack! {
            vstack! {
                Text::new(format!("CPU {}", cpu.id))
                    .font_size(13.0)
                    .color(palette.text_primary()),
                Text::new(class_name)
                    .font_size(9.0)
                    .color(palette.text_secondary()),
            }
            .spacing(1.0),
            Spacer::new(),
            Text::new(format!("{}%", format_percent(cpu.usage_per_mille)))
                .font_size(15.0)
                .color(accent),
        },
        cpu_card_history_graph(history, accent),
        hstack! {
            Text::new(frequency)
                .font_size(9.0)
                .color(palette.text_secondary()),
            Spacer::new(),
            Text::new(current)
                .font_size(9.0)
                .color(palette.text_primary()),
        },
    }
    .spacing(5.0)
    .padding(9.0)
    .frame(f32::INFINITY, CPU_GRID_ROW_HEIGHT)
    .background(palette.surface_variant())
    .clip_radius(8.0)
}

fn cpu_card_history_graph(history: Vec<u32>, line: Color) -> impl View + Clone + use<> {
    let palette = ColorPalette::default();
    let background = palette.background_secondary();
    let grid = palette.border().with_opacity(0.32);

    CanvasView::new(
        180.0,
        CPU_CARD_GRAPH_HEIGHT,
        Rc::new(move |buffer, width, height| {
            let mut canvas = Canvas::new(buffer, width, height);
            draw_usage_history(&mut canvas, width, height, &history, background, grid, line);
        }),
    )
    .frame(f32::INFINITY, CPU_CARD_GRAPH_HEIGHT)
    .clip_radius(5.0)
}

fn task_table_header() -> impl View + Clone + use<> {
    let palette = ColorPalette::default();
    hstack! {
        table_cell(String::from("PID"), TASK_PID_COLUMN_WIDTH, palette.text_secondary()),
        table_cell(String::from("CPU"), TASK_CPU_COLUMN_WIDTH, palette.text_secondary()),
        table_cell(String::from("CPU%"), TASK_CPU_PERCENT_COLUMN_WIDTH, palette.text_secondary()),
        table_cell(String::from("UTIL"), TASK_UTIL_COLUMN_WIDTH, palette.text_secondary()),
        table_cell(String::from("STATE"), TASK_STATE_COLUMN_WIDTH, palette.text_secondary()),
        table_cell(String::from("TYPE"), TASK_TYPE_COLUMN_WIDTH, palette.text_secondary()),
        table_cell(String::from("TIME"), TASK_TIME_COLUMN_WIDTH, palette.text_secondary()),
        table_cell(String::from("COMMAND"), TASK_COMMAND_COLUMN_WIDTH, palette.text_secondary()),
    }
    .padding(6.0)
    // Rows are full-width because LazyVStack gives them tight viewport
    // constraints. Give the fixed header the same width so VStack does not
    // center its intrinsic column width independently from the rows.
    .frame(f32::INFINITY, TASK_TABLE_HEADER_HEIGHT)
    .background(palette.surface_variant())
    .clip_radius(6.0)
}

fn task_row(task: Option<TaskRow>, index: usize) -> impl View + Clone + use<> {
    let palette = ColorPalette::default();
    let (pid, cpu, cpu_percent, util, state, task_type, time, command) = match task {
        Some(task) => (
            task.pid.to_string(),
            format!("{}", task.cpu_id),
            format_percent(task.cpu_per_mille as u32),
            format_percent(util_to_per_mille(task.sched_util_avg)),
            task.state.label().to_string(),
            task.task_type.label().to_string(),
            format_duration(task.cpu_time_ns),
            format!("{}  ({})", truncate(&task.name, 28), task.tgid),
        ),
        None => (
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::from("No tasks"),
            String::new(),
            String::new(),
            String::new(),
        ),
    };
    let row_background = if index % 2 == 0 {
        palette.background_secondary()
    } else {
        palette.surface_variant()
    };

    hstack! {
        table_cell(pid, TASK_PID_COLUMN_WIDTH, palette.text_primary()),
        table_cell(cpu, TASK_CPU_COLUMN_WIDTH, palette.text_secondary()),
        table_cell(cpu_percent, TASK_CPU_PERCENT_COLUMN_WIDTH, palette.primary()),
        table_cell(util, TASK_UTIL_COLUMN_WIDTH, palette.text_secondary()),
        table_cell(state, TASK_STATE_COLUMN_WIDTH, palette.text_primary()),
        table_cell(task_type, TASK_TYPE_COLUMN_WIDTH, palette.text_secondary()),
        table_cell(time, TASK_TIME_COLUMN_WIDTH, palette.text_secondary()),
        table_cell(command, TASK_COMMAND_COLUMN_WIDTH, palette.text_primary()),
    }
    .padding(6.0)
    .frame(f32::INFINITY, TASK_ROW_HEIGHT)
    .background(row_background)
}

fn table_cell(value: String, width: f32, color: Color) -> impl View + Clone + use<> {
    Text::new(value)
        .font_size(11.0)
        .color(color)
        .frame_width(width)
}

fn format_percent(per_mille: u32) -> String {
    format!("{}.{:01}", per_mille / 10, per_mille % 10)
}

fn util_to_per_mille(util: u32) -> u32 {
    ((util.min(SCHED_UTIL_SCALE) as u64 * 1_000) / SCHED_UTIL_SCALE as u64) as u32
}

fn format_frequency_value(khz: u32) -> String {
    if khz >= 1_000_000 {
        format!("{}.{:02} GHz", khz / 1_000_000, (khz % 1_000_000) / 10_000)
    } else if khz >= 1_000 {
        format!("{}.{:02} MHz", khz / 1_000, (khz % 1_000) / 10)
    } else {
        format!("{} kHz", khz)
    }
}

fn format_duration(nanoseconds: u64) -> String {
    let total_milliseconds = nanoseconds / 1_000_000;
    let milliseconds = total_milliseconds % 1_000;
    let total_seconds = total_milliseconds / 1_000;
    let seconds = total_seconds % 60;
    let minutes = (total_seconds / 60) % 60;
    let hours = total_seconds / 3_600;
    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{:02}:{:02}.{:03}", minutes, seconds, milliseconds)
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut result = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        result.push('…');
    }
    result
}

fn main() -> ExitCode {
    println!("Starting Scarlet Task Manager...");
    let _ = TaskManagerApp::new().run();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_info(
        pid: usize,
        name: &str,
        state: TaskState,
        task_type: TaskType,
        cpu_id: usize,
        cpu_time_ns: u64,
        migrations: u64,
    ) -> TaskInfo {
        TaskInfo {
            pid,
            tgid: pid,
            cpu_id,
            state,
            task_type,
            name: name.to_string(),
            cpu_time_ns,
            sched_util_avg: 0,
            sched_migration_count: migrations,
        }
    }

    #[test]
    fn decodes_task_names_without_trailing_nul_bytes() {
        let mut name = [0; 64];
        name[..4].copy_from_slice(b"init");
        assert_eq!(decode_name(&name), "init");
    }

    #[test]
    fn formats_percent_as_one_decimal_place() {
        assert_eq!(format_percent(0), "0.0");
        assert_eq!(format_percent(375), "37.5");
        assert_eq!(format_percent(1_000), "100.0");
    }

    #[test]
    fn computes_cpu_delta_from_busy_and_idle_time() {
        let previous = RawCpuUsageInfo {
            online_cpus: 4,
            busy_time_ns: 50,
            idle_time_ns: 50,
            total_time_ns: 100,
            usage_per_mille: 500,
            _reserved: 0,
        };
        let current = RawCpuUsageInfo {
            online_cpus: 4,
            busy_time_ns: 110,
            idle_time_ns: 90,
            total_time_ns: 200,
            usage_per_mille: 550,
            _reserved: 0,
        };
        assert_eq!(cpu_delta(Some(previous), Some(current)), (4, 600));
    }

    #[test]
    fn snapshot_excludes_idle_tasks_and_separates_running_from_runnable() {
        let previous = vec![
            task_info(
                1,
                "idle/0",
                TaskState::Running,
                TaskType::Kernel,
                0,
                1_000,
                0,
            ),
            task_info(2, "worker", TaskState::Running, TaskType::User, 0, 1_000, 2),
            task_info(3, "ready", TaskState::Ready, TaskType::User, 1, 1_000, 4),
        ];
        let current = vec![
            task_info(
                1,
                "idle/0",
                TaskState::Running,
                TaskType::Kernel,
                0,
                2_000,
                0,
            ),
            task_info(2, "worker", TaskState::Running, TaskType::User, 0, 2_000, 3),
            task_info(3, "ready", TaskState::Ready, TaskType::User, 1, 2_000, 6),
        ];
        let cpus = vec![
            CpuSnapshot {
                id: 0,
                ..CpuSnapshot::default()
            },
            CpuSnapshot {
                id: 1,
                ..CpuSnapshot::default()
            },
        ];
        let mut history = Vec::new();
        let mut per_cpu_histories = BTreeMap::new();

        let snapshot = build_snapshot(
            &previous,
            &current,
            None,
            None,
            &[],
            cpus,
            1,
            Duration::from_secs(1),
            &mut history,
            &mut per_cpu_histories,
        );

        assert_eq!(snapshot.counts.total, 2);
        assert_eq!(snapshot.counts.running, 1);
        assert_eq!(snapshot.scheduler.runnable, 2);
        assert!(snapshot.tasks.iter().all(|task| task.name != "idle/0"));
        assert_eq!(snapshot.cpus[0].current_pid, 2);
        assert_eq!(snapshot.cpus[0].current_name, "worker");
        assert_eq!(snapshot.cpus[1].current_pid, 0);
        assert!(snapshot.status.contains("3 migrations"));
    }

    #[test]
    fn per_cpu_usage_uses_busy_and_idle_time_deltas() {
        let previous = vec![
            CpuSnapshot {
                id: 0,
                busy_time_ns: 100,
                idle_time_ns: 900,
                util_avg: 900,
                ..CpuSnapshot::default()
            },
            CpuSnapshot {
                id: 1,
                busy_time_ns: 500,
                idle_time_ns: 500,
                util_avg: 900,
                ..CpuSnapshot::default()
            },
        ];
        let mut current = vec![
            CpuSnapshot {
                id: 0,
                busy_time_ns: 160,
                idle_time_ns: 940,
                util_avg: 900,
                ..CpuSnapshot::default()
            },
            CpuSnapshot {
                id: 1,
                busy_time_ns: 500,
                idle_time_ns: 600,
                util_avg: 900,
                ..CpuSnapshot::default()
            },
        ];

        update_per_cpu_usage(&previous, &mut current);

        assert_eq!(current[0].usage_per_mille, 600);
        assert_eq!(current[1].usage_per_mille, 0);
    }

    #[test]
    fn per_cpu_histories_are_independent_and_bounded() {
        let mut histories = BTreeMap::new();
        let mut cpus = vec![
            CpuSnapshot {
                id: 0,
                ..CpuSnapshot::default()
            },
            CpuSnapshot {
                id: 1,
                ..CpuSnapshot::default()
            },
        ];

        for sample in 0..=CPU_HISTORY_CAPACITY as u32 {
            cpus[0].usage_per_mille = sample;
            cpus[1].usage_per_mille = 1_000 - sample;
            update_per_cpu_histories(&mut histories, &mut cpus);
        }

        assert_eq!(cpus[0].history.len(), CPU_HISTORY_CAPACITY);
        assert_eq!(cpus[1].history.len(), CPU_HISTORY_CAPACITY);
        assert_eq!(cpus[0].history.first(), Some(&1));
        assert_eq!(cpus[0].history.last(), Some(&(CPU_HISTORY_CAPACITY as u32)));
        assert_eq!(cpus[1].history.first(), Some(&999));
        assert_eq!(
            cpus[1].history.last(),
            Some(&(1_000 - CPU_HISTORY_CAPACITY as u32))
        );
    }

    #[test]
    fn cpu_history_keeps_only_the_latest_bounded_samples() {
        let mut history = (0..CPU_HISTORY_CAPACITY as u32).collect::<Vec<_>>();

        push_cpu_history(&mut history, 1_500);

        assert_eq!(history.len(), CPU_HISTORY_CAPACITY);
        assert_eq!(history.first(), Some(&1));
        assert_eq!(history.last(), Some(&1_000));
    }
}
