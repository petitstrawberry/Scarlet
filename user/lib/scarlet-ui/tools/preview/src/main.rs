use std::collections::hash_map::DefaultHasher;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime};

use scarlet_ui::preview::{LoadedPreviewLibrary, PreviewHost};

struct Config {
    manifest_path: PathBuf,
    target: Option<String>,
    extra_features: Option<String>,
    preview: Option<String>,
    poll_interval: Duration,
    build_only: bool,
}

fn main() {
    let config = match Config::parse() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            print_usage();
            std::process::exit(2);
        }
    };

    if let Err(error) = run(config) {
        eprintln!("[preview] {error}");
        std::process::exit(1);
    }
}

impl Config {
    fn parse() -> Result<Self, String> {
        let mut args = env::args_os().skip(1);
        let mut manifest_path = None;
        let mut target = None;
        let mut extra_features = None;
        let mut preview = None;
        let mut poll_interval = Duration::from_millis(250);
        let mut build_only = false;

        while let Some(arg) = args.next() {
            match arg.to_string_lossy().as_ref() {
                "--manifest-path" => {
                    manifest_path = Some(PathBuf::from(next_arg(&mut args, "--manifest-path")?));
                }
                "--target" => {
                    target = Some(
                        next_arg(&mut args, "--target")?
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
                "--features" => {
                    extra_features = Some(
                        next_arg(&mut args, "--features")?
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
                "--preview" => {
                    preview = Some(
                        next_arg(&mut args, "--preview")?
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
                "--poll-ms" => {
                    let value = next_arg(&mut args, "--poll-ms")?;
                    let millis = value
                        .to_string_lossy()
                        .parse::<u64>()
                        .map_err(|_| String::from("--poll-ms expects an integer"))?;
                    poll_interval = Duration::from_millis(millis.max(16));
                }
                "--build-only" => {
                    build_only = true;
                }
                "-h" | "--help" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }

        let manifest_path =
            manifest_path.ok_or_else(|| String::from("--manifest-path is required"))?;
        Ok(Self {
            manifest_path,
            target,
            extra_features,
            preview,
            poll_interval,
            build_only,
        })
    }
}

fn next_arg(args: &mut impl Iterator<Item = OsString>, name: &str) -> Result<OsString, String> {
    args.next()
        .ok_or_else(|| format!("{name} requires a value"))
}

fn print_usage() {
    eprintln!(
        "Usage: scarlet-ui-preview --manifest-path <Cargo.toml> [--target <triple>] [--features <features>] [--preview <id-or-name>] [--poll-ms <ms>] [--build-only]"
    );
}

fn run(config: Config) -> Result<(), String> {
    let manifest_path = config
        .manifest_path
        .canonicalize()
        .map_err(|error| format!("failed to resolve manifest path: {error}"))?;
    let crate_dir = manifest_path
        .parent()
        .ok_or_else(|| String::from("manifest path has no parent"))?
        .to_path_buf();
    let package_name = read_package_name(&manifest_path)?;
    if !has_library_target(&manifest_path, &crate_dir)? {
        return Err(String::from(
            "preview target must expose a library target; move preview functions to src/lib.rs or a dedicated preview crate",
        ));
    }

    println!("[preview] package={package_name}");
    println!("[preview] manifest={}", manifest_path.display());

    if config.build_only {
        let library = build_and_load(&config, &manifest_path, &crate_dir, &package_name, 0)?;
        print_previews(&library);
        println!("[preview] build loaded");
        return Ok(());
    }

    let mut last_seen = latest_source_mtime(&crate_dir)?;
    let mut build_index = 0u64;
    let mut host: Option<PreviewHost> = None;

    match build_and_load(
        &config,
        &manifest_path,
        &crate_dir,
        &package_name,
        build_index,
    ) {
        Ok(library) => {
            print_previews(&library);
            println!("[preview] initial build loaded");
            host = Some(PreviewHost::new_with_selection(
                library,
                config.preview.as_deref(),
            )?);
        }
        Err(error) => {
            eprintln!("[preview] initial build failed: {error}");
        }
    }

    loop {
        let current_mtime = latest_source_mtime(&crate_dir)?;
        if current_mtime > last_seen {
            last_seen = current_mtime;
            build_index = build_index.wrapping_add(1);
            println!("[preview] change detected; rebuilding");
            match build_and_load(
                &config,
                &manifest_path,
                &crate_dir,
                &package_name,
                build_index,
            ) {
                Ok(library) => {
                    print_previews(&library);
                    if let Some(host) = host.as_mut() {
                        host.reload(library)?;
                        println!("[preview] reloaded");
                    } else {
                        host = Some(PreviewHost::new(library)?);
                        println!("[preview] loaded");
                    }
                }
                Err(error) => {
                    eprintln!("[preview] rebuild failed; keeping previous preview: {error}");
                }
            }
        }

        if let Some(host) = host.as_mut() {
            if !host.tick(Duration::from_millis(16))? {
                break;
            }
        } else {
            thread::sleep(config.poll_interval);
        }
    }

    Ok(())
}

fn build_and_load(
    config: &Config,
    manifest_path: &Path,
    crate_dir: &Path,
    package_name: &str,
    build_index: u64,
) -> Result<LoadedPreviewLibrary, String> {
    let dylib = build_preview_dylib(config, manifest_path, crate_dir, package_name)?;
    if !dylib.exists() {
        return Err(format!("built dylib not found: {}", dylib.display()));
    }

    let copy_path = preview_copy_path(config, crate_dir, package_name, build_index);
    if let Some(parent) = copy_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create preview cache dir: {error}"))?;
    }
    fs::copy(&dylib, &copy_path).map_err(|error| {
        format!(
            "failed to copy {} to {}: {error}",
            dylib.display(),
            copy_path.display()
        )
    })?;

    unsafe { LoadedPreviewLibrary::load(&copy_path) }
}

fn print_previews(library: &LoadedPreviewLibrary) {
    for preview in library.previews() {
        println!(
            "[preview] available: {} ({})",
            preview.name,
            preview.id.as_str()
        );
    }
}

fn build_preview_dylib(
    config: &Config,
    manifest_path: &Path,
    crate_dir: &Path,
    package_name: &str,
) -> Result<PathBuf, String> {
    let wrapper = ensure_wrapper_crate(config, manifest_path, crate_dir, package_name)?;
    run_cargo_preview_build(config, &wrapper.manifest_path, &wrapper.target_dir)?;
    Ok(built_wrapper_dylib_path(config, &wrapper.target_dir))
}

fn run_cargo_preview_build(
    config: &Config,
    manifest_path: &Path,
    target_dir: &Path,
) -> Result<(), String> {
    let mut command = Command::new("cargo");
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(manifest_path)
        .arg("--target-dir")
        .arg(target_dir)
        .arg("--lib")
        .arg("--quiet");

    if let Some(target) = &config.target {
        command.arg("--target").arg(target);
    }

    let status = command
        .status()
        .map_err(|error| format!("failed to run cargo build: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo build failed with status {status}"))
    }
}

struct WrapperCrate {
    manifest_path: PathBuf,
    target_dir: PathBuf,
}

fn ensure_wrapper_crate(
    config: &Config,
    manifest_path: &Path,
    crate_dir: &Path,
    package_name: &str,
) -> Result<WrapperCrate, String> {
    let work_dir = preview_work_dir(crate_dir, package_name);
    let wrapper_dir = work_dir.join("wrapper");
    let src_dir = wrapper_dir.join("src");
    fs::create_dir_all(&src_dir)
        .map_err(|error| format!("failed to create preview wrapper dir: {error}"))?;

    let scarlet_ui_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|error| format!("failed to resolve scarlet-ui path: {error}"))?;
    let target_dir = manifest_path
        .parent()
        .ok_or_else(|| String::from("manifest path has no parent"))?;

    let cargo_toml = format!(
        r#"[package]
name = "scarlet-ui-preview-wrapper"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["dylib"]

[dependencies]
scarlet-ui = {{ path = {}, default-features = false, features = ["std", "platform-winit", "preview"] }}
preview-target = {{ package = {}, path = {}, features = {} }}
"#,
        toml_string(&scarlet_ui_path.display().to_string()),
        toml_string(package_name),
        toml_string(&target_dir.display().to_string()),
        preview_feature_array(config.extra_features.as_deref()),
    );
    let lib_rs = r#"extern crate preview_target as _;

#[unsafe(no_mangle)]
pub fn scarlet_ui_preview_entry() -> ::scarlet_ui::__private::Box<dyn ::scarlet_ui::preview::PreviewLibrary> {
    ::scarlet_ui::preview::registered_preview_library()
}
"#;

    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)
        .map_err(|error| format!("failed to write preview wrapper manifest: {error}"))?;
    fs::write(src_dir.join("lib.rs"), lib_rs)
        .map_err(|error| format!("failed to write preview wrapper source: {error}"))?;

    Ok(WrapperCrate {
        manifest_path: wrapper_dir.join("Cargo.toml"),
        target_dir: work_dir.join("build"),
    })
}

fn preview_feature_array(extra: Option<&str>) -> String {
    let mut features = vec![String::from("preview")];
    if let Some(extra) = extra {
        for feature in extra
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            if feature != "preview" {
                features.push(feature.to_string());
            }
        }
    }
    format!(
        "[{}]",
        features
            .iter()
            .map(|feature| toml_string(feature))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn built_wrapper_dylib_path(config: &Config, target_dir: &Path) -> PathBuf {
    let mut path = target_dir.to_path_buf();
    if let Some(target) = &config.target {
        path = path.join(target);
    }
    path.join("debug").join(format!(
        "libscarlet_ui_preview_wrapper.{}",
        dylib_extension()
    ))
}

fn preview_copy_path(
    config: &Config,
    crate_dir: &Path,
    package_name: &str,
    build_index: u64,
) -> PathBuf {
    let target = config.target.as_deref().unwrap_or("host");
    preview_work_dir(crate_dir, package_name)
        .join(target)
        .join(format!(
            "lib{}-{}.{}",
            lib_name(package_name),
            build_index,
            dylib_extension()
        ))
}

fn preview_work_dir(crate_dir: &Path, package_name: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    crate_dir.hash(&mut hasher);
    package_name.hash(&mut hasher);
    env::temp_dir()
        .join("scarlet-ui-preview")
        .join(format!("{:016x}", hasher.finish()))
}

fn lib_name(package_name: &str) -> String {
    package_name.replace('-', "_")
}

fn dylib_extension() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "dylib"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "so"
    }
    #[cfg(windows)]
    {
        "dll"
    }
}

fn read_package_name(manifest_path: &Path) -> Result<String, String> {
    let contents = fs::read_to_string(manifest_path)
        .map_err(|error| format!("failed to read manifest: {error}"))?;
    let mut in_package = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed == "[package]" {
            in_package = true;
            continue;
        }
        if in_package && trimmed.starts_with('[') {
            break;
        }
        if in_package && trimmed.starts_with("name") {
            let Some((_, value)) = trimmed.split_once('=') else {
                continue;
            };
            let name = value.trim().trim_matches('"');
            if !name.is_empty() {
                return Ok(name.to_string());
            }
        }
    }
    Err(String::from("failed to find [package] name in manifest"))
}

fn has_library_target(manifest_path: &Path, crate_dir: &Path) -> Result<bool, String> {
    let contents = fs::read_to_string(manifest_path)
        .map_err(|error| format!("failed to read manifest: {error}"))?;
    if contents.lines().any(|line| line.trim() == "[lib]") {
        return Ok(true);
    }
    Ok(crate_dir.join("src").join("lib.rs").exists())
}

fn latest_source_mtime(root: &Path) -> Result<SystemTime, String> {
    let mut latest = SystemTime::UNIX_EPOCH;
    visit_sources(root, &mut |path| {
        if let Ok(metadata) = fs::metadata(path)
            && let Ok(modified) = metadata.modified()
            && modified > latest
        {
            latest = modified;
        }
    })?;
    Ok(latest)
}

fn visit_sources(root: &Path, visitor: &mut impl FnMut(&Path)) -> Result<(), String> {
    let entries = fs::read_dir(root)
        .map_err(|error| format!("failed to read directory {}: {error}", root.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to read directory entry: {error}"))?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if name == "target" || name == ".git" || name == ".scarlet-ui-preview" {
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|error| format!("failed to stat {}: {error}", path.display()))?;
        if metadata.is_dir() {
            visit_sources(&path, visitor)?;
        } else if is_source_file(&path) {
            visitor(&path);
        }
    }
    Ok(())
}

fn is_source_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("rs" | "toml" | "lock")
    )
}
