//! Stem Daemon (stemd) - A systemd-like service manager for Scarlet
//!
//! This daemon:
//! - Reads a TOML configuration file to determine services and their dependencies
//! - Launches services in dependency order
//! - Reaps child processes using waitpid(-1, 0)
//! - Provides an IPC interface via LocalSocket for control commands
//! - Supports .desktop files for application definitions

use core::sync::atomic::{AtomicBool, Ordering, fence};
use sbus_client as sbus;
use scarlet_desktop_config::DESKTOP_STEMD_LIST_APPLICATIONS_METHOD;
use scarlet_os::handle::capability::StreamOps;
use scarlet_os::process::{ShutdownType, WAIT_NOHANG, shutdown, waitpid};
use scarlet_os::socket::Socket;
use std::{
    format,
    fs::{self, File, OpenOptions},
    io::Read,
    println,
    process::{Command, Stdio},
    string::{String, ToString},
    sync::Mutex,
    thread, vec,
    vec::Vec,
};
use sws_client::Connection;

// Import protocol and desktop modules
mod desktop;
mod protocol;

use desktop::{
    DesktopEntry, expand_exec, list_apps, load_desktop_files, lookup_app, lookup_app_for_mime,
    mime_type_for_path,
};
use protocol::cmd;

const ACTIVATION_TOKEN_ENV: &str = "SWS_ACTIVATION_TOKEN";
const SWS_QUERY_TIMEOUT_MS: u64 = 1_000;
const SBUS_REGISTRATION_ATTEMPTS: usize = 20;
const SBUS_REGISTRATION_TIMEOUT_MS: u64 = 1_000;
const SBUS_RECONNECT_DELAY_MS: u64 = 1_000;

fn application_environment(activation_token: Option<&str>) -> Vec<String> {
    let user = std::env::var("USER").unwrap_or(String::from("root"));
    let home = std::env::var("HOME").unwrap_or(String::from("/root"));
    let shell = std::env::var("SHELL").unwrap_or(String::from("/bin/sh"));
    let mut environment = vec![
        format!("USER={user}"),
        format!("HOME={home}"),
        format!("SHELL={shell}"),
    ];
    if let Some(token) = activation_token {
        environment.push(format!("{ACTIVATION_TOKEN_ENV}={token}"));
    }
    environment
}

fn configure_command_stdio(command: &mut Command, path: &str) -> Result<(), &'static str> {
    let stdin = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| "Failed to open service stdio device")?;
    let stdout = stdin
        .try_clone()
        .map_err(|_| "Failed to clone service stdout")?;
    let stderr = stdin
        .try_clone()
        .map_err(|_| "Failed to clone service stderr")?;

    command.stdin(Stdio::from(stdin));
    command.stdout(Stdio::from(stdout));
    command.stderr(Stdio::from(stderr));
    Ok(())
}

fn spawn_command(
    argv: &[String],
    activation_token: Option<&str>,
    stdio_path: Option<&str>,
) -> Result<i32, &'static str> {
    let program = argv.first().ok_or("Invalid empty command")?;
    let mut command = Command::new(program);
    command.args(&argv[1..]);
    command.env_clear();
    for assignment in application_environment(activation_token) {
        let (key, value) = assignment
            .split_once('=')
            .ok_or("Invalid application environment")?;
        command.env(key, value);
    }
    configure_command_stdio(&mut command, stdio_path.unwrap_or("/dev/null"))?;

    command
        .spawn()
        .map(|child| child.id() as i32)
        .map_err(|_| "Failed to spawn process")
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
    ready_notify: bool,
    ready_timeout_ms: u32,
}

/// Running application tracking
#[derive(Debug, Clone)]
struct RunningApp {
    app_id: String,
    pid: i32,
    exec_path: String,
}

/// Running service tracking
#[derive(Debug, Clone)]
struct RunningService {
    name: String,
    pid: i32,
    exec_path: String,
}

