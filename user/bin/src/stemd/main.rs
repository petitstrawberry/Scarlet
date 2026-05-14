//! Stem Daemon (stemd) - A systemd-like service manager for Scarlet
//!
//! This daemon:
//! - Reads a TOML configuration file to determine services and their dependencies
//! - Launches services in dependency order
//! - Reaps child processes using waitpid(-1, 0)
//! - Provides an IPC interface via LocalSocket for control commands
//! - Supports .desktop files for application definitions

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::sync::atomic::fence;
use sbus_client as sbus;
use std::{
    format,
    fs::File,
    handle::Handle,
    println,
    socket::Socket,
    string::{String, ToString},
    sync::Mutex,
    task::{exit, fork, waitpid},
    thread, vec,
    vec::Vec,
};
use sws_client::Connection;

// Import protocol and desktop modules
mod desktop;
mod protocol;

use desktop::{load_desktop_files, lookup_app};
use protocol::cmd;

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

/// Running application tracking
#[derive(Debug, Clone)]
struct RunningApp {
    app_id: String,
    pid: i32,
    exec_path: String,
}

// Global tracking for running applications
// Thread-safe using Mutex (static, not mutable)
static RUNNING_APPS: Mutex<Vec<RunningApp>> = Mutex::new(Vec::new());

// Global sbus connection for receiving method calls
// Wrapped in Option to handle initialization
static SBUS_CONNECTION: Mutex<Option<sbus::Connection>> = Mutex::new(None);

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
        if ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
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
            if !ch.is_ascii_digit() {
                return None;
            }
            let digit = (ch - b'0') as i32;
            acc = acc.saturating_mul(10).saturating_add(digit);
        }

        Some(if neg { acc.saturating_neg() } else { acc })
    }
}

/// Add a running app to the tracking list
fn add_running_app(app_id: String, pid: i32, exec_path: String) {
    let mut apps = RUNNING_APPS.lock();
    apps.push(RunningApp {
        app_id,
        pid,
        exec_path,
    });
}

/// Remove a running app from the tracking list by PID
fn remove_running_app_by_pid(pid: i32) -> bool {
    let mut apps = RUNNING_APPS.lock();
    if let Some(pos) = apps.iter().position(|app| app.pid == pid) {
        apps.remove(pos);
        return true;
    }
    false
}

/// Find a running app by app_id
fn find_running_app(app_id: &str) -> Option<RunningApp> {
    let apps = RUNNING_APPS.lock();
    apps.iter().find(|app| app.app_id == app_id).cloned()
}

/// Focus a window by app_id
/// This queries SWS for the window list and tries to find a window matching the app_id
fn focus_window_by_app_id(app_id: &str) -> Result<(), &'static str> {
    println!("stemd: focus_window_by_app_id START for app_id={}", app_id);

    // Look up the app to get its name for matching
    let search_terms = if let Some(entry) = lookup_app(app_id) {
        println!(
            "stemd: Looking for window with title containing: {}",
            entry.name
        );
        vec![entry.name.clone(), app_id.to_string()]
    } else {
        println!("stemd: App not in registry, using app_id directly");
        vec![app_id.to_string()]
    };

    // Try to connect to SWS
    println!("stemd: Connecting to SWS...");
    let mut conn = match Connection::connect_default() {
        Ok(c) => {
            println!("stemd: Successfully connected to SWS");
            c
        }
        Err(e) => {
            println!("stemd: Failed to connect to SWS: {:?}", e);
            return Err("Failed to connect to SWS");
        }
    };

    // Get the list of windows
    println!("stemd: Calling get_window_list()...");
    let windows = match conn.get_window_list() {
        Ok(w) => {
            println!("stemd: get_window_list() returned {} windows", w.len());
            w
        }
        Err(e) => {
            println!("stemd: Failed to get window list: {:?}", e);
            return Err("Failed to get window list");
        }
    };

    println!("stemd: Checking {} windows for match", windows.len());

    // Look for a window whose app_id or title matches
    for window in &windows {
        println!(
            "stemd: Window #{} app_id='{}' title='{}' visible={}",
            window.window_id, window.app_id, window.title, window.visible
        );

        // First try exact app_id match (include hidden/minimized windows)
        if window.app_id == app_id {
            println!(
                "stemd: Found matching window #{} by app_id (visible={})",
                window.window_id, window.visible
            );
            return focus_window_id(&mut conn, window.window_id);
        }

        // Fallback to title matching for backwards compatibility (include hidden/minimized windows)
        for search_term in &search_terms {
            if window.title.contains(search_term) {
                println!(
                    "stemd: Found matching window #{} by title (visible={})",
                    window.window_id, window.visible
                );
                return focus_window_id(&mut conn, window.window_id);
            }
        }
    }

    // No matching window found
    println!("stemd: No matching window found for app_id={}", app_id);
    Err("No window found for app_id")
}

