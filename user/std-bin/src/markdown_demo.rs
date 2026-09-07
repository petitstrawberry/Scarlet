use std::io::{self, Write};

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

fn main() {
    let stdout = &mut io::stdout();

    let md = "\
# Scarlet OS

A **kernel** in Rust designed to provide a universal,
multi-ABI container runtime.

## Features

- Multi-ABI support (Scarlet / xv6 / Linux)
- Built-in hypervisor (SHV)
- Advanced VFS with ext2, FAT32, overlay
- `std` support for user programs

## Quick Start

```
cargo make run-riscv64
```

## Status

| ABI | Status |
|-----|--------|
| Scarlet Native | Complete |
| xv6 RISC-V 64 | Experimental |
| Linux RISC-V 64 | Partial |

> Cross-ABI pipes work seamlessly.

---

*See the docs for details.* \
**Now with `regex`, `serde_json`, `ureq`, and `pulldown-cmark`!**

~~No std support yet~~ Works great!
";

    writeln!(stdout).unwrap();

    let mut opts = pulldown_cmark::Options::empty();
    opts.insert(pulldown_cmark::Options::ENABLE_TABLES);
    opts.insert(pulldown_cmark::Options::ENABLE_STRIKETHROUGH);
    opts.insert(pulldown_cmark::Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(md, opts);
    let mut in_code_block = false;
    let mut in_blockquote = false;
    let mut table_row_index: usize = 0;

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let (prefix, color) = match level {
                    HeadingLevel::H1 => ("", "\x1b[1;36m"),
                    HeadingLevel::H2 => ("  ", "\x1b[1;33m"),
                    HeadingLevel::H3 => ("    ", "\x1b[1;32m"),
                    _ => ("      ", "\x1b[1;37m"),
                };
                write!(stdout, "\n{prefix}{color}").unwrap();
            }
            Event::End(TagEnd::Heading(_)) => {
                writeln!(stdout, "\x1b[0m").unwrap();
            }

            Event::Start(Tag::Paragraph) => {
                write!(stdout, "  ").unwrap();
            }
            Event::End(TagEnd::Paragraph) => {
                writeln!(stdout, "\n").unwrap();
            }

            Event::Start(Tag::Strong) => write!(stdout, "\x1b[1m").unwrap(),
            Event::End(TagEnd::Strong) => write!(stdout, "\x1b[22m").unwrap(),

            Event::Start(Tag::Emphasis) => write!(stdout, "\x1b[3m").unwrap(),
            Event::End(TagEnd::Emphasis) => write!(stdout, "\x1b[23m").unwrap(),

            Event::Start(Tag::Strikethrough) => write!(stdout, "\x1b[9m").unwrap(),
            Event::End(TagEnd::Strikethrough) => write!(stdout, "\x1b[29m").unwrap(),

            Event::Code(code) => {
                write!(stdout, "\x1b[36m{}\x1b[0m", code).unwrap();
            }

            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                writeln!(stdout, "  \x1b[90m\u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}").unwrap();
                write!(stdout, "  \x1b[90m\u{2502}\x1b[0m ").unwrap();
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                writeln!(stdout, "\x1b[90m\u{2502}\x1b[0m").unwrap();
                writeln!(stdout, "  \x1b[90m\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\x1b[0m").unwrap();
            }

            Event::Start(Tag::List(_)) => {}
            Event::End(TagEnd::List(_)) => {
                writeln!(stdout).unwrap();
            }
            Event::Start(Tag::Item) => {
                write!(stdout, "  \x1b[33m\u{2022}\x1b[0m ").unwrap();
            }
            Event::End(TagEnd::Item) => {
                writeln!(stdout).unwrap();
            }

            Event::Start(Tag::BlockQuote(_)) => {
                in_blockquote = true;
                write!(stdout, "  \x1b[35m\u{2588}\u{2588}\x1b[0m ").unwrap();
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                in_blockquote = false;
                writeln!(stdout).unwrap();
            }

            Event::Start(Tag::Table(_)) => {
                table_row_index = 0;
            }
            Event::End(TagEnd::Table) => {
                writeln!(stdout).unwrap();
            }
            Event::Start(Tag::TableHead) => {
                table_row_index = 0;
                write!(stdout, "  \x1b[1;36m").unwrap();
            }
            Event::End(TagEnd::TableHead) => {
                writeln!(stdout, "\x1b[0m").unwrap();
                writeln!(stdout, "  \x1b[90m\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\x1b[0m").unwrap();
            }
            Event::Start(Tag::TableRow) => {
                table_row_index += 1;
                write!(stdout, "  ").unwrap();
            }
            Event::End(TagEnd::TableRow) => {
                writeln!(stdout, "\x1b[0m").unwrap();
            }
            Event::Start(Tag::TableCell) => {
                let style = if table_row_index % 2 == 1 {
                    "\x1b[37m"
                } else {
                    "\x1b[90m"
                };
                write!(stdout, "{style}").unwrap();
            }
            Event::End(TagEnd::TableCell) => {
                write!(stdout, "\x1b[0m \x1b[90m\u{2502}\x1b[0m ").unwrap();
            }

            Event::Rule => {
                writeln!(stdout, "  \x1b[90m\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\x1b[0m").unwrap();
            }

            Event::SoftBreak | Event::HardBreak => {
                writeln!(stdout).unwrap();
                if in_blockquote {
                    write!(stdout, "  \x1b[35m\u{2588}\u{2588}\x1b[0m ").unwrap();
                } else if in_code_block {
                    write!(stdout, "  \x1b[90m\u{2502}\x1b[0m ").unwrap();
                }
            }

            Event::Text(text) => {
                if in_code_block {
                    write!(stdout, "\x1b[32m{}\x1b[0m", text).unwrap();
                } else {
                    write!(stdout, "{}", text).unwrap();
                }
            }

            _ => {}
        }
    }

    stdout.flush().unwrap();
}