// Global tracking for running applications
// Thread-safe using Mutex (static, not mutable)
static RUNNING_APPS: Mutex<Vec<RunningApp>> = Mutex::new(Vec::new());

// Raw IPC and sbus can request activation concurrently. Keep the
// check/focus/reap/launch sequence atomic so both paths cannot launch the
// same single-instance application after observing the same stale state.
static APP_ACTIVATION_LOCK: Mutex<()> = Mutex::new(());

// Global tracking for running services
static RUNNING_SERVICES: Mutex<Vec<RunningService>> = Mutex::new(Vec::new());
static READY_SERVICES: Mutex<Vec<String>> = Mutex::new(Vec::new());

const STEMD_IPC_SOCKET_PATH: &str = "/tmp/stemd.sock";
static IPC_ACCEPT_LOOP_STARTED: AtomicBool = AtomicBool::new(false);

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
                let mut ready_notify = false;
                let mut ready_timeout_ms = 5000u32;

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
                            "ready_notify" => {
                                if let Some(v) = Self::parse_bool(value) {
                                    ready_notify = v;
                                }
                            }
                            "ready_timeout_ms" => {
                                if let Some(v) = Self::parse_u32(value) {
                                    ready_timeout_ms = v;
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
                        ready_notify,
                        ready_timeout_ms,
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

    fn parse_u32(s: &str) -> Option<u32> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }

        let mut acc: u32 = 0;
        for ch in s.bytes() {
            if !ch.is_ascii_digit() {
                return None;
            }
            let digit = (ch - b'0') as u32;
            acc = acc.saturating_mul(10).saturating_add(digit);
        }

        Some(acc)
    }

    fn parse_bool(s: &str) -> Option<bool> {
        match Self::unquote(s).as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => None,
        }
    }
}

/// Add a running app to the tracking list
fn add_running_app(app_id: String, pid: i32, exec_path: String) {
    let mut apps = RUNNING_APPS.lock().expect("stemd mutex poisoned");
    apps.push(RunningApp {
        app_id,
        pid,
        exec_path,
    });
}

/// Remove a running app from the tracking list by PID
fn remove_running_app_by_pid(pid: i32) -> bool {
    let mut apps = RUNNING_APPS.lock().expect("stemd mutex poisoned");
    if let Some(pos) = apps.iter().position(|app| app.pid == pid) {
        apps.remove(pos);
        return true;
    }
    false
}

/// Find a running app by app_id
fn find_running_app(app_id: &str) -> Option<RunningApp> {
    let apps = RUNNING_APPS.lock().expect("stemd mutex poisoned");
    apps.iter().find(|app| app.app_id == app_id).cloned()
}

fn add_running_service(name: String, pid: i32, exec_path: String) {
    let mut services = RUNNING_SERVICES.lock().expect("stemd mutex poisoned");
    services.push(RunningService {
        name,
        pid,
        exec_path,
    });
}

fn remove_running_service_by_pid(pid: i32) -> Option<RunningService> {
    let mut services = RUNNING_SERVICES.lock().expect("stemd mutex poisoned");
    services
        .iter()
        .position(|service| service.pid == pid)
        .map(|pos| services.remove(pos))
}

fn mark_service_ready(service_name: &str) {
    let mut ready = READY_SERVICES.lock().expect("stemd mutex poisoned");
    if !ready.iter().any(|name| name == service_name) {
        ready.push(service_name.to_string());
    }
}

fn clear_service_ready(service_name: &str) {
    let mut ready = READY_SERVICES.lock().expect("stemd mutex poisoned");
    ready.retain(|name| name != service_name);
}

fn is_service_ready(service_name: &str) -> bool {
    let ready = READY_SERVICES.lock().expect("stemd mutex poisoned");
    ready.iter().any(|name| name == service_name)
}

fn is_running_service_pid(pid: i32) -> bool {
    let services = RUNNING_SERVICES.lock().expect("stemd mutex poisoned");
    services.iter().any(|service| service.pid == pid)
}

