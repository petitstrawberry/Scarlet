#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use std::env;
use std::fs::File;
use std::io::Read;
use std::println;

use scpm::PackageManager;

fn print_help() {
    println!("Scarlet Package Manager (SCPM) v0.1.0");
    println!();
    println!("Usage: scpm <command> [options]");
    println!();
    println!("Commands:");
    println!("  install <package>    Install a package");
    println!("  remove <package>     Remove a package");
    println!("  list                 List installed packages");
    println!("  info <package>       Show package information");
    println!("  search <query>       Search for packages");
    println!("  help                 Show this help message");
}

fn cmd_list(_args: &[String]) {
    let manager = PackageManager::with_default_config();
    let installed = manager.list_installed();

    if installed.is_empty() {
        println!("No packages installed.");
    } else {
        println!("Installed packages:");
        for pkg in installed {
            println!("  {} - {}", pkg.name, pkg.version);
        }
    }
}

fn cmd_info(args: &[String]) {
    if args.is_empty() {
        println!("Error: Package name required");
        return;
    }

    let manager = PackageManager::with_default_config();
    let name = &args[0];

    match manager.get_installed(name) {
        Some(pkg) => {
            println!("Package: {}", pkg.name);
            println!("Version: {}", pkg.version);
        }
        None => {
            println!("Error: Package '{}' not found", name);
        }
    }
}

fn cmd_install(args: &[String]) {
    if args.is_empty() {
        println!("Error: Package path required");
        return;
    }

    let package_path = &args[0];
    let mut file = match File::open(package_path) {
        Ok(f) => f,
        Err(e) => {
            println!("Error opening package file: {:?}", e);
            return;
        }
    };

    let mut data = Vec::new();
    let mut buffer = [0u8; 8192];

    loop {
        match file.read(&mut buffer) {
            Ok(bytes_read) => {
                if bytes_read == 0 {
                    break;
                }
                data.extend_from_slice(&buffer[..bytes_read]);
            }
            Err(e) => {
                println!("Error reading package file: {:?}", e);
                return;
            }
        }
    }

    let mut manager = PackageManager::with_default_config();
    let package_name = String::from("package");
    match manager.install_from_bytes(&package_name, &data) {
        Ok(()) => println!("Package installed successfully"),
        Err(e) => println!("Error installing package: {}", e),
    }
}

fn cmd_remove(args: &[String]) {
    if args.is_empty() {
        println!("Error: Package name required");
        return;
    }
    println!("Removing: {}", args[0]);
    println!("Not yet implemented.");
}

fn run_command(cmd: &str, args: &[String]) {
    match cmd {
        "install" => cmd_install(args),
        "remove" => cmd_remove(args),
        "list" => cmd_list(args),
        "info" => cmd_info(args),
        "search" => println!("Repository not yet implemented."),
        "help" | "--help" | "-h" => print_help(),
        "--version" | "-v" => println!("SCPM v0.1.0"),
        _ => println!("Unknown command: {}", cmd),
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: Vec<String> = env::args().collect();

    if args.len() <= 1 {
        print_help();
        return 0;
    }

    run_command(&args[1], &args[2..]);
    0
}
