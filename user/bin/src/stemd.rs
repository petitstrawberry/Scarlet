//! Stem Daemon (stemd) - A systemd-like service manager for Scarlet
//!
//! This daemon:
//! - Reads a TOML configuration file to determine services and their dependencies
//! - Launches services in dependency order
//! - Reaps child processes using waitpid(-1, 0)
//! - Provides an IPC interface via LocalSocket for control commands

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::sync::atomic::fence;
use std::{
    fs::File,
    handle::Handle,
    println,
    socket::Socket,
    string::{String, ToString},
    task::{exit, fork, waitpid},
    thread,
    vec::Vec,
};

fn try_attach_stdio_to_path(path: &str) {
    // Best-effort: rebind stdio handles (0/1/2) to the given path.
    // Follow the same approach as `sh`: close 0/1/2 then `duplicate()` so that
    // the duplicated handles get assigned to 0,1,2.

    let Ok(h) = Handle::open(path, 2) else {
        return;
    };

    // Rebind each stdio FD independently to respect the kernel handle allocator's LIFO behavior.
    // This mirrors `sh`: close FD N, then duplicate the target handle and expect it to become FD N.
    for fd in 0..3 {
        if let Ok(old) = unsafe { Handle::from_raw(fd) } {
            let _ = old.close();
        }
        match h.duplicate() {
            Ok(new_h) => {
                if new_h.as_raw() == fd {
                    core::mem::forget(new_h);
                } else {
                    // If we didn't get the expected FD, leave things alone (best effort).
                    let _ = new_h.close();
                }
            }
            Err(_) => break,
        }
    }

    // Close the original handle to avoid leaking it.
    let _ = h.close();
}

fn try_attach_stdio_to_tty(tty_path: &str) {
    try_attach_stdio_to_path(tty_path);
}

fn try_attach_stdio_to_null() {
    try_attach_stdio_to_path("/dev/null");
}

/// Service configuration
#[derive(Debug, Clone)]
struct Service {
    name: String,
    exec: String,
    depends: Vec<String>,
    after: Vec<String>,
    tty: Option<String>,
    order: i32,
}

/// Simple TOML parser for service configuration
struct ConfigParser {
    content: String,
}

impl ConfigParser {
    fn new(content: String) -> Self {
        Self { content }
    }

    /// Parse the configuration file and extract services
    fn parse_services(&self) -> Vec<Service> {
        let mut services = Vec::new();
        let lines: Vec<&str> = self.content.lines().collect();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i].trim();

            // Check for service section header [service.name]
            if line.starts_with("[service.") && line.ends_with(']') {
                let service_name = line[9..line.len() - 1].to_string();
                let mut exec = String::new();
                let mut depends = Vec::new();
                let mut after = Vec::new();
                let mut tty: Option<String> = None;
                let mut order: i32 = 0;

                // Parse service properties
                i += 1;
                while i < lines.len() {
                    let prop_line = lines[i].trim();

                    // Empty line or start of next section
                    if prop_line.is_empty() || prop_line.starts_with('[') {
                        break;
                    }

                    // Skip comments
                    if prop_line.starts_with('#') {
                        i += 1;
                        continue;
                    }

                    // Parse key = value
                    if let Some(eq_pos) = prop_line.find('=') {
                        let key = prop_line[..eq_pos].trim();
                        let value = prop_line[eq_pos + 1..].trim();

                        match key {
                            "exec" => {
                                // Remove quotes if present
                                exec = Self::unquote(value);
                            }
                            "depends" => {
                                // Parse comma-separated dependencies
                                // Note: Supports both quoted strings and array-like syntax
                                // E.g., depends = "service1, service2" or depends = ["service1", "service2"]
                                let value = value.trim_start_matches('[').trim_end_matches(']');
                                for dep in value.split(',') {
                                    let dep = Self::unquote(dep.trim());
                                    if !dep.is_empty() {
                                        depends.push(dep);
                                    }
                                }
                            }
                            "after" => {
                                // Ordering-only constraints (systemd-like After=)
                                // E.g., after = ["sws", "net"]
                                let value = value.trim_start_matches('[').trim_end_matches(']');
                                for name in value.split(',') {
                                    let name = Self::unquote(name.trim());
                                    if !name.is_empty() {
                                        after.push(name);
                                    }
                                }
                            }
                            "tty" => {
                                let t = Self::unquote(value);
                                if !t.is_empty() {
                                    tty = Some(t);
                                }
                            }
                            "order" => {
                                if let Some(v) = Self::parse_i32(value) {
                                    order = v;
                                }
                            }
                            _ => {}
                        }
                    }

                    i += 1;
                }

                if !exec.is_empty() {
                    services.push(Service {
                        name: service_name,
                        exec,
                        depends,
                        after,
                        tty,
                        order,
                    });
                }

                // Don't increment i here, as we might be at the start of the next section
                continue;
            }

