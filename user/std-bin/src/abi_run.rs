//! Execute a program inside an ABI view of the current Environment.

#[cfg(target_os = "scarlet")]
#[path = "../../bin/src/abi_exec.rs"]
mod abi_exec;

#[cfg(target_os = "scarlet")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 || !args[2].starts_with('/') {
        eprintln!("usage: abi-run <abi> </executable> [args...]");
        std::process::exit(2);
    }
    let argv: Vec<&str> = args[2..].iter().map(String::as_str).collect();
    let environment: Vec<String> = std::env::vars()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let envp: Vec<&str> = environment.iter().map(String::as_str).collect();
    let result = abi_exec::exec(&args[1], &args[2], &argv, &envp, "/", None);
    eprintln!(
        "abi-run: cannot execute {} in {} ({:?})",
        args[2], args[1], result
    );
    std::process::exit(127);
}

#[cfg(not(target_os = "scarlet"))]
fn main() {
    eprintln!("abi-run is available only on Scarlet");
    std::process::exit(1);
}