/// Focus a window by ID
fn focus_window_id(conn: &mut sws_client::Connection, window_id: u32) -> Result<(), &'static str> {
    // Focus the window (this also restores if minimized)
    println!("stemd: Focusing window #{}", window_id);
    match conn.focus_window_any(window_id) {
        Ok(()) => {
            println!("stemd: Successfully focused window #{}", window_id);
            Ok(())
        }
        Err(e) => {
            println!("stemd: Failed to focus window: {:?}", e);
            Err("Failed to focus window")
        }
    }
}

/// Launch an application or focus an existing window
/// If exec_path is empty, look up the app from the registry
fn launch_or_focus(app_id: &str, exec_path: Option<&str>) -> Result<(), &'static str> {
    println!(
        "stemd: launch_or_focus called with app_id={}, exec_path={:?}",
        app_id, exec_path
    );

    // First, look up exec_path from registry if needed (before holding RUNNING_APPS lock)
    // This avoids potential lock ordering issues
    println!("stemd: Looking up app in registry...");
    let exec_path_resolved = match exec_path {
        Some(path) if !path.is_empty() => {
            println!("stemd: Using provided exec_path: {}", path);
            Some(path.to_string())
        }
        _ => {
            println!("stemd: Looking up {} from .desktop files...", app_id);
            let result = lookup_app(app_id).map(|entry| {
                println!("stemd: Found app: {} -> {}", entry.name, entry.exec);
                entry.exec
            });
            if result.is_none() {
                println!("stemd: App not found in registry");
            }
            result
        }
    };

    // Now check if app is already running (acquire RUNNING_APPS lock separately)
    println!("stemd: Checking if app is already running...");
    if let Some(app) = find_running_app(app_id) {
        // App is running, try to focus its window
        println!(
            "stemd: App '{}' is already running (PID={}), focusing window",
            app_id, app.pid
        );

        if let Err(e) = focus_window_by_app_id(app_id) {
            println!("stemd: Failed to focus window: {}", e);
            // Even if focusing fails, don't launch a new instance
            return Err(e);
        }
        println!("stemd: Successfully focused window");
        return Ok(());
    }

    println!("stemd: App is not running, preparing to launch...");

    // App is not running, determine exec_path
    let exec_path = match exec_path_resolved {
        Some(path) => path,
        None => {
            println!("stemd: App '{}' not found in registry", app_id);
            return Err("App not found in registry and no exec_path provided");
        }
    };

    println!(
        "stemd: Looked up '{}' from registry: exec={}",
        app_id, exec_path
    );

    // App is not running, launch it
    println!("stemd: Launching app '{}' with exec: {}", app_id, exec_path);

    let pid = fork();
    match pid {
        0 => {
            // Child process
            try_attach_stdio_to_null();

            fence(core::sync::atomic::Ordering::SeqCst);

            // Parse the exec command
            let parts: Vec<&str> = exec_path.split_whitespace().collect();
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
            println!("stemd: Failed to fork for app: {}", app_id);
            Err("Fork failed")
        }
        pid => {
            // Parent process - track the launched app
            add_running_app(app_id.to_string(), pid, exec_path.to_string());
            println!("stemd: App '{}' launched with PID: {}", app_id, pid);
            Ok(())
        }
    }
}

/// Get a list of all running applications
fn get_running_apps_list() -> Vec<RunningApp> {
    let apps = RUNNING_APPS.lock();
    apps.clone()
}

