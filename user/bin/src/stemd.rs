//! Stem Daemon (stemd) - A systemd-like service manager for Scarlet
//!
//! This daemon:
//! - Reads a TOML configuration file to determine services and their dependencies
//! - Launches services in dependency order
//! - Maintains a wait thread to reap zombie processes
//! - Provides an IPC interface via LocalSocket for control commands

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{
    fs::File,
    println,
    socket::Socket,
    string::{String, ToString},
    task::{exit, fork, waitpid},
    thread,
    vec::Vec,
};

/// Service configuration
#[derive(Debug, Clone)]
struct Service {
    name: String,
    exec: String,
    depends: Vec<String>,
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
                                for dep in value.split(',') {
                                    let dep = Self::unquote(dep.trim());
                                    if !dep.is_empty() {
                                        depends.push(dep);
                                    }
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
        if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
            if s.len() >= 2 {
                return s[1..s.len() - 1].to_string();
            }
        }
        s.to_string()
    }
}

/// Launch a service by forking and executing
fn launch_service(service: &Service) -> Result<i32, &'static str> {
    println!("stemd: Launching service: {}", service.name);

    match fork() {
        0 => {
            // Child process: execute the service
            println!("stemd: Executing: {}", service.exec);

            // Parse the exec command (simple split by spaces)
            let parts: Vec<&str> = service.exec.split_whitespace().collect();
            if parts.is_empty() {
                println!("stemd: Invalid exec command");
                exit(-1);
            }

            let path = parts[0];
            let argv: Vec<&str> = parts.iter().copied().collect();
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
            println!("stemd: Service {} started with PID: {}", service.name, pid);
            Ok(pid)
        }
    }
}

/// Topological sort for dependency resolution
fn resolve_dependencies(services: &[Service]) -> Vec<Service> {
    let mut sorted = Vec::new();
    let mut visited = Vec::new();

    fn visit(
        service_name: &str,
        services: &[Service],
        sorted: &mut Vec<Service>,
        visited: &mut Vec<String>,
    ) {
        if visited.contains(&service_name.to_string()) {
            return;
        }

        visited.push(service_name.to_string());

        // Find the service
        if let Some(service) = services.iter().find(|s| s.name == service_name) {
            // Visit dependencies first
            for dep in &service.depends {
                visit(dep, services, sorted, visited);
            }

            // Add this service
            sorted.push(service.clone());
        }
    }

    for service in services {
        visit(&service.name, services, &mut sorted, &mut visited);
    }

    sorted
}

/// Wait thread: continuously reap zombie processes
fn wait_thread() {
    println!("stemd: Wait thread started");
    loop {
        let (pid, status) = waitpid(-1, 0);
        if pid > 0 {
            println!("stemd: Reaped process PID={}, status={}", pid, status);
        }
        // Sleep briefly to avoid busy-waiting
        thread::sleep(core::time::Duration::from_millis(100));
    }
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

                // Read command
                let mut buffer = [0u8; 256];
                match stream.read(&mut buffer) {
                    Ok(n) if n > 0 => {
                        let cmd = core::str::from_utf8(&buffer[..n]).unwrap_or("");
                        println!("stemd: Received command: {}", cmd.trim());

                        // Process command (simplified)
                        let response = match cmd.trim() {
                            "status" => "stemd is running\n",
                            "help" => "Commands: status, help\n",
                            _ => "Unknown command\n",
                        };

                        let _ = stream.write(response.as_bytes());
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
                if let Ok(s) = core::str::from_utf8(&buffer[..n]) {
                    content.push_str(s);
                }
            }
            Err(_) => return Err("Failed to read config file"),
        }
    }

    Ok(content)
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("stemd: Stem Daemon starting...");
    println!("stemd: PID={}", std::task::getpid());

    // Read configuration file
    let config_path = "/etc/stemd.toml";
    println!("stemd: Reading configuration from {}", config_path);

    let config_content = match read_config(config_path) {
        Ok(content) => content,
        Err(e) => {
            println!("stemd: {}", e);
            println!("stemd: Using default configuration");
            // Default configuration with login service
            String::from(
                r#"
[service.login]
exec = "/system/scarlet/bin/login"
depends = []
"#,
            )
        }
    };

    // Parse services
    let parser = ConfigParser::new(config_content);
    let services = parser.parse_services();

    println!("stemd: Found {} services", services.len());
    for service in &services {
        println!("stemd:   - {} (exec: {})", service.name, service.exec);
    }

    // Resolve dependencies and get launch order
    let launch_order = resolve_dependencies(&services);
    println!("stemd: Launch order resolved");

    // Launch services
    for service in &launch_order {
        if let Err(e) = launch_service(service) {
            println!("stemd: Failed to launch service {}: {}", service.name, e);
        }
        // Brief delay between service launches
        thread::sleep(core::time::Duration::from_millis(100));
    }

    println!("stemd: All services launched");

    // Spawn wait thread
    println!("stemd: Starting wait thread");
    let wait_handle = thread::spawn(wait_thread);

    // Spawn IPC thread
    println!("stemd: Starting IPC thread");
    let ipc_handle = thread::spawn(ipc_thread);

    // Main thread: just wait for threads to complete (they won't in normal operation)
    println!("stemd: Daemon running");

    // Wait for threads (they should run forever)
    let _ = wait_handle.join();
    let _ = ipc_handle.join();

    0
}
