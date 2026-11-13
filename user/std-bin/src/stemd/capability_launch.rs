//! Administrator-owned application grants, loaded once before desktop clients.
use super::capability_config::{self, ApplicationGrant};
use scarlet_os::{
    Handle,
    environment::{Environment, HandleMapping, VfsView},
};
use std::{fs, io::Read, os::fd::IntoRawFd, sync::OnceLock};

const CONFIG_DIR: &str = "/etc/stemd.d/capabilities";
const MAX_CONFIG_BYTES: u64 = 64 * 1024;

pub struct Grant {
    rule: ApplicationGrant,
    executable: Handle,
    execution_argv: Vec<String>,
    views: Vec<VfsView>,
}

// Configuration and capabilities are immutable after startup. IPC callers can
// choose a registered application but cannot provide or replace these grants.
static GRANTS: OnceLock<Result<Vec<Grant>, &'static str>> = OnceLock::new();

fn load() -> Result<Vec<Grant>, &'static str> {
    let entries = match fs::read_dir(CONFIG_DIR) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("cannot read application capability configuration"),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| "cannot read capability directory entry")?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        if !entry
            .file_type()
            .map_err(|_| "capability file type")?
            .is_file()
        {
            return Err("capability configuration must be a regular file");
        }
        paths.push(path);
        if paths.len() > 32 {
            return Err("too many capability configuration files");
        }
    }
    paths.sort();
    let mut rules = Vec::new();
    for path in paths {
        let mut text = String::new();
        fs::File::open(path)
            .map_err(|_| "cannot open capability configuration")?
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|_| "cannot read capability configuration")?;
        let parsed = capability_config::parse(&text)?;
        for rule in parsed {
            if rules.iter().any(|old: &ApplicationGrant| old.id == rule.id) {
                return Err("duplicate application capability configuration");
            }
            rules.push(rule);
            if rules.len() > 32 {
                return Err("too many application capability grants");
            }
        }
    }
    if rules.is_empty() {
        return Ok(Vec::new());
    }
    let native = VfsView::current_admin().map_err(|_| "stemd has no native View authority")?;
    let mut grants = Vec::new();
    for rule in rules {
        let mut executable = native
            .open(&rule.argv[0], 0)
            .map_err(|_| "cannot open configured capability application")?;
        // EnvironmentSpawn accepts an already-open image, so resolve the same
        // system-shell script format accepted by the native app catalog here.
        let mut header = [0u8; 256];
        let mut length = 0;
        let stream = executable
            .as_stream()
            .map_err(|_| "application image read")?;
        while length < header.len() {
            let count = stream
                .read(&mut header[length..])
                .map_err(|_| "application image read")?;
            if count == 0 {
                break;
            }
            length += count;
        }
        executable
            .as_file()
            .map_err(|_| "application image seek")?
            .seek(scarlet_os::handle::capability::file::SeekFrom::Start(0))
            .map_err(|_| "application image seek")?;
        let line_end = header[..length].iter().position(|&byte| byte == b'\n');
        let shell_script = (line_end.is_some() || length < header.len())
            && std::str::from_utf8(&header[..line_end.unwrap_or(length)])
                .ok()
                .and_then(|line| line.strip_prefix("#!"))
                .is_some_and(|interpreter| interpreter.trim() == "/bin/sh");
        let mut execution_argv = rule.argv.clone();
        if shell_script {
            executable = native
                .open("/bin/sh", 0)
                .map_err(|_| "application system shell")?;
            execution_argv.insert(0, "/bin/sh".to_owned());
        }
        let mut views = Vec::new();
        for view in &rule.view {
            views.push(
                native
                    .rooted_at(&view.root)
                    .map_err(|_| "cannot construct delegated View")?,
            );
        }
        grants.push(Grant {
            rule,
            executable,
            execution_argv,
            views,
        });
    }
    Ok(grants)
}

pub fn initialize() -> Result<(), &'static str> {
    GRANTS
        .get_or_init(load)
        .as_ref()
        .map(|_| ())
        .map_err(|error| *error)
}

pub fn find(app_id: &str, argv: &[String]) -> Result<Option<&'static Grant>, &'static str> {
    let grants = GRANTS
        .get()
        .ok_or("application grants have not been initialized")?
        .as_ref()
        .map_err(|error| *error)?;
    match grants.iter().find(|grant| grant.rule.id == app_id) {
        Some(grant) => {
            capability_config::find_grant(std::slice::from_ref(&grant.rule), app_id, argv)?;
            Ok(Some(grant))
        }
        None => Ok(None),
    }
}

fn own_handle(fd: impl IntoRawFd) -> Result<Handle, &'static str> {
    // SAFETY: into_raw_fd transfers ownership; from_raw consumes it even on error.
    unsafe { Handle::from_raw(fd.into_raw_fd()) }.map_err(|_| "application stdio handle")
}

pub struct Spawned {
    pub pid: i32,
    pub stdout: std::io::PipeReader,
    pub stderr: std::io::PipeReader,
}

pub fn spawn(grant: &Grant, environment: &[String]) -> Result<Spawned, &'static str> {
    let stdin = own_handle(fs::File::open("/dev/null").map_err(|_| "application stdin")?)?;
    let (out_read, out_write) = std::io::pipe().map_err(|_| "application stdout pipe")?;
    let (err_read, err_write) = std::io::pipe().map_err(|_| "application stderr pipe")?;
    let stdout = own_handle(out_write)?;
    let stderr = own_handle(err_write)?;
    let mut handles = vec![
        HandleMapping {
            source: &stdin,
            target: 0,
        },
        HandleMapping {
            source: &stdout,
            target: 1,
        },
        HandleMapping {
            source: &stderr,
            target: 2,
        },
    ];
    handles.extend(
        grant
            .views
            .iter()
            .zip(&grant.rule.view)
            .map(|(view, rule)| HandleMapping {
                source: view.as_handle(),
                target: rule.target,
            }),
    );
    let argv: Vec<_> = grant.execution_argv.iter().map(String::as_str).collect();
    let environment: Vec<_> = environment.iter().map(String::as_str).collect();
    let pid = Environment::current()
        .map_err(|_| "stemd Environment")?
        .spawn_with_abi(
            "scarlet",
            &grant.executable,
            &argv,
            &environment,
            "/",
            &handles,
        )
        .map_err(|_| "capability application spawn")?;
    Ok(Spawned {
        pid: pid as i32,
        stdout: out_read,
        stderr: err_read,
    })
}
