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
    source_path: Option<PathBuf>,
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
        let mut source_path = None;
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
                "--source" => {
                    source_path = Some(PathBuf::from(next_arg(&mut args, "--source")?));
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
            source_path,
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
        "Usage: scarlet-ui-preview --manifest-path <Cargo.toml> [--source <file.rs>] [--target <triple>] [--features <features>] [--preview <id-or-name>] [--poll-ms <ms>] [--build-only]"
    );
}

enum PreviewTarget {
    Library,
    Source {
        source_path: PathBuf,
        relative_path: PathBuf,
    },
}

struct BuildArtifact {
    dylib: PathBuf,
    crate_name: String,
    manifest_path: PathBuf,
    target_dir: PathBuf,
    features: Option<String>,
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
    let preview_target = resolve_preview_target(&config, &manifest_path, &crate_dir)?;

    println!("[preview] package={package_name}");
    println!("[preview] manifest={}", manifest_path.display());
    if let PreviewTarget::Source { source_path, .. } = &preview_target {
        println!("[preview] source={}", source_path.display());
    }

    if config.build_only {
        let library = build_and_load(
            &config,
            &manifest_path,
            &crate_dir,
            &package_name,
            &preview_target,
            0,
        )?;
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
        &preview_target,
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
                &preview_target,
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
    preview_target: &PreviewTarget,
    build_index: u64,
) -> Result<LoadedPreviewLibrary, String> {
    let artifact = build_preview_dylib(config, manifest_path, crate_dir, package_name, preview_target)?;
    run_cargo_preview_build(
        config,
        &artifact.manifest_path,
        &artifact.target_dir,
        artifact.features.as_deref(),
    )?;
    let dylib = artifact.dylib;
    if !dylib.exists() {
        return Err(format!("built dylib not found: {}", dylib.display()));
    }

    let copy_path = preview_copy_path(config, crate_dir, &artifact.crate_name, build_index);
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
    preview_target: &PreviewTarget,
) -> Result<BuildArtifact, String> {
    match preview_target {
        PreviewTarget::Library => {
            let wrapper = ensure_wrapper_crate(config, manifest_path, crate_dir, package_name)?;
            Ok(BuildArtifact {
                dylib: built_dylib_path(config, &wrapper.target_dir, &wrapper.crate_name),
                crate_name: wrapper.crate_name,
                manifest_path: wrapper.manifest_path,
                target_dir: wrapper.target_dir,
                features: None,
            })
        }
        PreviewTarget::Source {
            source_path,
            relative_path,
        } => ensure_source_preview_crate(
            config,
            manifest_path,
            crate_dir,
            package_name,
            source_path,
            relative_path,
        ),
    }
}

fn run_cargo_preview_build(
    config: &Config,
    manifest_path: &Path,
    target_dir: &Path,
    features: Option<&str>,
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

    if let Some(features) = features {
        command.arg("--features").arg(features);
    }

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
    crate_name: String,
    manifest_path: PathBuf,
    target_dir: PathBuf,
}

fn ensure_source_preview_crate(
    config: &Config,
    manifest_path: &Path,
    crate_dir: &Path,
    package_name: &str,
    source_path: &Path,
    relative_path: &Path,
) -> Result<BuildArtifact, String> {
    let work_dir = preview_work_dir(crate_dir, package_name).join("source");
    let shadow_dir = work_dir.join("package");
    let crate_name = String::from("scarlet_ui_preview_source");
    if shadow_dir.exists() {
        fs::remove_dir_all(&shadow_dir)
            .map_err(|error| format!("failed to clear shadow crate dir: {error}"))?;
    }
    fs::create_dir_all(&shadow_dir)
        .map_err(|error| format!("failed to create shadow crate dir: {error}"))?;

    mirror_crate_tree(crate_dir, &shadow_dir, source_path)?;

    let shadow_source_path = shadow_dir.join(relative_path);
    if let Some(parent) = shadow_source_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create shadow source dir: {error}"))?;
    }
    let sanitized = sanitize_source_file(source_path)?;
    fs::write(&shadow_source_path, sanitized)
        .map_err(|error| format!("failed to write shadow source file: {error}"))?;

    let cargo_toml = build_source_manifest(
        manifest_path,
        crate_dir,
        &crate_name,
        relative_path,
        config.extra_features.as_deref(),
    )?;
    fs::write(shadow_dir.join("Cargo.toml"), cargo_toml)
        .map_err(|error| format!("failed to write shadow manifest: {error}"))?;

    Ok(BuildArtifact {
        dylib: built_dylib_path(config, &work_dir.join("build"), &crate_name),
        crate_name,
        manifest_path: shadow_dir.join("Cargo.toml"),
        target_dir: work_dir.join("build"),
        features: Some(source_preview_features(config.extra_features.as_deref())),
    })
}

