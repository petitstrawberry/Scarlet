use std::env;
use std::fs::File;
use std::io::{self, Read, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = env::args().skip(1);
    let mut saw_file = false;
    let mut failed = false;

    for path in args {
        saw_file = true;
        if let Err(err) = cat_file(&path) {
            println!("cat: {path}: {err}");
            failed = true;
        }
    }

    if !saw_file {
        println!("cat: missing file operand");
        println!("usage: cat [FILE]...");
        return ExitCode::from(1);
    }

    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn cat_file(path: &str) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut stdout = io::stdout();
    let mut buffer = [0; 4096];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            return stdout.flush();
        }

        stdout.write_all(&buffer[..bytes_read])?;
    }
}
