use std::io::{self, Write};

fn main() {
    let stdout = &mut io::stdout();
    let text = "\
 Scarlet OS kernel log:
 [OK] Memory: 128 MB
 [OK] CPU: RISC-V 64-bit (RV64IMAFDC)
 [WARN] Driver: virtio-gpu not found
 [OK] Filesystem: ext2 mounted at /
 [OK] Network: 10.0.2.15/24
 [ERR] DNS: resolver timeout
 [OK] Shell: interactive mode
";

    let email_re = regex::Regex::new(r"\[OK\]|\[WARN\]|\[ERR\]").unwrap();
    let ip_re = regex::Regex::new(r"\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}").unwrap();

    writeln!(stdout, "=== Original log ===").unwrap();
    writeln!(stdout, "{text}").unwrap();

    writeln!(stdout, "=== Severity matches ===").unwrap();
    for cap in email_re.captures_iter(text) {
        writeln!(stdout, "  found: {}", &cap[0]).unwrap();
    }

    writeln!(stdout, "\n=== IP addresses ===").unwrap();
    for cap in ip_re.captures_iter(text) {
        writeln!(stdout, "  {}", &cap[0]).unwrap();
    }

    let replace_re = regex::Regex::new(r"\[(OK|WARN|ERR)\]").unwrap();
    let colored = replace_re.replace_all(text, |caps: &regex::Captures| -> String {
        match &caps[1] {
            "OK" => "\x1b[32m[OK]\x1b[0m".into(),
            "WARN" => "\x1b[33m[WARN]\x1b[0m".into(),
            "ERR" => "\x1b[31m[ERR]\x1b[0m".into(),
            _ => caps[0].to_string(),
        }
    });
    writeln!(stdout, "=== Colorized log ===").unwrap();
    writeln!(stdout, "{colored}").unwrap();

    stdout.flush().unwrap();
}