fn build_source_manifest(
    manifest_path: &Path,
    crate_dir: &Path,
    crate_name: &str,
    relative_source_path: &Path,
    _extra_features: Option<&str>,
) -> Result<String, String> {
    let scarlet_ui_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|error| format!("failed to resolve scarlet-ui path: {error}"))?;
    let edition = read_package_field(manifest_path, "edition")?
        .unwrap_or_else(|| String::from("2024"));
    let manifest_contents = fs::read_to_string(manifest_path)
        .map_err(|error| format!("failed to read manifest: {error}"))?;

    let mut out = String::new();
    out.push_str("[package]\n");
    out.push_str(&format!("name = {}\n", toml_string(&crate_name.replace('_', "-"))));
    out.push_str("version = \"0.1.0\"\n");
    out.push_str(&format!("edition = {}\n\n", toml_string(&edition)));
    out.push_str("[lib]\n");
    out.push_str("crate-type = [\"dylib\"]\n");
    out.push_str(&format!(
        "path = {}\n\n",
        toml_string(&relative_source_path.display().to_string())
    ));

    let filtered = filter_manifest_sections(&manifest_contents, crate_dir)?;
    if !filtered.trim().is_empty() {
        let filtered = ensure_feature_entry(&filtered, "preview", "preview = []");
        let filtered = ensure_dependency_entry(
            &filtered,
            "scarlet-ui",
            &format!(
                "scarlet-ui = {{ path = {}, default-features = false, features = [\"std\", \"platform-winit\", \"preview\"] }}",
                toml_string(&scarlet_ui_path.display().to_string())
            ),
        );
        out.push_str(&filtered);
        if !filtered.ends_with('\n') {
            out.push('\n');
        }
    } else {
        out.push_str("[features]\npreview = []\n\n");
        out.push_str(&format!(
            "[dependencies]\nscarlet-ui = {{ path = {}, default-features = false, features = [\"std\", \"platform-winit\", \"preview\"] }}\n",
            toml_string(&scarlet_ui_path.display().to_string())
        ));
    }

    Ok(out)
}

fn ensure_feature_entry(contents: &str, key: &str, entry: &str) -> String {
    ensure_section_entry(contents, "[features]", key, entry)
}

fn ensure_dependency_entry(contents: &str, key: &str, entry: &str) -> String {
    ensure_section_entry(contents, "[dependencies]", key, entry)
}

fn ensure_section_entry(contents: &str, section: &str, key: &str, entry: &str) -> String {
    if contents.lines().any(|line| manifest_line_key(line) == Some(key)) {
        return contents.to_string();
    }

    if let Some(index) = contents.find(section) {
        let insert_at = contents[index..]
            .find('\n')
            .map(|offset| index + offset + 1)
            .unwrap_or(contents.len());
        let mut output = String::new();
        output.push_str(&contents[..insert_at]);
        output.push_str(entry);
        output.push('\n');
        output.push_str(&contents[insert_at..]);
        return output;
    }

    let mut output = contents.to_string();
    if !output.ends_with('\n') && !output.is_empty() {
        output.push('\n');
    }
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(section);
    output.push('\n');
    output.push_str(entry);
    output.push('\n');
    output
}

fn manifest_line_key(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('[') || trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('=')?;
    Some(key.trim())
}

fn filter_manifest_sections(contents: &str, crate_dir: &Path) -> Result<String, String> {
    let mut output = String::new();
    let mut keep_section = false;
    let mut in_features = false;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            keep_section = should_keep_manifest_section(trimmed);
            in_features = keep_section && trimmed == "[features]";
            if keep_section {
                output.push_str(line);
                output.push('\n');
            }
            continue;
        }

        if !keep_section {
            continue;
        }

        if skip_scarlet_ui_dependency_line(trimmed) {
            continue;
        }

        if in_features && trimmed.starts_with("preview") && trimmed.contains('=') {
            continue;
        }

        output.push_str(&rewrite_manifest_paths(line, crate_dir)?);
        output.push('\n');
    }

    Ok(output)
}

fn should_keep_manifest_section(section: &str) -> bool {
    matches!(section, "[features]" | "[dependencies]" | "[build-dependencies]")
        || section.starts_with("[target.")
        || section.starts_with("[patch.")
}

fn skip_scarlet_ui_dependency_line(trimmed: &str) -> bool {
    trimmed.starts_with("scarlet-ui ")
        || trimmed.starts_with("scarlet-ui=")
        || trimmed.starts_with("scarlet-ui\t")
}

fn rewrite_manifest_paths(line: &str, crate_dir: &Path) -> Result<String, String> {
    let Some(path_index) = line.find("path = \"") else {
        return Ok(line.to_string());
    };
    let start = path_index + "path = \"".len();
    let Some(end_rel) = line[start..].find('"') else {
        return Ok(line.to_string());
    };
    let end = start + end_rel;
    let raw_path = &line[start..end];
    let absolute = crate_dir
        .join(raw_path)
        .canonicalize()
        .map_err(|error| format!("failed to resolve dependency path {}: {error}", raw_path))?;
    Ok(format!(
        "{}{}{}",
        &line[..start],
        absolute.display(),
        &line[end..]
    ))
}

