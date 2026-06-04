use std::env;
use std::fs;
use std::io::{self, Write};
use std::process::Command;

fn main() -> io::Result<()> {
    println!("std_hello: hello from Rust std on Scarlet");
    println!("std_hello: args={:?}", env::args().collect::<Vec<_>>());
    println!("std_hello: cwd={}", env::current_dir()?.display());

    fs::write("/tmp/std_hello.txt", b"scarlet rust std\n")?;
    let content = fs::read_to_string("/tmp/std_hello.txt")?;
    print!("std_hello: file={content}");
    io::stdout().flush()?;

    let status = Command::new("/bin/hello").status()?;
    println!("std_hello: /bin/hello status={status}");

    Ok(())
}
