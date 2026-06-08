use std::io::{self, Write};

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct Process {
    pid: u32,
    name: String,
    state: String,
    ppid: u32,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct SystemInfo {
    hostname: String,
    arch: String,
    kernel_version: String,
    processes: Vec<Process>,
    uptime_secs: u64,
}

fn main() {
    let stdout = &mut io::stdout();

    let info = SystemInfo {
        hostname: "scarlet".into(),
        arch: "riscv64".into(),
        kernel_version: "0.16.0".into(),
        processes: vec![
            Process { pid: 1, name: "init".into(), state: "running".into(), ppid: 0 },
            Process { pid: 2, name: "shell".into(), state: "waiting".into(), ppid: 1 },
            Process { pid: 3, name: "httpd".into(), state: "running".into(), ppid: 1 },
            Process { pid: 4, name: "cat".into(), state: "zombie".into(), ppid: 2 },
        ],
        uptime_secs: 3661,
    };

    let json = serde_json::to_string_pretty(&info).unwrap();
    writeln!(stdout, "=== Serialize ===").unwrap();
    writeln!(stdout, "{json}").unwrap();

    let roundtrip: SystemInfo = serde_json::from_str(&json).unwrap();
    writeln!(stdout, "\n=== Deserialize ===").unwrap();
    writeln!(stdout, "hostname: {}", roundtrip.hostname).unwrap();
    writeln!(stdout, "arch:     {}", roundtrip.arch).unwrap();
    writeln!(stdout, "uptime:   {}s ({}h {}m {}s)",
        roundtrip.uptime_secs,
        roundtrip.uptime_secs / 3600,
        (roundtrip.uptime_secs % 3600) / 60,
        roundtrip.uptime_secs % 60,
    ).unwrap();
    writeln!(stdout, "processes ({}):", roundtrip.processes.len()).unwrap();
    for p in &roundtrip.processes {
        writeln!(stdout, "  [{}] {} ({})", p.pid, p.name, p.state).unwrap();
    }

    let arr = serde_json::json!({
        "status": "ok",
        "load": [0.5, 0.3, 0.1],
        "features": ["tcp", "udp", "vfs", "hypervisor"],
    });
    writeln!(stdout, "\n=== json! macro ===").unwrap();
    writeln!(stdout, "{}", serde_json::to_string_pretty(&arr).unwrap()).unwrap();

    stdout.flush().unwrap();
}
