use std::env;
use std::fs;
use std::io::{self, Write};
use std::process;

fn main() -> io::Result<()> {
    println!("Hello, world!");
    println!("PID  = {}", process::id());
    println!("args = {:?}", env::args().collect::<Vec<_>>());
    println!("cwd  = {}", env::current_dir()?.display());

    fs::write("/tmp/hello.txt", b"scarlet rust std\n")?;
    let content = fs::read_to_string("/tmp/hello.txt")?;
    print!("file = {content}");
    io::stdout().flush()?;

    Ok(())
}
