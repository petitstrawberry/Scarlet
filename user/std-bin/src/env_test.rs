use std::env;
use std::process::{self, Command, ExitCode};

const TEST_ENV_KEY: &str = "SCARLET_TEST";
const TEST_ENV_VALUE: &str = "test_value_123";
const CHILD_MARKER: &str = "--child";

fn main() -> ExitCode {
    println!("=== Environment and Argument Test ===");
    println!("This test verifies Command argument and environment passing");

    let args = env::args().collect::<Vec<_>>();
    if args.get(1).is_some_and(|arg| arg == CHILD_MARKER) {
        run_child_test(&args)
    } else {
        run_parent_test(&args)
    }
}

fn run_child_test(args: &[String]) -> ExitCode {
    println!("=== CHILD PROCESS TEST ===");
    println!("This is the child process from Command");
    println!("PID: {}", process::id());
    println!("Child argc: {}", args.len());

    for (i, arg) in args.iter().enumerate() {
        println!("Child argv[{i}]: {arg}");
    }

    match env::var(TEST_ENV_KEY) {
        Ok(value) if value == TEST_ENV_VALUE => {
            println!("Environment variable passed correctly: {TEST_ENV_KEY}={value}");
        }
        Ok(value) => {
            println!(
                "Environment variable value mismatch: expected '{TEST_ENV_VALUE}', got '{value}'"
            );
            return ExitCode::from(1);
        }
        Err(err) => {
            println!("Test environment variable '{TEST_ENV_KEY}' not found: {err}");
            return ExitCode::from(1);
        }
    }

    println!("Child environment variables:");
    for (key, value) in env::vars() {
        println!("  {key}={value}");
    }

    println!("Child process test completed successfully");
    ExitCode::SUCCESS
}

fn run_parent_test(args: &[String]) -> ExitCode {
    println!("=== PARENT PROCESS TEST ===");
    println!("This is the parent process, about to spawn child");
    println!("PID: {}", process::id());
    println!("Parent argc: {}", args.len());

    for (i, arg) in args.iter().enumerate() {
        println!("Parent argv[{i}]: {arg}");
    }

    println!("Parent environment variables:");
    for (key, value) in env::vars() {
        println!("  {key}={value}");
    }

    println!("Spawning child process...");
    let status = Command::new("/bin/env-test")
        .arg(CHILD_MARKER)
        .arg("test_arg1")
        .arg("test arg with spaces")
        .env_clear()
        .env("PATH", "/bin:/usr/bin")
        .env("HOME", "/root")
        .env(TEST_ENV_KEY, TEST_ENV_VALUE)
        .env("SHELL", "/bin/sh")
        .status();

    match status {
        Ok(status) if status.success() => {
            println!("Command env test completed successfully");
            ExitCode::SUCCESS
        }
        Ok(status) => {
            println!("Child process failed with status {status}");
            ExitCode::from(1)
        }
        Err(err) => {
            println!("Failed to spawn child process: {err}");
            ExitCode::from(1)
        }
    }
}