fn wait_for_service_ready(service: &Service, pid: i32) -> Result<(), &'static str> {
    if !service.ready_notify {
        return Ok(());
    }

    let mut waited_ms = 0u32;
    let sleep_ms = 50u32;
    let mut observed_exit = false;

    while waited_ms < service.ready_timeout_ms {
        reap_children_nonblocking("wait-ready");

        if is_service_ready(&service.name) {
            println!(
                "stemd: Service '{}' reported ready after {} ms",
                service.name, waited_ms
            );
            return Ok(());
        }

        if !is_running_service_pid(pid) {
            // A one-shot service may send SERVICE_READY and exit before its
            // IPC handler runs. Keep accepting that already-sent notification
            // until the configured readiness deadline instead of racing the
            // child reaper.
            observed_exit = true;
        }

        thread::sleep(core::time::Duration::from_millis(sleep_ms as u64));
        waited_ms = waited_ms.saturating_add(sleep_ms);
    }

    // Close the boundary race with a notification processed during the final
    // sleep interval before deciding whether this was an exit or a timeout.
    reap_children_nonblocking("wait-ready-final");
    if is_service_ready(&service.name) {
        println!(
            "stemd: Service '{}' reported ready after {} ms",
            service.name, waited_ms
        );
        return Ok(());
    }
    observed_exit |= !is_running_service_pid(pid);

    if observed_exit {
        println!(
            "stemd: Service '{}' exited without a readiness notification before the {} ms deadline",
            service.name, service.ready_timeout_ms
        );
        return Err("Service exited before ready");
    }

    println!(
        "stemd: Timed out waiting for service '{}' readiness notification",
        service.name
    );
    Err("Timed out waiting for service readiness")
}

fn launch_service_and_wait(service: &Service) -> Result<(), &'static str> {
    if service.ready_notify {
        clear_service_ready(&service.name);
    }
    let pid = launch_service(service)?;
    fence(core::sync::atomic::Ordering::SeqCst);
    wait_for_service_ready(service, pid)
}

fn has_failed_dependency(service: &Service, failed_services: &[String]) -> Option<String> {
    service
        .depends
        .iter()
        .find(|dep| failed_services.iter().any(|failed| failed == *dep))
        .cloned()
}

fn reap_children_nonblocking(context: &str) {
    loop {
        let (pid, status) = waitpid(-1, WAIT_NOHANG);
        if pid <= 0 {
            break;
        }
        if let Some(service) = remove_running_service_by_pid(pid) {
            println!(
                "stemd: [{}] Reaped service PID={} status={} name={} exec={}",
                context, pid, status, service.name, service.exec_path
            );
            continue;
        }
        if remove_running_app_by_pid(pid) {
            println!(
                "stemd: [{}] Reaped app PID={} status={}",
                context, pid, status
            );
            continue;
        }
        println!(
            "stemd: [{}] Reaped unknown child PID={} status={}",
            context, pid, status
        );
    }
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
    let windows = match conn.get_window_list_timeout(SWS_QUERY_TIMEOUT_MS) {
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
    let _activation_guard = APP_ACTIVATION_LOCK.lock().expect("stemd mutex poisoned");
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
            let (wait_pid, wait_status) = waitpid(app.pid, WAIT_NOHANG);
            if wait_pid > 0 {
                println!(
                    "stemd: Reaped stale app PID={} status={} app_id={}",
                    wait_pid, wait_status, app_id
                );
                remove_running_app_by_pid(wait_pid);
            } else if wait_pid < 0 {
                println!(
                    "stemd: Removing stale app entry PID={} app_id={}",
                    app.pid, app_id
                );
                remove_running_app_by_pid(app.pid);
            } else {
                return Err(e);
            }
        } else {
            println!("stemd: Successfully focused window");
            return Ok(());
        }
    }

    if focus_window_by_app_id(app_id).is_ok() {
        println!("stemd: Found existing window for '{}'", app_id);
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

    let argv: Vec<String> = exec_path.split_whitespace().map(String::from).collect();
    let pid = spawn_command(&argv, None, None).map_err(|error| {
        println!("stemd: Failed to launch app '{}': {}", app_id, error);
        error
    })?;

    add_running_app(app_id.to_string(), pid, exec_path.to_string());
    println!("stemd: App '{}' launched with PID: {}", app_id, pid);
    Ok(())
}

