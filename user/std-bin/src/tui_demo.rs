use std::io::{self, BufWriter, Read, Write};

use ratatui::Terminal;
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

struct AnsiBackend {
    stdout: BufWriter<io::Stdout>,
    width: u16,
    height: u16,
}

impl AnsiBackend {
    fn new(width: u16, height: u16) -> Self {
        Self {
            stdout: BufWriter::new(io::stdout()),
            width,
            height,
        }
    }
}

impl Backend for AnsiBackend {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        for (x, y, cell) in content {
            if cell.skip {
                continue;
            }
            write!(self.stdout, "\x1b[{};{}H", y + 1, x + 1)?;
            write_style(&mut self.stdout, cell.fg, cell.bg, cell.modifier)?;
            write!(self.stdout, "{}", cell.symbol())?;
            if !cell.modifier.is_empty() || cell.fg != Color::Reset || cell.bg != Color::Reset {
                write!(self.stdout, "\x1b[0m")?;
            }
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        write!(self.stdout, "\x1b[?25l")
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        write!(self.stdout, "\x1b[?25h")
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(Position { x: 0, y: 0 })
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let p = position.into();
        write!(self.stdout, "\x1b[{};{}H", p.y + 1, p.x + 1)
    }

    fn clear(&mut self) -> io::Result<()> {
        write!(self.stdout, "\x1b[2J\x1b[H")
    }

    fn clear_region(&mut self, ct: ClearType) -> io::Result<()> {
        match ct {
            ClearType::All => self.clear(),
            ClearType::CurrentLine => write!(self.stdout, "\x1b[2K"),
            ClearType::AfterCursor => write!(self.stdout, "\x1b[J"),
            ClearType::UntilNewLine => write!(self.stdout, "\x1b[K"),
            ClearType::BeforeCursor => write!(self.stdout, "\x1b[1J"),
        }
    }

    fn size(&self) -> io::Result<Size> {
        Ok(Size {
            width: self.width,
            height: self.height,
        })
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: Size {
                width: self.width,
                height: self.height,
            },
            pixels: Size {
                width: 0,
                height: 0,
            },
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdout.flush()
    }
}

fn write_style(buf: &mut impl Write, fg: Color, bg: Color, modifier: Modifier) -> io::Result<()> {
    if fg != Color::Reset {
        match fg {
            Color::Rgb(r, g, b) => write!(buf, "\x1b[38;2;{r};{g};{b}m")?,
            Color::Indexed(i) => write!(buf, "\x1b[38;5;{i}m")?,
            c => write!(buf, "\x1b[{}m", color_code(c, true))?,
        }
    }
    if bg != Color::Reset {
        match bg {
            Color::Rgb(r, g, b) => write!(buf, "\x1b[48;2;{r};{g};{b}m")?,
            Color::Indexed(i) => write!(buf, "\x1b[48;5;{i}m")?,
            c => write!(buf, "\x1b[{}m", color_code(c, false))?,
        }
    }
    let sgrs: [(Modifier, &str); 7] = [
        (Modifier::BOLD, "1"),
        (Modifier::DIM, "2"),
        (Modifier::ITALIC, "3"),
        (Modifier::UNDERLINED, "4"),
        (Modifier::SLOW_BLINK, "5"),
        (Modifier::REVERSED, "7"),
        (Modifier::CROSSED_OUT, "9"),
    ];
    for (flag, code) in &sgrs {
        if modifier.contains(*flag) {
            write!(buf, "\x1b[{code}m")?;
        }
    }
    Ok(())
}

fn color_code(color: Color, fg: bool) -> u8 {
    let base = if fg { 30 } else { 40 };
    match color {
        Color::Black => base,
        Color::Red => base + 1,
        Color::Green => base + 2,
        Color::Yellow => base + 3,
        Color::Blue => base + 4,
        Color::Magenta => base + 5,
        Color::Cyan => base + 6,
        Color::Gray => base + 7,
        Color::DarkGray => base + 60,
        Color::LightRed => base + 61,
        Color::LightGreen => base + 62,
        Color::LightYellow => base + 63,
        Color::LightBlue => base + 64,
        Color::LightMagenta => base + 65,
        Color::LightCyan => base + 66,
        Color::White => base + 67,
        _ => base,
    }
}

fn main() -> io::Result<()> {
    let backend = AnsiBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    terminal.hide_cursor()?;

    terminal.draw(|frame| {
        let area = frame.area();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Scarlet OS \u{2014} ratatui TUI Demo ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let lines = vec![
            ratatui::text::Line::from(""),
            ratatui::text::Line::from(ratatui::text::Span::styled(
                "  Hello from ratatui on Scarlet!",
                ratatui::style::Style::default()
                    .fg(ratatui::style::Color::Green)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            )),
            ratatui::text::Line::from(""),
            ratatui::text::Line::from("  Custom ANSI backend \u{2014} no crossterm needed"),
            ratatui::text::Line::from(format!("  Terminal size: {}x{}", area.width, area.height)),
            ratatui::text::Line::from(""),
            ratatui::text::Line::from(ratatui::text::Span::styled(
                "  Pure Rust TUI framework running on a custom OS",
                ratatui::style::Style::default().fg(ratatui::style::Color::Cyan),
            )),
            ratatui::text::Line::from(""),
            ratatui::text::Line::from(ratatui::text::Span::styled(
                "  Press Enter to exit...",
                ratatui::style::Style::default().fg(ratatui::style::Color::Yellow),
            )),
        ];
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: true });
        frame.render_widget(paragraph, inner);
    })?;

    let mut buf = [0u8; 1];
    let _ = io::stdin().read(&mut buf);

    terminal.show_cursor()?;
    writeln!(io::stdout(), "\x1b[2J\x1b[H")?;
    Ok(())
}
