use std::env;
use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutputMode {
    Text,
    Json,
}

#[derive(Debug, Parser)]
#[command(name = "clap_demo", version, about = "Exercise clap on Scarlet std")]
struct Cli {
    #[arg(short, long, default_value = "world")]
    name: String,

    #[arg(short, long, default_value_t = 1)]
    repeat: usize,

    #[arg(long, value_enum, default_value_t = OutputMode::Text)]
    mode: OutputMode,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Echo {
        #[arg(required = true)]
        words: Vec<String>,
    },
    Inspect,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let mut stdout = io::stdout();

    match cli.command {
        Some(Command::Echo { words }) => {
            write_lines(&mut stdout, cli.mode, &words)?;
        }
        Some(Command::Inspect) => {
            let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("<unknown>"));
            let args = env::args().collect::<Vec<_>>();
            let lines = [
                format!("cwd={}", cwd.display()),
                format!("argc={}", args.len()),
                format!("args={args:?}"),
            ];
            write_lines(&mut stdout, cli.mode, &lines)?;
        }
        None => {
            for index in 0..cli.repeat {
                writeln!(stdout, "{}: hello, {}", index + 1, cli.name)?;
            }
        }
    }

    stdout.flush()
}

fn write_lines(
    stdout: &mut impl Write,
    mode: OutputMode,
    lines: &[impl AsRef<str>],
) -> io::Result<()> {
    match mode {
        OutputMode::Text => {
            for line in lines {
                writeln!(stdout, "{}", line.as_ref())?;
            }
        }
        OutputMode::Json => {
            write!(stdout, "[")?;
            for (index, line) in lines.iter().enumerate() {
                if index != 0 {
                    write!(stdout, ",")?;
                }
                write!(stdout, "\"{}\"", escape_json(line.as_ref()))?;
            }
            writeln!(stdout, "]")?;
        }
    }

    Ok(())
}

fn escape_json(input: &str) -> String {
    let mut escaped = String::new();
    for ch in input.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch => escaped.push(ch),
        }
    }
    escaped
}