            i += 1;
        }

        services
    }

    /// Remove surrounding quotes from a string
    fn unquote(s: &str) -> String {
        let s = s.trim();
        if ((s.starts_with('"') && s.ends_with('"'))
            || (s.starts_with('\'') && s.ends_with('\'')))
            && s.len() >= 2
        {
            return s[1..s.len() - 1].to_string();
        }
        s.to_string()
    }

    fn parse_i32(s: &str) -> Option<i32> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let (neg, digits) = match s.as_bytes().first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, s),
        };

        if digits.is_empty() {
            return None;
        }

        let mut acc: i32 = 0;
        for ch in digits.bytes() {
            if !(b'0'..=b'9').contains(&ch) {
                return None;
            }
            let digit = (ch - b'0') as i32;
            acc = acc.saturating_mul(10).saturating_add(digit);
        }

        Some(if neg { acc.saturating_neg() } else { acc })
    }
}

/// Launch a service by forking and executing
fn launch_service(service: &Service) -> Result<i32, &'static str> {
    println!("stemd: Launching service: {}", service.name);

    let pid = fork();

    match pid {
        0 => {
            // Child process: execute the service
            // println!("stemd: Executing: {}", service.exec);

            if let Some(tty) = &service.tty {
                try_attach_stdio_to_tty(tty);
            } else {
                try_attach_stdio_to_null();
            }

            fence(core::sync::atomic::Ordering::SeqCst);

            // Parse the exec command (simple split by spaces)
            let parts: Vec<&str> = service.exec.split_whitespace().collect();
            if parts.is_empty() {
                println!("stemd: Invalid exec command");
                exit(-1);
            }

            let path = parts[0];
            let argv: Vec<&str> = parts.to_vec();
            let envp: Vec<&str> = Vec::new();

            if std::task::execve(path, &argv, &envp) != 0 {
                println!("stemd: Failed to execve {}", path);
                exit(-1);
            }
            unreachable!();
        }
        -1 => {
            println!("stemd: Failed to fork for service: {}", service.name);
            Err("Fork failed")
        }
        pid => {
            // println!("stemd: Service {} started with PID: {}", service.name, pid);
            Ok(pid)
        }
    }
}

/// Topological sort for dependency resolution
fn resolve_dependencies(services: &[Service]) -> Vec<Service> {
    // Kahn's algorithm.
    // Guarantees: `depends` and `after` ordering constraints are respected;
    // among runnable services, smaller `order` starts first.

    let n = services.len();
    if n == 0 {
        return Vec::new();
    }

    fn index_of(services: &[Service], name: &str) -> Option<usize> {
        services.iter().position(|s| s.name == name)
    }

    // edges[from] = list of services that depend on `from`
    let mut edges: Vec<Vec<usize>> = Vec::new();
    for _ in 0..n {
        edges.push(Vec::new());
    }
    let mut indegree: Vec<usize> = Vec::new();
    indegree.resize(n, 0);

    for (to_idx, svc) in services.iter().enumerate() {
        for dep_name in &svc.depends {
            match index_of(services, dep_name) {
                Some(from_idx) => {
                    edges[from_idx].push(to_idx);
                    indegree[to_idx] += 1;
                }
                None => {
                    println!(
                        "stemd: Warning: Service '{}' depends on unknown service '{}'",
                        svc.name, dep_name
                    );
                }
            }
        }

        for after_name in &svc.after {
            match index_of(services, after_name) {
                Some(from_idx) => {
                    edges[from_idx].push(to_idx);
                    indegree[to_idx] += 1;
                }
                None => {
                    println!(
                        "stemd: Warning: Service '{}' has after='{}' but it is not defined (ignored)",
                        svc.name, after_name
                    );
                }
            }
        }
    }

    let mut ready: Vec<usize> = Vec::new();
    for (idx, &deg) in indegree.iter().enumerate() {
        if deg == 0 {
            ready.push(idx);
        }
    }

    fn pick_best(services: &[Service], ready: &mut Vec<usize>) -> Option<usize> {
        if ready.is_empty() {
            return None;
        }

        let mut best_pos: usize = 0;
        for pos in 1..ready.len() {
            let a = &services[ready[pos]];
            let b = &services[ready[best_pos]];
            if a.order < b.order || (a.order == b.order && a.name < b.name) {
                best_pos = pos;
            }
        }

        Some(ready.swap_remove(best_pos))
    }

    let mut sorted: Vec<Service> = Vec::new();
    while let Some(idx) = pick_best(services, &mut ready) {
        sorted.push(services[idx].clone());
        for &next in &edges[idx] {
            if indegree[next] > 0 {
                indegree[next] -= 1;
                if indegree[next] == 0 {
                    ready.push(next);
                }
            }
        }
    }

    if sorted.len() != n {
        println!(
            "stemd: Warning: Could not resolve full dependency graph (cycle or missing deps/after)."
        );

        // Append remaining services in deterministic order (order, then name).
        let mut remaining: Vec<usize> = Vec::new();
        for idx in 0..n {
            let name = &services[idx].name;
            if !sorted.iter().any(|s| &s.name == name) {
                remaining.push(idx);
            }
        }

        remaining.sort_by(|&a, &b| {
            let sa = &services[a];
            let sb = &services[b];
            sa.order
                .cmp(&sb.order)
                .then_with(|| sa.name.cmp(&sb.name))
        });

        for idx in remaining {
            sorted.push(services[idx].clone());
        }
    }

    sorted
}