/// Get a list of all running applications
fn get_running_apps_list() -> Vec<RunningApp> {
    let apps = RUNNING_APPS.lock().expect("stemd mutex poisoned");
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
    let windows = match conn.get_window_list_timeout(SWS_QUERY_TIMEOUT_MS) {
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
fn launch_app_by_id(app_id: &str, activation_token: Option<&str>) -> Result<i32, &'static str> {
    println!("stemd: launch_app_by_id called with app_id={}", app_id);

    println!("stemd: Looking up {} from .desktop files...", app_id);
    let entry = match lookup_app(app_id) {
        Some(entry) => {
            println!("stemd: Found app: {} -> {}", entry.name, entry.exec);
            entry
        }
        None => {
            println!("stemd: App '{}' not found in registry", app_id);
            return Err("App not found in registry");
        }
    };

    launch_desktop_entry(&entry, &[], activation_token)
}

/// Launch a registered desktop entry with optional local file arguments.
fn launch_desktop_entry(
    entry: &DesktopEntry,
    files: &[String],
    activation_token: Option<&str>,
) -> Result<i32, &'static str> {
    let argv_strings = expand_exec(&entry.exec, files)?;
    let app_id = entry.app_id.clone();
    let exec_path = entry.exec.clone();

    let pid = spawn_command(&argv_strings, activation_token, None).map_err(|error| {
        println!("stemd: Failed to launch app '{}': {}", app_id, error);
        error
    })?;

    add_running_app(app_id.clone(), pid, exec_path);
    println!("stemd: App '{}' launched with PID: {}", app_id, pid);
    Ok(pid)
}

/// Resolve and open a local path using its registered default application.
fn open_path(path: &str) -> Result<i32, &'static str> {
    let mime_type = mime_type_for_path(path).ok_or("Unsupported file type")?;
    let entry = lookup_app_for_mime(mime_type).ok_or("No application registered for file type")?;
    println!(
        "stemd: Opening '{}' as {} with {}",
        path, mime_type, entry.app_id
    );
    let files = vec![String::from(path)];
    launch_desktop_entry(&entry, &files, None)
}

/// Launch a service as a child process.
fn launch_service(service: &Service) -> Result<i32, &'static str> {
    println!("stemd: Launching service: {}", service.name);

    let argv: Vec<String> = service.exec.split_whitespace().map(String::from).collect();
    let pid = spawn_command(&argv, None, service.tty.as_deref()).map_err(|error| {
        println!(
            "stemd: Failed to launch service '{}': {}",
            service.name, error
        );
        error
    })?;

    add_running_service(service.name.clone(), pid, service.exec.clone());
    println!(
        "stemd: Service '{}' launched with PID={} exec={}",
        service.name, pid, service.exec
    );
    Ok(pid)
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

fn read_until(
    stream: &StreamOps<'_>,
    buffer: &mut [u8],
    filled: &mut usize,
    target: usize,
) -> bool {
    if target > buffer.len() {
        return false;
    }

    while *filled < target {
        match stream.read(&mut buffer[*filled..target]) {
            Ok(0) | Err(_) => return false,
            Ok(bytes) => *filled += bytes,
        }
    }

    true
}

/// Create the stemd IPC listener before any readiness-reporting service starts.
fn prepare_ipc_server() -> Option<Socket> {
    let server = match Socket::new() {
        Ok(s) => s,
        Err(e) => {
            println!("stemd: Failed to create IPC socket: {:?}", e);
            return None;
        }
    };

    if let Err(e) = server.bind(STEMD_IPC_SOCKET_PATH) {
        println!("stemd: Failed to bind IPC socket: {:?}", e);
        return None;
    }

    if let Err(e) = server.listen(5) {
        println!("stemd: Failed to listen on IPC socket: {:?}", e);
        return None;
    }

    println!("stemd: IPC socket listening at {}", STEMD_IPC_SOCKET_PATH);
    Some(server)
}