fn source_preview_features(extra: Option<&str>) -> String {
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
    features.join(",")
}

fn mirror_crate_tree(crate_dir: &Path, shadow_dir: &Path, source_path: &Path) -> Result<(), String> {
    mirror_tree_recursive(crate_dir, shadow_dir, crate_dir, source_path)
}

fn mirror_tree_recursive(
    root: &Path,
    source_dir: &Path,
    target_dir: &Path,
    source_path: &Path,
) -> Result<(), String> {
    fs::create_dir_all(target_dir)
        .map_err(|error| format!("failed to create mirror dir {}: {error}", target_dir.display()))?;
    let entries = fs::read_dir(source_dir)
        .map_err(|error| format!("failed to read {}: {error}", source_dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to read directory entry: {error}"))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| format!("failed to relativize {}", path.display()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path == source_path || name == "target" || name == ".git" || name == ".scarlet-ui-preview" {
            continue;
        }
        let dest = target_dir.join(relative.file_name().unwrap());
        let metadata = entry
            .metadata()
            .map_err(|error| format!("failed to stat {}: {error}", path.display()))?;
        if metadata.is_dir() {
            mirror_tree_recursive(root, &path, &dest, source_path)?;
        } else {
            create_link_or_copy(&path, &dest)?;
        }
    }
    Ok(())
}

fn create_link_or_copy(source: &Path, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        fs::remove_file(dest)
            .map_err(|error| format!("failed to replace {}: {error}", dest.display()))?;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, dest)
            .map_err(|error| format!("failed to symlink {}: {error}", source.display()))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::copy(source, dest)
            .map_err(|error| format!("failed to copy {}: {error}", source.display()))?;
        Ok(())
    }
}

fn sanitize_source_file(source_path: &Path) -> Result<String, String> {
    let source = fs::read_to_string(source_path)
        .map_err(|error| format!("failed to read source {}: {error}", source_path.display()))?;
    let mut output = String::new();
    let mut stripping_inner_attrs = true;

    for line in source.lines() {
        let trimmed = line.trim();
        if stripping_inner_attrs {
            if trimmed.is_empty() {
                output.push_str(line);
                output.push('\n');
                continue;
            }
            if trimmed.starts_with("#![") {
                continue;
            }
            stripping_inner_attrs = false;
        }

        if trimmed == "extern crate scarlet_std as std;" {
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    output.push_str(
        "\n#[unsafe(no_mangle)]\npub fn scarlet_ui_preview_entry() -> ::scarlet_ui::__private::Box<dyn ::scarlet_ui::preview::PreviewLibrary> {\n    ::scarlet_ui::preview::registered_preview_library()\n}\n",
    );
    Ok(output)
}

fn resolve_preview_target(
    config: &Config,
    manifest_path: &Path,
    crate_dir: &Path,
) -> Result<PreviewTarget, String> {
    if let Some(source_path) = &config.source_path {
        let source_path = if source_path.is_absolute() {
            source_path.clone()
        } else {
            crate_dir.join(source_path)
        };
        let source_path = source_path
            .canonicalize()
            .map_err(|error| format!("failed to resolve source path: {error}"))?;
        let relative_path = source_path
            .strip_prefix(crate_dir)
            .map_err(|_| {
                format!(
                    "source {} must be inside {}",
                    source_path.display(),
                    crate_dir.display()
                )
            })?
            .to_path_buf();
        return Ok(PreviewTarget::Source {
            source_path,
            relative_path,
        });
    }

    if !has_library_target(manifest_path, crate_dir)? {
        return Err(String::from(
            "preview target must expose a library target, or pass --source <file.rs>",
        ));
    }
    Ok(PreviewTarget::Library)
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
    let crate_name = String::from("scarlet_ui_preview_wrapper");
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
name = "{}"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["dylib"]

[dependencies]
scarlet-ui = {{ path = {}, default-features = false, features = ["std", "platform-winit", "preview"] }}
preview-target = {{ package = {}, path = {}, features = {} }}
"#,
        crate_name.replace('_', "-"),
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
        crate_name,
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

fn built_dylib_path(config: &Config, target_dir: &Path, crate_name: &str) -> PathBuf {
    let mut path = target_dir.to_path_buf();
    if let Some(target) = &config.target {
        path = path.join(target);
    }
    path.join("debug").join(format!(
        "lib{}.{}",
        crate_name,
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
    read_package_field(manifest_path, "name")?
        .ok_or_else(|| String::from("failed to find [package] name in manifest"))
}

fn read_package_field(manifest_path: &Path, field_name: &str) -> Result<Option<String>, String> {
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
        if in_package && trimmed.starts_with(field_name) {
            let Some((_, value)) = trimmed.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            if !value.is_empty() {
                return Ok(Some(value.to_string()));
            }
        }
    }
    Ok(None)
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