/// Get the app_id of the currently focused window
fn get_focused_window_app_id() -> Option<String> {
    // Try to connect to SWS
    let mut conn = match Connection::connect_default() {
        Ok(c) => c,
        Err(_e) => {
            // Silent failure - don't log on every poll
            return None;
        }
    };

    // Get the list of windows
    let windows = match conn.get_window_list() {
        Ok(w) => w,
        Err(_e) => {
            // Silent failure - don't log on every poll
            return None;
        }
    };

    // Find the focused window
    for window in &windows {
        if window.focused {
            return Some(window.app_id.clone());
        }
    }

    None
}

/// Get menu titles for an application
/// Returns a list of menu titles (e.g., ["Notepad", "File", "Edit", "View", "Help"])
///
/// DEPRECATED: This functionality is now handled by SWS.
/// Applications register their menus when creating surfaces via SWS.
/// The TaskBar receives menu information through FOCUS_CHANGED events.
/// This function is kept for backward compatibility but should not be used.
fn get_app_menu_titles(app_id: &str) -> Vec<String> {
    // Look up the app to get its name
    let app_name = if let Some(entry) = lookup_app(app_id) {
        entry.name
    } else {
        // Fallback: use app_id as name
        app_id.to_string()
    };

    // Build menu list based on app type
    let mut menus = vec![app_name];

    // Add app-specific menus based on app_id
    if app_id.contains("notepad") || app_id.contains("text") || app_id.contains("editor") {
        menus.extend(vec![
            String::from("File"),
            String::from("Edit"),
            String::from("View"),
            String::from("Help"),
        ]);
    } else if app_id.contains("terminal") {
        menus.extend(vec![
            String::from("Shell"),
            String::from("Edit"),
            String::from("View"),
            String::from("Help"),
        ]);
    } else if app_id.contains("filer") || app_id.contains("file") {
        menus.extend(vec![
            String::from("File"),
            String::from("Edit"),
            String::from("View"),
            String::from("Go"),
            String::from("Help"),
        ]);
    } else if app_id.contains("settings") || app_id.contains("config") {
        menus.extend(vec![
            String::from("Edit"),
            String::from("View"),
            String::from("Help"),
        ]);
    } else if app_id.contains("launcher") {
        menus.extend(vec![
            String::from("File"),
            String::from("View"),
            String::from("Help"),
        ]);
    } else if app_id.contains("clock") {
        menus.extend(vec![String::from("View"), String::from("Help")]);
    } else {
        // Generic menus for unknown apps
        menus.extend(vec![
            String::from("File"),
            String::from("Edit"),
            String::from("View"),
            String::from("Window"),
            String::from("Help"),
        ]);
    }

    menus
}

/// Launch an application by app_id (looks up from .desktop registry)
/// Returns the PID of the launched process
fn launch_app_by_id(app_id: &str) -> Result<i32, &'static str> {
    println!("stemd: launch_app_by_id called with app_id={}", app_id);

    // Look up exec_path from registry
    println!("stemd: Looking up {} from .desktop files...", app_id);
    let exec_path = match lookup_app(app_id) {
        Some(entry) => {
            println!("stemd: Found app: {} -> {}", entry.name, entry.exec);
            entry.exec
        }
        None => {
            println!("stemd: App '{}' not found in registry", app_id);
            return Err("App not found in registry");
        }
    };

    // Fork and execute
    let pid = fork();
    match pid {
        0 => {
            // Child process
            try_attach_stdio_to_null();

            fence(core::sync::atomic::Ordering::SeqCst);

            // Parse the exec command
            let parts: Vec<&str> = exec_path.split_whitespace().collect();
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
            println!("stemd: Failed to fork for app: {}", app_id);
            Err("Fork failed")
        }
        pid => {
            // Parent process - track the launched app
            add_running_app(app_id.to_string(), pid, exec_path.to_string());
            println!("stemd: App '{}' launched with PID: {}", app_id, pid);
            Ok(pid)
        }
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
    let mut indegree: Vec<usize> = vec![0; n];

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
            sa.order.cmp(&sb.order).then_with(|| sa.name.cmp(&sb.name))
        });

        for idx in remaining {
            sorted.push(services[idx].clone());
        }
    }

    sorted
}

