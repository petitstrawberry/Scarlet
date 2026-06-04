use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = env::args().skip(1);
    let mut saw_file = false;
    let mut failed = false;

    for path in args {
        saw_file = true;
        if let Err(err) = fs::remove_file(&path) {
            println!("rm: cannot remove '{path}': {err}");
            failed = true;
        }
    }

    if !saw_file {
        println!("rm: missing operand");
        println!("Usage: rm FILE...");
        return ExitCode::from(1);
    }

    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