/// IPC thread: accept commands via socket
fn ipc_thread() {
    println!("stemd: IPC thread started");

    let socket_path = "/tmp/stemd.sock";

    // Create and bind socket
    let server = match Socket::new() {
        Ok(s) => s,
        Err(e) => {
            println!("stemd: Failed to create IPC socket: {:?}", e);
            return;
        }
    };

    if let Err(e) = server.bind(socket_path) {
        println!("stemd: Failed to bind IPC socket: {:?}", e);
        return;
    }

    if let Err(e) = server.listen(5) {
        println!("stemd: Failed to listen on IPC socket: {:?}", e);
        return;
    }

    println!("stemd: IPC socket listening at {}", socket_path);

    // Accept connections
    loop {
        match server.accept() {
            Ok(client) => {
                println!("stemd: IPC client connected");

                let stream = match client.as_stream() {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                // Read command (max 256 bytes)
                // Note: Commands longer than 256 bytes will be truncated
                let mut buffer = [0u8; 256];
                match stream.read(&mut buffer) {
                    Ok(n) if n > 0 => {
                        // Try to parse command as UTF-8
                        match core::str::from_utf8(&buffer[..n]) {
                            Ok(cmd) => {
                                println!("stemd: Received command: {}", cmd.trim());

                                // Process command (simplified)
                                let response = match cmd.trim() {
                                    "status" => "stemd is running\n",
                                    "help" => "Commands: status, help\n",
                                    _ => "Unknown command\n",
                                };

                                let _ = stream.write(response.as_bytes());
                            }
                            Err(_) => {
                                let error_msg = "Error: Invalid UTF-8 in command\n";
                                let _ = stream.write(error_msg.as_bytes());
                                println!("stemd: Received invalid UTF-8 command");
                            }
                        }
                    }
                    _ => {}
                }
            }
            Err(_) => {
                thread::sleep(core::time::Duration::from_millis(100));
            }
        }
    }
}

/// Read configuration file
fn read_config(path: &str) -> Result<String, &'static str> {
    let mut file = File::open(path).map_err(|_| "Failed to open config file")?;

    let mut content = String::new();
    let mut buffer = [0u8; 4096];

    loop {
        match file.read(&mut buffer) {
            Ok(0) => break, // EOF
            Ok(n) => {
                // Ensure UTF-8 validity, otherwise fail
                match core::str::from_utf8(&buffer[..n]) {
                    Ok(s) => content.push_str(s),
                    Err(_) => return Err("Config file contains invalid UTF-8"),
                }
            }
            Err(_) => return Err("Failed to read config file"),
        }
    }

    Ok(content)
}

