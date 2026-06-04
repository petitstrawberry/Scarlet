use std::env;
use std::fs;
use std::io;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1).peekable();
    let mut failed = false;

    if args.peek().is_none() {
        if let Err(err) = list_path(".") {
            println!("ls: cannot open '.': {err}");
            failed = true;
        }
    } else {
        for path in args {
            if let Err(err) = list_path(&path) {
                println!("ls: cannot open '{path}': {err}");
                failed = true;
            }
        }
    }

    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn list_path(path: &str) -> io::Result<()> {
    let mut rows = Vec::new();

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy().into_owned();
        let file_type = entry.file_type()?;
        let metadata = entry.metadata()?;

        rows.push(Row {
            kind: kind_name(file_type),
            size: metadata.len(),
            name,
        });
    }

    rows.sort_by(|left, right| left.name.cmp(&right.name));
    let kind_width = rows.iter().map(|row| row.kind.len()).max().unwrap_or(0);
    let size_width = rows
        .iter()
        .map(|row| decimal_width(row.size))
        .max()
        .unwrap_or(0);

    for row in rows {
        println!(
            "{:kind_width$} {:>size_width$} {}",
            row.kind, row.size, row.name
        );
    }

    Ok(())
}

struct Row {
    kind: &'static str,
    size: u64,
    name: String,
}

fn kind_name(file_type: fs::FileType) -> &'static str {
    if file_type.is_dir() {
        "Directory"
    } else if file_type.is_file() {
        "File"
    } else if file_type.is_symlink() {
        "Symlink"
    } else {
        "Other"
    }
}

fn decimal_width(value: u64) -> usize {
    if value == 0 {
        return 1;
    }

    let mut width = 0;
    let mut remaining = value;
    while remaining > 0 {
        width += 1;
        remaining /= 10;
    }
    width
}