/// IPC thread: accept commands from an already-listening socket.
fn ipc_thread(server: Socket) {
    IPC_ACCEPT_LOOP_STARTED.store(true, Ordering::Release);
    println!("stemd: IPC thread started");

    // Accept connections. Each connection is handled on its own thread so one
    // slow command (registry lookup, SWS round-trip, fork) cannot block every
    // other stemd client.
    loop {
        match server.accept() {
            Ok(client) => {
                let _ = thread::spawn(move || handle_ipc_client(client));
            }
            Err(_) => {
                thread::sleep(core::time::Duration::from_millis(100));
            }
        }
    }
}

/// Handle a single stemd IPC connection (one command per connection).
fn handle_ipc_client(client: Socket) {
    let stream = match client.as_stream() {
        Ok(s) => s,
        Err(_) => return,
    };

    // Read command (larger buffer for binary commands)
    let mut buffer = [0u8; 1024];
    match stream.read(&mut buffer) {
        Ok(mut n) if n > 0 => {
            // Check if this is a binary launch command.
            if buffer[0] == cmd::LAUNCH_OR_FOCUS || buffer[0] == cmd::LAUNCH {
                // Parse launch command.
                // Format: cmd(1) + app_id_len(4) + app_id + exec_path_len(4) + exec_path
                if read_until(&stream, &mut buffer, &mut n, 9) {
                    let app_id_len =
                        u32::from_le_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;

                    let exec_path_offset = 5 + app_id_len;
                    if read_until(&stream, &mut buffer, &mut n, exec_path_offset + 4) {
                        let exec_path_len = u32::from_le_bytes([
                            buffer[exec_path_offset],
                            buffer[exec_path_offset + 1],
                            buffer[exec_path_offset + 2],
                            buffer[exec_path_offset + 3],
                        ]) as usize;

                        let total_len = exec_path_offset + 4 + exec_path_len;
                        if read_until(&stream, &mut buffer, &mut n, total_len) {
                            let app_id = core::str::from_utf8(&buffer[5..5 + app_id_len]);
                            let exec_path = core::str::from_utf8(
                                &buffer[exec_path_offset + 4..exec_path_offset + 4 + exec_path_len],
                            );

                            match (app_id, exec_path) {
                                (Ok(app_id), Ok(exec_path)) => {
                                    let launch_only = buffer[0] == cmd::LAUNCH;
                                    println!(
                                        "stemd: {} app_id={} exec={}",
                                        if launch_only {
                                            "LAUNCH"
                                        } else {
                                            "LAUNCH_OR_FOCUS"
                                        },
                                        app_id,
                                        exec_path
                                    );

                                    let exec_path_arg = if exec_path.is_empty() {
                                        None
                                    } else {
                                        Some(exec_path)
                                    };

                                    let response = if launch_only {
                                        match launch_app_by_id(app_id, None) {
                                            Ok(_) => "OK: Launched\n".as_bytes(),
                                            Err(_) => "ERROR: Failed to launch\n".as_bytes(),
                                        }
                                    } else {
                                        match launch_or_focus(app_id, exec_path_arg) {
                                            Ok(()) => "OK: Launched or focused\n".as_bytes(),
                                            Err(e) => {
                                                // Build error message as byte array directly
                                                let error_prefix = "ERROR: ";
                                                let error_suffix = "\n";
                                                let mut error_msg = Vec::new();
                                                error_msg
                                                    .extend_from_slice(error_prefix.as_bytes());
                                                error_msg.extend_from_slice(e.as_bytes());
                                                error_msg
                                                    .extend_from_slice(error_suffix.as_bytes());
                                                // Note: This is a temporary solution - the Vec will be dropped
                                                // but we're returning a slice. For IPC, we should handle this differently.
                                                // For now, use a static error message.
                                                "ERROR: Failed to launch or focus\n".as_bytes()
                                            }
                                        }
                                    };

                                    let _ = stream.write(response);
                                }
                                _ => {
                                    let error_msg = "ERROR: Invalid UTF-8 in parameters\n";
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
            } else if buffer[0] == cmd::SERVICE_READY {
                if read_until(&stream, &mut buffer, &mut n, 5) {
                    let service_name_len =
                        u32::from_le_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
                    let end = 5 + service_name_len;
                    if read_until(&stream, &mut buffer, &mut n, end) {
                        match core::str::from_utf8(&buffer[5..end]) {
                            Ok(service_name) => {
                                mark_service_ready(service_name);
                                // Publish the readiness latch before logging or
                                // acknowledging it. A one-shot service is free
                                // to exit as soon as it receives the reply.
                                println!("stemd: SERVICE_READY received for '{}'", service_name);
                                let _ = stream.write("OK: Service marked ready\n".as_bytes());
                            }
                            Err(_) => {
                                let _ = stream
                                    .write("ERROR: Invalid UTF-8 in service name\n".as_bytes());
                            }
                        }
                    } else {
                        let _ =
                            stream.write("ERROR: Incomplete SERVICE_READY command\n".as_bytes());
                    }
                } else {
                    let _ = stream.write("ERROR: Malformed SERVICE_READY command\n".as_bytes());
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
                            "help" => "Commands: status, help, launch_or_focus, shutdown\n",
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
    let mut combined_content = String::new();

    let entries = fs::read_dir(dir_path).map_err(|_| "Failed to read configuration directory")?;
    println!("stemd: Reading configuration from directory: {}", dir_path);

    let mut toml_files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| "Failed to read configuration directory entry")?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "." || name == ".." {
            continue;
        }

        let file_type = entry
            .file_type()
            .map_err(|_| "Failed to query configuration directory entry")?;
        if file_type.is_file() && name.ends_with(".toml") {
            toml_files.push(name);
        }
    }

    toml_files.sort();
    if toml_files.is_empty() {
        println!("stemd: No .toml files found in {}", dir_path);
        return Err("No configuration files found in directory");
    }

    for filename in toml_files {
        let file_path = format!("{}/{}", dir_path, filename);
        println!("stemd:   Loading {}", file_path);

        match read_config(&file_path) {
            Ok(content) => {
                combined_content.push_str(&content);
                combined_content.push('\n');
            }
            Err(error) => {
                println!("stemd:   Warning: Failed to read {}: {}", file_path, error);
            }
        }
    }

    if combined_content.is_empty() {
        Err("All configuration files failed to load")
    } else {
        Ok(combined_content)
    }
}

/// sbus handler thread: receive and process method calls from sbus
fn reconnect_stemd_sbus() -> Result<sbus::Connection, sbus::Error> {
    let mut connection = sbus::Connection::connect()?;
    connection.register_service_timeout("org.scarlet-os.stemd", SBUS_REGISTRATION_TIMEOUT_MS)?;
    Ok(connection)
}

fn sbus_handler_thread() {
    println!("stemd: sbus handler thread started");

    loop {
        // Get the sbus connection
        let mut conn_guard = SBUS_CONNECTION.lock().expect("stemd mutex poisoned");
        let conn_result = match conn_guard.as_mut() {
            Some(conn) => conn.receive_message(),
            None => {
                drop(conn_guard);
                match reconnect_stemd_sbus() {
                    Ok(connection) => {
                        let mut connection_slot =
                            SBUS_CONNECTION.lock().expect("stemd mutex poisoned");
                        if connection_slot.is_none() {
                            *connection_slot = Some(connection);
                            println!("stemd: Reconnected and registered with sbus");
                        }
                    }
                    Err(error) => {
                        println!("stemd: Failed to reconnect to sbus: {:?}", error);
                        std::thread::sleep(core::time::Duration::from_millis(
                            SBUS_RECONNECT_DELAY_MS,
                        ));
                    }
                }
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
                // A failed stream remains at EOF/error permanently. Remove it
                // before retrying so the next iteration reconnects instead of
                // polling and logging the same dead connection forever.
                let _ = conn_guard.take();
                drop(conn_guard);
                println!("stemd: Lost sbus connection: {:?}; reconnecting", e);
                std::thread::sleep(core::time::Duration::from_millis(SBUS_RECONNECT_DELAY_MS));
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
        Message::CallMethod { method, args, .. } => {
            // Get the serial from the message
            // For now, we don't have access to the serial number in the parsed message
            // We'll use 0 as a placeholder
            let serial = 0u32;

            // Handle the method call
            match method.as_str() {
                "OpenPath" => {
                    let path = match args.first() {
                        Some(Argument::String(path)) if !path.is_empty() => path,
                        _ => {
                            if let Some(conn) = conn_guard.as_mut() {
                                let _ = conn.send_method_error(
                                    serial,
                                    "org.scarlet-os.stemd.InvalidArgs",
                                    "OpenPath requires a non-empty path argument",
                                );
                            }
                            return Ok(());
                        }
                    };

                    match open_path(path) {
                        Ok(_) => {
                            if let Some(conn) = conn_guard.as_mut() {
                                let _ = conn.send_method_return(
                                    serial,
                                    vec![Argument::String(String::from("Launched"))],
                                );
                            }
                        }
                        Err(error) => {
                            if let Some(conn) = conn_guard.as_mut() {
                                let _ = conn.send_method_error(
                                    serial,
                                    "org.scarlet-os.stemd.OpenFailed",
                                    error,
                                );
                            }
                        }
                    }

                    Ok(())
                }
                "LaunchOrFocus" => {
                    // Extract app_id from arguments
                    if args.is_empty() {
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
                    let activation_token = match args.get(1) {
                        None => None,
                        Some(Argument::String(token)) if !token.is_empty() => Some(token.as_str()),
                        _ => {
                            if let Some(conn) = conn_guard.as_mut() {
                                let _ = conn.send_method_error(
                                    serial,
                                    "org.scarlet-os.stemd.InvalidArgs",
                                    "activation token must be a non-empty string",
                                );
                            }
                            return Ok(());
                        }
                    };

                    let _activation_guard =
                        APP_ACTIVATION_LOCK.lock().expect("stemd mutex poisoned");

                    // Check if the app is already running
                    if let Some(running_app) = find_running_app(app_id) {
                        // Focus the existing window
                        match focus_window_by_app_id(app_id) {
                            Ok(_) => {
                                if let Some(conn) = conn_guard.as_mut() {
                                    let result: core::result::Result<(), sbus::Error> = conn
                                        .send_method_return(
                                            serial,
                                            vec![Argument::String("Focused".to_string())],
                                        );
                                    let _ = result;
                                }
                            }
                            Err(_) => {
                                // Check if the process is still alive
                                let pid = running_app.pid;
                                let wait_result = waitpid(pid, 1); // WNOHANG = non-blocking

                                if wait_result.0 > 0 {
                                    // Process has exited (waitpid reaped the zombie)
                                    remove_running_app_by_pid(pid);

                                    match launch_app_by_id(app_id, activation_token) {
                                        Ok(_) => {
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
                                    remove_running_app_by_pid(pid);

                                    match launch_app_by_id(app_id, activation_token) {
                                        Ok(_) => {
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
                        // Launch the application
                        match launch_app_by_id(app_id, activation_token) {
                            Ok(_) => {
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
                DESKTOP_STEMD_LIST_APPLICATIONS_METHOD => {
                    let mut result_args = Vec::new();
                    for app in list_apps() {
                        // The response is a flat sequence of triples so this
                        // remains usable with the current sbus argument model:
                        // app_id, display name, and desktop icon name.
                        result_args.push(Argument::String(app.app_id));
                        result_args.push(Argument::String(app.name));
                        result_args.push(Argument::String(app.icon.unwrap_or_default()));
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
        _ => Ok(()),
    }
}

fn main() {
    println!("stemd: Stem Daemon starting...");
    println!("stemd: PID={}", std::process::id());

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

    // Establish the listener synchronously. `thread::spawn()` only makes the
    // accept task runnable; it does not guarantee that the task has run before
    // a child attempts its SERVICE_READY connection.
    println!("stemd: Starting IPC service");
    let Some(ipc_server) = prepare_ipc_server() else {
        println!("stemd: Cannot launch services without the IPC listener");
        return;
    };
    IPC_ACCEPT_LOOP_STARTED.store(false, Ordering::Release);
    let _ipc_handle = thread::spawn(move || ipc_thread(ipc_server));
    while !IPC_ACCEPT_LOOP_STARTED.load(Ordering::Acquire) {
        thread::yield_now();
    }
    println!("stemd: IPC service ready");

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
                if let Err(e) = launch_service_and_wait(dep_service) {
                    println!("stemd: Failed to launch dependency {}: {}", dep_name, e);
                } else {
                    println!("stemd: Successfully launched dependency: {}", dep_name);
                }
            }
        }
    }

    // Phase 2: Register with sbus (now that sbusd should be running)
    let mut registered = false;
    for attempt in 0..SBUS_REGISTRATION_ATTEMPTS {
        // Give CPU time to sbusd to start up
        for _ in 0..10 {
            std::thread::yield_now();
        }
        if attempt == 0 {
            println!("stemd: Registering with sbus...");
        }

        match sbus::Connection::connect() {
            Ok(mut conn) => {
                match conn
                    .register_service_timeout("org.scarlet-os.stemd", SBUS_REGISTRATION_TIMEOUT_MS)
                {
                    Ok(_) => {
                        println!(
                            "stemd: Successfully registered with sbus as org.scarlet-os.stemd"
                        );

                        // Store the connection globally for method handling
                        {
                            let mut sbus_conn =
                                SBUS_CONNECTION.lock().expect("stemd mutex poisoned");
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
        println!(
            "stemd: Could not register with sbus after {} attempts",
            SBUS_REGISTRATION_ATTEMPTS
        );
        println!("stemd: Continuing without sbus registration");
    }

    // Phase 3: Launch other services (excluding stemd itself)
    println!("stemd: Launching services...");
    let mut failed_services: Vec<String> = Vec::new();

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

        if let Some(dep_name) = has_failed_dependency(service, &failed_services) {
            println!(
                "stemd: Skipping service '{}' because dependency '{}' failed or was not ready",
                service.name, dep_name
            );
            failed_services.push(service.name.clone());
            continue;
        }

        if let Err(e) = launch_service_and_wait(service) {
            println!("stemd: Failed to launch service {}: {}", service.name, e);
            failed_services.push(service.name.clone());
        }
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

    // Spawn sbus handler thread if we registered with sbus
    if registered {
        println!("stemd: Starting sbus handler thread");
        let _sbus_handle = thread::spawn(sbus_handler_thread);
    }

    loop {
        let (pid, status) = waitpid(-1, 0);
        if pid < 0 {
            // Do not turn a transient wait error (or a temporarily empty child
            // set) into a PID 1 full-core retry loop.
            thread::sleep(core::time::Duration::from_millis(10));
            continue;
        }
        if let Some(service) = remove_running_service_by_pid(pid) {
            println!(
                "stemd: Reaped service PID={} status={} name={} exec={}",
                pid, status, service.name, service.exec_path
            );
            continue;
        }
        if remove_running_app_by_pid(pid) {
            println!("stemd: Reaped app PID={} status={}", pid, status);
            continue;
        }
        println!("stemd: Reaped unknown child PID={} status={}", pid, status);
    }
}