/// Read all configuration files from a directory
fn read_config_dir(dir_path: &str) -> Result<String, &'static str> {
    use std::fs::list_directory;

    let mut combined_content = String::new();

    // Try to list directory entries
    match list_directory(dir_path) {
        Ok(entries) => {
            println!(
                "stemd: Reading configuration from directory: {}",
                dir_path
            );

            // Filter and sort .toml files
            let mut toml_files = Vec::new();
            for entry in entries {
                if entry.name == "." || entry.name == ".." {
                    continue;
                }

                // Only process .toml files
                if entry.is_file() && entry.name.ends_with(".toml") {
                    toml_files.push(entry.name);
                }
            }

            // Sort files for consistent ordering
            toml_files.sort();

            if toml_files.is_empty() {
                println!("stemd: No .toml files found in {}", dir_path);
                return Err("No configuration files found in directory");
            }

            // Read each file and combine content
            for filename in toml_files {
                use std::format;
                let file_path = format!("{}/{}", dir_path, filename);
                println!("stemd:   Loading {}", file_path);

                match read_config(&file_path) {
                    Ok(content) => {
                        combined_content.push_str(&content);
                        combined_content.push('\n'); // Add newline between files
                    }
                    Err(e) => {
                        println!("stemd:   Warning: Failed to read {}: {}", file_path, e);
                        // Continue with other files
                    }
                }
            }

            if combined_content.is_empty() {
                return Err("All configuration files failed to load");
            }

            Ok(combined_content)
        }
        Err(_) => Err("Failed to read configuration directory"),
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("stemd: Stem Daemon starting...");
    println!("stemd: PID={}", std::task::getpid());

    let started_by_init = std::task::getppid() == 1;

    // If stemd is started manually from an interactive shell, don't keep the shell
    // blocked. Fork once and let the parent exit immediately.
    if !started_by_init {
        match fork() {
            0 => {
                // child continues as daemon
                try_attach_stdio_to_null();
            }
            -1 => {
                println!("stemd: Failed to daemonize (fork failed)");
                return 1;
            }
            pid => {
                println!("stemd: Daemonized (PID={})", pid);
                return 0;
            }
        }
    }

    // Read configuration.
    // Note: current filesystem layout copies userland under `/system/scarlet`,
    // so configs may live under `/system/scarlet/etc` instead of `/etc`.
    let config_dirs = ["/etc/stemd.d", "/system/scarlet/etc/stemd.d"];
    let config_files = ["/etc/stemd.toml", "/system/scarlet/etc/stemd.toml"];

    let mut config_content: Option<String> = None;

    for dir in &config_dirs {
        match read_config_dir(dir) {
            Ok(content) => {
                config_content = Some(content);
                break;
            }
            Err(e) => {
                println!("stemd: Failed to read from {}: {}", dir, e);
            }
        }
    }

    if config_content.is_none() {
        for file in &config_files {
            println!("stemd: Trying fallback configuration file {}", file);
            match read_config(file) {
                Ok(content) => {
                    config_content = Some(content);
                    break;
                }
                Err(e) => {
                    println!("stemd: {}", e);
                }
            }
        }
    }

    let config_content = config_content.unwrap_or_else(|| {
        println!("stemd: Using default configuration");
        // Default configuration with login service
        String::from(
            r#"
[service.login]
exec = "/bin/login"
depends = []
tty = "/dev/tty0"
"#,
        )
    });

    // Parse services
    let parser = ConfigParser::new(config_content);
    let services = parser.parse_services();

    println!("stemd: Found {} services", services.len());
    for service in &services {
        println!(
            "stemd:   - {} (exec: {}, tty: {:?}, order: {}, depends: {}, after: {})",
            service.name,
            service.exec,
            service.tty,
            service.order,
            service.depends.len(),
            service.after.len(),
        );
    }

    // Resolve dependencies and get launch order
    let launch_order = resolve_dependencies(&services);
    println!("stemd: Launch order resolved");

    // Launch services.
    println!("stemd: Launching services...");

    for service in &launch_order {
        // Avoid stealing an interactive TTY when stemd is started manually from a shell.
        // TTY-bound services (like login/getty equivalents) should normally be started by init.
        if !started_by_init && service.tty.is_some() {
            println!(
                "stemd: Skipping tty service '{}' (not started by init)",
                service.name
            );
            continue;
        }
        if let Err(e) = launch_service(service) {
            println!("stemd: Failed to launch service {}: {}", service.name, e);
        }
        fence(core::sync::atomic::Ordering::SeqCst);
    }

    println!("stemd: All services launched");

    // Spawn IPC thread
    println!("stemd: Starting IPC thread");
    let _ipc_handle = thread::spawn(ipc_thread);


    loop {
        let (pid, status) = waitpid(-1, 0);
        if pid < 0 {
            continue;
        }
        println!("stemd: Reaped child PID={} status={}", pid, status);
    }
}