fn handle_shutdown() {
    use std::task::{ShutdownType, shutdown};
    println!("stemd: Initiating system shutdown...");

    // TODO: Implement proper shutdown sequence:
    // 1. Stop all services in reverse dependency order
    // 2. Send SIGTERM to all child processes
    // 3. Wait for processes to exit gracefully (with timeout)
    // 4. Send SIGKILL to remaining processes
    // 5. Sync all filesystems (call sync() on all open files)
    // 6. Unmount all filesystems
    // 7. Finally call kernel shutdown syscall
    //
    // Currently: Directly call kernel shutdown (simpler fallback)

    shutdown(ShutdownType::PowerOff);
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
                let stream = match client.as_stream() {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                // Read command (larger buffer for binary commands)
                let mut buffer = [0u8; 1024];
                match stream.read(&mut buffer) {
                    Ok(n) if n > 0 => {
                        // Check if this is a binary command (LAUNCH_OR_FOCUS)
                        if buffer[0] == cmd::LAUNCH_OR_FOCUS {
                            // Parse LAUNCH_OR_FOCUS command
                            // Format: cmd(1) + app_id_len(4) + app_id + exec_path_len(4) + exec_path
                            if n >= 9 {
                                let app_id_len = u32::from_le_bytes([
                                    buffer[1], buffer[2], buffer[3], buffer[4],
                                ]) as usize;

                                let exec_path_offset = 5 + app_id_len;
                                if n >= exec_path_offset + 4 {
                                    let exec_path_len = u32::from_le_bytes([
                                        buffer[exec_path_offset],
                                        buffer[exec_path_offset + 1],
                                        buffer[exec_path_offset + 2],
                                        buffer[exec_path_offset + 3],
                                    ])
                                        as usize;

                                    let total_len = exec_path_offset + 4 + exec_path_len;
                                    if n >= total_len {
                                        let app_id =
                                            core::str::from_utf8(&buffer[5..5 + app_id_len]);
                                        let exec_path = core::str::from_utf8(
                                            &buffer[exec_path_offset + 4
                                                ..exec_path_offset + 4 + exec_path_len],
                                        );

                                        match (app_id, exec_path) {
                                            (Ok(app_id), Ok(exec_path)) => {
                                                println!(
                                                    "stemd: LAUNCH_OR_FOCUS app_id={} exec={}",
                                                    app_id, exec_path
                                                );

                                                let exec_path_arg = if exec_path.is_empty() {
                                                    None
                                                } else {
                                                    Some(exec_path)
                                                };

                                                let response =
                                                    match launch_or_focus(app_id, exec_path_arg) {
                                                        Ok(()) => {
                                                            "OK: Launched or focused\n".as_bytes()
                                                        }
                                                        Err(e) => {
                                                            // Build error message as byte array directly
                                                            let error_prefix = "ERROR: ";
                                                            let error_suffix = "\n";
                                                            let mut error_msg = Vec::new();
                                                            error_msg.extend_from_slice(
                                                                error_prefix.as_bytes(),
                                                            );
                                                            error_msg
                                                                .extend_from_slice(e.as_bytes());
                                                            error_msg.extend_from_slice(
                                                                error_suffix.as_bytes(),
                                                            );
                                                            // Note: This is a temporary solution - the Vec will be dropped
                                                            // but we're returning a slice. For IPC, we should handle this differently.
                                                            // For now, use a static error message.
                                                            "ERROR: Failed to launch or focus\n"
                                                                .as_bytes()
                                                        }
                                                    };

                                                let _ = stream.write(response);
                                            }
                                            _ => {
                                                let error_msg =
                                                    "ERROR: Invalid UTF-8 in parameters\n";
                                                let _ = stream.write(error_msg.as_bytes());
                                            }
                                        }
                                    } else {
                                        let error_msg = "ERROR: Incomplete command\n";
                                        let _ = stream.write(error_msg.as_bytes());
                                    }
                                } else {
                                    let error_msg = "ERROR: Incomplete command\n";
                                    let _ = stream.write(error_msg.as_bytes());
                                }
                            } else {
                                let error_msg = "ERROR: Malformed LAUNCH_OR_FOCUS command\n";
                                let _ = stream.write(error_msg.as_bytes());
                            }
                        } else if buffer[0] == cmd::SHUTDOWN {
                            println!("stemd: Received SHUTDOWN command");

                            let response = "OK: Shutting down\n";
                            let _ = stream.write(response.as_bytes());

                            // Shutdown must be called from main thread (PID 1)
                            // to pass kernel authorization check
                            handle_shutdown();
                        } else {
                            // Try to parse command as UTF-8 text
                            match core::str::from_utf8(&buffer[..n]) {
                                Ok(cmd) => {
                                    println!("stemd: Received command: {}", cmd.trim());

                                    // Process command (simplified)
                                    let response = match cmd.trim() {
                                        "status" => "stemd is running\n",
                                        "help" => {
                                            "Commands: status, help, launch_or_focus, shutdown\n"
                                        }
                                        "shutdown" => {
                                            // Handle text-based shutdown command too
                                            let _ = thread::spawn(handle_shutdown);
                                            "OK: Shutting down\n"
                                        }
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
            println!("stemd: Reading configuration from directory: {}", dir_path);

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

/// sbus handler thread: receive and process method calls from sbus
fn sbus_handler_thread() {
    println!("stemd: sbus handler thread started");

    loop {
        // Get the sbus connection
        let mut conn_guard = SBUS_CONNECTION.lock();
        let conn_result = match conn_guard.as_mut() {
            Some(conn) => conn.receive_message(),
            None => {
                drop(conn_guard);
                println!("stemd: sbus connection not available, waiting...");
                std::thread::sleep(core::time::Duration::from_millis(100));
                continue;
            }
        };

        match conn_result {
            Ok(msg) => {
                // Handle the message
                if let Err(e) = handle_sbus_message(conn_guard, msg) {
                    println!("stemd: Error handling sbus message: {:?}", e);
                }
            }
            Err(e) => {
                println!("stemd: Error receiving message from sbus: {:?}", e);
                std::thread::sleep(core::time::Duration::from_millis(100));
            }
        }
    }
}

/// Handle an incoming sbus message
fn handle_sbus_message(
    mut conn_guard: std::sync::MutexGuard<'_, Option<sbus::Connection>>,
    msg: sbus::Message,
) -> Result<(), &'static str> {
    use sbus::Argument;
    use sbus::Message;

    match msg {
        Message::CallMethod {
            destination,
            path,
            interface,
            method,
            args,
        } => {
            println!(
                "[sbus] CallMethod: dest={} path={} interface={} method={}",
                destination, path, interface, method
            );

            // Get the serial from the message
            // For now, we don't have access to the serial number in the parsed message
            // We'll use 0 as a placeholder
            let serial = 0u32;

            // Handle the method call
            match method.as_str() {
                "LaunchOrFocus" => {
                    println!("[sbus] Handling LaunchOrFocus method");

                    // Extract app_id from arguments
                    if args.is_empty() {
                        println!("[sbus] LaunchOrFocus: missing app_id argument");
                        if let Some(conn) = conn_guard.as_mut() {
                            let result: core::result::Result<(), sbus::Error> = conn
                                .send_method_error(
                                    serial,
                                    "org.scarlet-os.stemd.InvalidArgs",
                                    "Missing app_id argument",
                                );
                            let _ = result;
                        }
                        return Ok(());
                    }

                    let app_id = match &args[0] {
                        Argument::String(s) => s,
                        _ => {
                            println!("[sbus] LaunchOrFocus: invalid argument type");
                            if let Some(conn) = conn_guard.as_mut() {
                                let result: core::result::Result<(), sbus::Error> = conn
                                    .send_method_error(
                                        serial,
                                        "org.scarlet-os.stemd.InvalidArgs",
                                        "Invalid argument type",
                                    );
                                let _ = result;
                            }
                            return Ok(());
                        }
                    };

                    println!("[sbus] LaunchOrFocus: app_id={}", app_id);

                    // Check if the app is already running
                    if let Some(running_app) = find_running_app(app_id) {
                        println!(
                            "[sbus] App {} is already running (PID={}), focusing",
                            app_id, running_app.pid
                        );
                        // Focus the existing window
                        match focus_window_by_app_id(app_id) {
                            Ok(_) => {
                                println!("[sbus] Successfully focused window for {}", app_id);
                                if let Some(conn) = conn_guard.as_mut() {
                                    let result: core::result::Result<(), sbus::Error> = conn
                                        .send_method_return(
                                            serial,
                                            vec![Argument::String("Focused".to_string())],
                                        );
                                    let _ = result;
                                }
                            }
                            Err(e) => {
                                println!("[sbus] Failed to focus window: {}", e);
                                // Check if the process is still alive
                                let pid = running_app.pid;
                                let wait_result = waitpid(pid, 1); // WNOHANG = non-blocking

                                if wait_result.0 > 0 {
                                    // Process has exited (waitpid reaped the zombie)
                                    println!(
                                        "[sbus] Process {} has exited (status={}), removing stale entry",
                                        pid, wait_result.1
                                    );
                                    remove_running_app_by_pid(pid);

                                    println!("[sbus] Launching new instance of {}", app_id);
                                    match launch_app_by_id(app_id) {
                                        Ok(new_pid) => {
                                            println!(
                                                "[sbus] Successfully launched {} (PID={})",
                                                app_id, new_pid
                                            );
                                            if let Some(conn) = conn_guard.as_mut() {
                                                let result: core::result::Result<(), sbus::Error> =
                                                    conn.send_method_return(
                                                        serial,
                                                        vec![Argument::String(
                                                            "Launched".to_string(),
                                                        )],
                                                    );
                                                let _ = result;
                                            }
                                        }
                                        Err(launch_err) => {
                                            println!("[sbus] Failed to launch app: {}", launch_err);
                                            if let Some(conn) = conn_guard.as_mut() {
                                                let result: core::result::Result<(), sbus::Error> =
                                                    conn.send_method_error(
                                                        serial,
                                                        "org.scarlet-os.stemd.LaunchFailed",
                                                        launch_err,
                                                    );
                                                let _ = result;
                                            }
                                        }
                                    }
                                } else if wait_result.0 < 0 {
                                    // Process doesn't exist (waitpid error)
                                    println!(
                                        "[sbus] Process {} doesn't exist, removing stale entry",
                                        pid
                                    );
                                    remove_running_app_by_pid(pid);

                                    println!("[sbus] Launching new instance of {}", app_id);
                                    match launch_app_by_id(app_id) {
                                        Ok(new_pid) => {
                                            println!(
                                                "[sbus] Successfully launched {} (PID={})",
                                                app_id, new_pid
                                            );
                                            if let Some(conn) = conn_guard.as_mut() {
                                                let result: core::result::Result<(), sbus::Error> =
                                                    conn.send_method_return(
                                                        serial,
                                                        vec![Argument::String(
                                                            "Launched".to_string(),
                                                        )],
                                                    );
                                                let _ = result;
                                            }
                                        }
                                        Err(launch_err) => {
                                            println!("[sbus] Failed to launch app: {}", launch_err);
                                            if let Some(conn) = conn_guard.as_mut() {
                                                let result: core::result::Result<(), sbus::Error> =
                                                    conn.send_method_error(
                                                        serial,
                                                        "org.scarlet-os.stemd.LaunchFailed",
                                                        launch_err,
                                                    );
                                                let _ = result;
                                            }
                                        }
                                    }
                                } else {
                                    // wait_result.0 == 0: Process is still alive (WNOHANG returned 0)
                                    println!(
                                        "[sbus] Process {} is still alive, window not ready yet",
                                        pid
                                    );
                                    if let Some(conn) = conn_guard.as_mut() {
                                        let result: core::result::Result<(), sbus::Error> = conn
                                            .send_method_error(
                                                serial,
                                                "org.scarlet-os.stemd.WindowNotReady",
                                                "Application is running but window not ready yet",
                                            );
                                        let _ = result;
                                    }
                                }
                            }
                        }
                    } else {
                        println!("[sbus] App {} is not running, launching", app_id);
                        // Launch the application
                        match launch_app_by_id(app_id) {
                            Ok(pid) => {
                                println!("[sbus] Successfully launched {} (PID={})", app_id, pid);
                                if let Some(conn) = conn_guard.as_mut() {
                                    let result: core::result::Result<(), sbus::Error> = conn
                                        .send_method_return(
                                            serial,
                                            vec![Argument::String("Launched".to_string())],
                                        );
                                    let _ = result;
                                }
                            }
                            Err(e) => {
                                println!("[sbus] Failed to launch app: {}", e);
                                if let Some(conn) = conn_guard.as_mut() {
                                    let result: core::result::Result<(), sbus::Error> = conn
                                        .send_method_error(
                                            serial,
                                            "org.scarlet-os.stemd.LaunchFailed",
                                            e,
                                        );
                                    let _ = result;
                                }
                            }
                        }
                    }

                    Ok(())
                }
                "GetRunningApps" => {
                    println!("[sbus] Handling GetRunningApps method");

                    let apps = get_running_apps_list();
                    let mut result_args = Vec::new();

                    // Return array of strings: "app_id|name"
                    for app in &apps {
                        let entry = lookup_app(&app.app_id);
                        let name = entry
                            .as_ref()
                            .map(|e| e.name.as_str())
                            .unwrap_or(&app.app_id);
                        let display_name = format!("{}|{}", app.app_id, name);
                        result_args.push(Argument::String(display_name));
                    }

                    if let Some(conn) = conn_guard.as_mut() {
                        let result: core::result::Result<(), sbus::Error> =
                            conn.send_method_return(serial, result_args);
                        let _ = result;
                    }
                    Ok(())
                }
                "GetActiveApp" => {
                    // Query SWS for the focused window
                    match get_focused_window_app_id() {
                        Some(app_id) => {
                            // Look up the app name
                            let entry = lookup_app(&app_id);
                            let name = entry.as_ref().map(|e| e.name.as_str()).unwrap_or(&app_id);
                            let display_name = format!("{}|{}", app_id, name);

                            if let Some(conn) = conn_guard.as_mut() {
                                let result: core::result::Result<(), sbus::Error> = conn
                                    .send_method_return(
                                        serial,
                                        vec![Argument::String(display_name)],
                                    );
                                let _ = result;
                            }
                        }
                        None => {
                            // No focused window
                            if let Some(conn) = conn_guard.as_mut() {
                                let result: core::result::Result<(), sbus::Error> = conn
                                    .send_method_return(
                                        serial,
                                        vec![Argument::String(String::new())],
                                    );
                                let _ = result;
                            }
                        }
                    }
                    Ok(())
                }
                "GetAppMenus" => {
                    // DEPRECATED: This method is obsolete. Menu information is now managed by SWS.
                    // Applications register menus via create_surface() in SWS.
                    // TaskBar receives menu info through FOCUS_CHANGED events.
                    // This handler is kept for backward compatibility only.
                    println!(
                        "[sbus] Handling GetAppMenus method for app: {}",
                        args.first().map_or("", |a| match a {
                            sbus::Argument::String(s) => s.as_str(),
                            _ => "",
                        })
                    );

                    // Extract app_id from arguments
                    let app_id = if !args.is_empty() {
                        match &args[0] {
                            Argument::String(s) => Some(s.clone()),
                            _ => None,
                        }
                    } else {
                        None
                    };

                    let menu_titles = if let Some(app_id) = app_id {
                        get_app_menu_titles(&app_id)
                    } else {
                        Vec::new()
                    };

                    // Return pipe-separated menu titles: "menu1|menu2|menu3"
                    let menus_string = menu_titles.join("|");

                    if let Some(conn) = conn_guard.as_mut() {
                        let result: core::result::Result<(), sbus::Error> =
                            conn.send_method_return(serial, vec![Argument::String(menus_string)]);
                        let _ = result;
                    }
                    Ok(())
                }
                _ => {
                    println!("[sbus] Unknown method: {}", method);
                    if let Some(conn) = conn_guard.as_mut() {
                        let mut error_msg = String::new();
                        error_msg.push_str("Unknown method: ");
                        error_msg.push_str(&method);
                        let _ = conn.send_method_error(
                            serial,
                            "org.scarlet-os.stemd.UnknownMethod",
                            &error_msg,
                        );
                    }
                    Ok(())
                }
            }
        }
        _ => {
            println!(
                "[sbus] Received unhandled message type: {:?}",
                msg.msg_type()
            );
            Ok(())
        }
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("stemd: Stem Daemon starting...");
    println!("stemd: PID={}", std::task::getpid());

    // Read configuration.
    // Note: current filesystem layout copies userland under `/system/scarlet`,
    // so configs may live under `/system/scarlet/etc` instead of `/etc`.
    // Directory structure:
    //   /etc/stemd.d/services/*.toml  - Service definitions
    //   /etc/stemd.d/apps/*.desktop   - Application definitions
    let config_dirs = [
        "/etc/stemd.d/services",
        "/system/scarlet/etc/stemd.d/services",
    ];
    let config_files = [
        "/etc/stemd.d/services.toml",
        "/system/scarlet/etc/stemd.d/services.toml",
    ];

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

    // Check if stemd itself is defined as a service
    let stemd_service = services.iter().find(|s| s.name == "stemd");

    // Phase 1: Initialize stemd's dependencies (e.g., sbusd)
    if let Some(service) = stemd_service {
        println!("stemd: Initializing dependencies for stemd...");
        for dep_name in &service.depends {
            println!("stemd: Launching dependency: {}", dep_name);

            // Find the dependency service and launch it
            if let Some(dep_service) = services.iter().find(|s| s.name == *dep_name) {
                if let Err(e) = launch_service(dep_service) {
                    println!("stemd: Failed to launch dependency {}: {}", dep_name, e);
                } else {
                    println!("stemd: Successfully launched dependency: {}", dep_name);
                }
                fence(core::sync::atomic::Ordering::SeqCst);
            }
        }
    }

    // Phase 2: Register with sbus (now that sbusd should be running)
    println!("stemd: Registering with sbus...");
    let mut registered = false;
    for attempt in 0..20 {
        // Give CPU time to sbusd to start up
        for _ in 0..10 {
            std::thread::yield_now();
        }

        match sbus::Connection::connect() {
            Ok(mut conn) => {
                match conn.register_service("org.scarlet-os.stemd") {
                    Ok(_) => {
                        println!(
                            "stemd: Successfully registered with sbus as org.scarlet-os.stemd"
                        );

                        // Store the connection globally for method handling
                        {
                            let mut sbus_conn = SBUS_CONNECTION.lock();
                            *sbus_conn = Some(conn);
                        }

                        registered = true;
                        break;
                    }
                    Err(e) => {
                        println!(
                            "stemd: Attempt {}: Failed to register with sbus: {:?}",
                            attempt + 1,
                            e
                        );
                    }
                }
            }
            Err(e) => {
                println!(
                    "stemd: Attempt {}: Failed to connect to sbus: {:?}",
                    attempt + 1,
                    e
                );
            }
        }
    }

    if !registered {
        println!("stemd: Could not register with sbus after 10 attempts");
        println!("stemd: Continuing without sbus registration");
    }

    // Phase 3: Launch other services (excluding stemd itself)
    println!("stemd: Launching services...");

    for service in &launch_order {
        // Skip stemd itself (we're already running!)
        if service.name == "stemd" {
            println!("stemd: Skipping stemd (already running as PID 1)");
            continue;
        }

        // Skip services that were already launched as dependencies
        if let Some(stemd_svc) = &stemd_service
            && stemd_svc.depends.contains(&service.name)
        {
            println!(
                "stemd: Skipping {} (already launched as dependency)",
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

    // Load .desktop files for application definitions
    println!("stemd: Loading application definitions...");
    // Unified directory structure: /etc/stemd.d/apps/*.desktop
    let desktop_dirs = ["/etc/stemd.d/apps", "/system/scarlet/etc/stemd.d/apps"];
    let mut total_apps = 0;
    for dir in &desktop_dirs {
        match load_desktop_files(dir) {
            Ok(count) => {
                if count > 0 {
                    println!("stemd: Loaded {} applications from {}", count, dir);
                    total_apps += count;
                }
            }
            Err(_) => {
                // Directory doesn't exist or couldn't be read, continue
            }
        }
    }
    if total_apps > 0 {
        println!("stemd: Total {} applications loaded", total_apps);
    } else {
        println!("stemd: No application definitions found");
    }

    // Spawn IPC thread
    println!("stemd: Starting IPC thread");
    let _ipc_handle = thread::spawn(ipc_thread);

    // Spawn sbus handler thread if we registered with sbus
    if registered {
        println!("stemd: Starting sbus handler thread");
        let _sbus_handle = thread::spawn(sbus_handler_thread);
    }

    loop {
        let (pid, status) = waitpid(-1, 0);
        if pid < 0 {
            continue;
        }
        println!("stemd: Reaped child PID={} status={}", pid, status);
    }
}
