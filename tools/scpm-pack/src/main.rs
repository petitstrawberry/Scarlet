use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};
use flate2::write::GzEncoder;
use flate2::Compression;
use tar::Builder;

#[derive(Parser)]
#[command(name = "scpm-pack")]
#[command(about = "Package builder for Scarlet Package Manager", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Build {
        /// Path to the package directory
        package_dir: String,
        /// Output path for the package file
        #[arg(short, long)]
        output: Option<String>,
    },
    Init {
        /// Package name
        name: String,
        /// Package version
        #[arg(short, long, default_value = "0.1.0")]
        version: String,
        /// Package description
        #[arg(short = 'd', long)]
        description: Option<String>,
        /// Author
        #[arg(short = 'a', long)]
        author: Option<String>,
        /// Architecture (riscv64, aarch64, any)
        #[arg(short, long, default_value = "any")]
        arch: String,
        /// Create package directory
        #[arg(short, long)]
        dir: String,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Build {
            package_dir,
            output,
        } => {
            if let Err(e) = build_package(&package_dir, output) {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
        }
        Commands::Init {
            name,
            version,
            description,
            author,
            arch,
            dir,
        } => {
            if let Err(e) = init_package(
                &name,
                &version,
                description.as_deref(),
                author.as_deref(),
                &arch,
                &dir,
            ) {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
        }
    }
}

fn build_package(package_dir: &str, output: Option<String>) -> Result<(), String> {
    let package_path = Path::new(package_dir);

    if !package_path.exists() {
        return Err(format!(
            "Package directory '{}' does not exist",
            package_dir
        ));
    }

    let metadata_file = package_path.join("package.toml");
    if !metadata_file.exists() {
        return Err(format!("package.toml not found in '{}'", package_dir));
    }

    let mut file =
        File::open(&metadata_file).map_err(|e| format!("Failed to open package.toml: {}", e))?;
    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|e| format!("Failed to read package.toml: {}", e))?;

    let name = content
        .lines()
        .find(|line| {
            let line = line.trim();
            line.starts_with("name") || (line.starts_with("[package]") && false)
        })
        .and_then(|line| line.split_once('='))
        .map(|(_, v)| v.trim().trim_matches('"').to_string())
        .unwrap_or_else(|| {
            content
                .lines()
                .skip_while(|l| !l.trim().starts_with("name"))
                .next()
                .and_then(|line| line.split_once('='))
                .map(|(_, v)| v.trim().trim_matches('"').to_string())
                .unwrap_or_else(|| String::from("unknown"))
        });

    let version = content
        .lines()
        .find(|line| line.trim().starts_with("version"))
        .and_then(|line| line.split_once('='))
        .map(|(_, v)| v.trim().trim_matches('"').to_string())
        .unwrap_or_else(|| String::from("0.0.0"));

    let output_file = output.unwrap_or_else(|| format!("{}-{}.scarlet", name, version));

    println!("Building package: {}-{}", name, version);
    println!("Output: {}", output_file);

    let output_path = PathBuf::from(&output_file);
    let output_tar_gz =
        File::create(&output_path).map_err(|e| format!("Failed to create output file: {}", e))?;
    let encoder = GzEncoder::new(output_tar_gz, Compression::default());
    let mut tar = Builder::new(encoder);

    tar.append_dir_all(".", package_path)
        .map_err(|e| format!("Failed to add files to archive: {}", e))?;

    let encoder = tar
        .into_inner()
        .map_err(|e| format!("Failed to finalize archive: {}", e))?;
    encoder
        .finish()
        .map_err(|e| format!("Failed to compress archive: {}", e))?;

    println!("Package built successfully!");

    Ok(())
}

fn init_package(
    name: &str,
    version: &str,
    description: Option<&str>,
    author: Option<&str>,
    arch: &str,
    dir: &str,
) -> Result<(), String> {
    let package_dir = Path::new(dir);
    if package_dir.exists() {
        return Err(format!("Directory '{}' already exists", dir));
    }

    fs::create_dir_all(package_dir)
        .map_err(|e| format!("Failed to create package directory: {}", e))?;

    let metadata = PackageMetadata {
        name: name.to_string(),
        version: version.to_string(),
        description: description.unwrap_or("").to_string(),
        author: author.map(String::from),
        homepage: None,
        binaries: Vec::new(),
        libraries: Vec::new(),
        dependencies: Vec::new(),
        architecture: arch.to_string(),
        license: None,
    };

    let mut toml_content = String::from("[package]\n");
    toml_content.push_str(&format!("name = \"{}\"\n", name));
    toml_content.push_str(&format!("version = \"{}\"\n", version));
    toml_content.push_str(&format!(
        "description = \"{}\"\n",
        description.unwrap_or("")
    ));
    if let Some(ref author) = author {
        toml_content.push_str(&format!("author = \"{}\"\n", author));
    }
    toml_content.push_str(&format!("architecture = \"{}\"\n", arch));
    toml_content.push_str("binaries = []\n");
    toml_content.push_str("libraries = []\n");
    toml_content.push_str("dependencies = []\n");

    let metadata_path = package_dir.join("package.toml");
    let mut file = File::create(&metadata_path)
        .map_err(|e| format!("Failed to create package.toml: {}", e))?;
    file.write_all(toml_content.as_bytes())
        .map_err(|e| format!("Failed to write package.toml: {}", e))?;

    println!("Initialized package: {}", name);
    println!("  Directory: {}", dir);
    println!("  Metadata: package.toml");
    println!("  Content: scarlet/");

    Ok(())
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub binaries: Vec<String>,
    pub libraries: Vec<String>,
    pub dependencies: Vec<Dependency>,
    pub architecture: String,
    pub license: Option<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct Dependency {
    pub name: String,
    pub version: Option<String>,
}
