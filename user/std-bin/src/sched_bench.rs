use std::env;
use std::process::ExitCode;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use scarlet_sys::{SCHED_UTIL_SCALE, Syscall, syscall1};

const DEFAULT_THREADS: usize = 4;
const DEFAULT_SECONDS: u64 = 15;

#[derive(Clone, Copy)]
enum Scenario {
    Cpu,
    Bursty,
    Sleepy,
    Mixed,
}

impl Scenario {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "cpu" => Some(Self::Cpu),
            "bursty" => Some(Self::Bursty),
            "sleepy" => Some(Self::Sleepy),
            "mixed" => Some(Self::Mixed),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Bursty => "bursty",
            Self::Sleepy => "sleepy",
            Self::Mixed => "mixed",
        }
    }

    fn worker_kind(self, worker_id: usize) -> WorkerKind {
        match self {
            Self::Cpu => WorkerKind::Cpu,
            Self::Bursty => WorkerKind::Bursty,
            Self::Sleepy => WorkerKind::Sleepy,
            Self::Mixed => match worker_id % 3 {
                0 => WorkerKind::Cpu,
                1 => WorkerKind::Bursty,
                _ => WorkerKind::Sleepy,
            },
        }
    }
}

#[derive(Clone, Copy)]
enum WorkerKind {
    Cpu,
    Bursty,
    Sleepy,
}

struct Config {
    scenario: Scenario,
    threads: usize,
    seconds: u64,
    performance: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scenario: Scenario::Mixed,
            threads: DEFAULT_THREADS,
            seconds: DEFAULT_SECONDS,
            performance: false,
        }
    }
}

fn main() -> ExitCode {
    let Ok(config) = parse_args() else {
        print_usage();
        return ExitCode::from(2);
    };

    println!(
        "[sched_bench] scenario={} threads={} seconds={} util_min={}",
        config.scenario.as_str(),
        config.threads,
        config.seconds,
        if config.performance {
            SCHED_UTIL_SCALE
        } else {
            0
        },
    );
    println!("[sched_bench] sample with: top --sort cpu");
    println!("[sched_bench] inspect placement with: cat /dev/cpuinfo");

    let stop = Arc::new(AtomicBool::new(false));
    let mut counters = Vec::new();
    let mut handles: Vec<thread::JoinHandle<()>> = Vec::new();

    for worker_id in 0..config.threads {
        let counter = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicU64::new(worker_id as u64));
        let stop_for_worker = stop.clone();
        let counter_for_worker = counter.clone();
        let checksum_for_worker = checksum.clone();
        let kind = config.scenario.worker_kind(worker_id);
        let util_min = if config.performance {
            SCHED_UTIL_SCALE
        } else {
            0
        };

        let spawn_result = thread::Builder::new().spawn(move || {
            run_worker(
                worker_id,
                kind,
                util_min,
                stop_for_worker,
                counter_for_worker,
                checksum_for_worker,
            );
        });

        match spawn_result {
            Ok(handle) => {
                counters.push((counter, checksum));
                handles.push(handle);
            }
            Err(error) => {
                println!(
                    "[sched_bench] failed to spawn worker {}: {}",
                    worker_id, error
                );
                stop.store(true, Ordering::SeqCst);
                return ExitCode::FAILURE;
            }
        }
    }

    let started_at = Instant::now();
    let mut previous_total = 0u64;
    while started_at.elapsed() < Duration::from_secs(config.seconds) {
        thread::sleep(Duration::from_secs(1));
        let total = total_iterations(&counters);
        let delta = total.saturating_sub(previous_total);
        previous_total = total;
        println!(
            "[sched_bench] t={:>3}s iter/s={:>12} total={:>14}",
            started_at.elapsed().as_secs(),
            delta,
            total,
        );
    }

    stop.store(true, Ordering::SeqCst);
    for handle in handles {
        let _ = handle.join();
    }

    let total = total_iterations(&counters);
    let checksum = total_checksum(&counters);
    println!(
        "[sched_bench] done total={} checksum=0x{:016x}",
        total, checksum
    );
    ExitCode::SUCCESS
}

fn parse_args() -> Result<Config, ()> {
    let args: Vec<String> = env::args().collect();
    let mut config = Config::default();
    let mut index = 1;

    while index < args.len() {
        match args[index].as_str() {
            "--scenario" | "-s" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(());
                };
                config.scenario = Scenario::parse(value).ok_or(())?;
            }
            "--threads" | "-t" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(());
                };
                config.threads = value.parse::<usize>().map_err(|_| ())?;
            }
            "--seconds" | "-d" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(());
                };
                config.seconds = value.parse::<u64>().map_err(|_| ())?;
            }
            "--performance" | "-p" => {
                config.performance = true;
            }
            "--help" | "-h" => {
                return Err(());
            }
            _ => return Err(()),
        }
        index += 1;
    }

    if config.threads == 0 || config.seconds == 0 {
        return Err(());
    }
    Ok(config)
}

fn print_usage() {
    println!(
        "usage: sched_bench [--scenario cpu|bursty|sleepy|mixed] [--threads N] [--seconds N] [--performance]"
    );
    println!("  --performance requests util_min=1024 for worker threads");
}

fn run_worker(
    worker_id: usize,
    kind: WorkerKind,
    util_min: u32,
    stop: Arc<AtomicBool>,
    counter: Arc<AtomicU64>,
    checksum: Arc<AtomicU64>,
) {
    if util_min != 0 {
        let result = syscall1(Syscall::SetTaskUtilMin, util_min as usize);
        if result == usize::MAX {
            println!(
                "[sched_bench] worker {} failed to set util_min={}",
                worker_id, util_min
            );
        }
    }

    let mut state = worker_id as u64 ^ 0x9e37_79b9_7f4a_7c15;
    while !stop.load(Ordering::Relaxed) {
        match kind {
            WorkerKind::Cpu => {
                state = burn(state, 90_000);
                counter.fetch_add(90_000, Ordering::Relaxed);
            }
            WorkerKind::Bursty => {
                state = burn(state, 35_000);
                counter.fetch_add(35_000, Ordering::Relaxed);
                thread::sleep(Duration::from_millis(4));
            }
            WorkerKind::Sleepy => {
                state = burn(state, 6_000);
                counter.fetch_add(6_000, Ordering::Relaxed);
                thread::sleep(Duration::from_millis(18));
            }
        }
        checksum.store(state, Ordering::Relaxed);
    }
}

fn burn(mut state: u64, iterations: u64) -> u64 {
    for _ in 0..iterations {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state ^= state.rotate_left(17);
    }
    state
}

fn total_iterations(counters: &[(Arc<AtomicU64>, Arc<AtomicU64>)]) -> u64 {
    counters
        .iter()
        .map(|(counter, _)| counter.load(Ordering::Relaxed))
        .sum()
}

fn total_checksum(counters: &[(Arc<AtomicU64>, Arc<AtomicU64>)]) -> u64 {
    counters.iter().fold(0, |acc, (_, checksum)| {
        acc ^ checksum.load(Ordering::Relaxed)
    })
}
