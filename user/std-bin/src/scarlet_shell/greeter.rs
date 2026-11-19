//! Login and unlock panel renderer. The session manager supplies display state
//! on stdin and receives a fixed-size opaque BGRA panel on stdout. This process
//! never connects to the desktop display, clipboard, IME, or account database.
use scarlet_ui::{Color, graphics::Canvas};
use std::io::{self, Read, Write};

const WIDTH: u32 = 480;
const HEIGHT: u32 = 240;
const FRAME_BYTES: usize = WIDTH as usize * HEIGHT as usize * 4;

// A fixed-size message avoids allocating from a length supplied by the peer.
// Bytes 0..4 contain field, failed, masked character count, and name length.
// The rest contains the ASCII login name, zero-padded to the account limit.
fn render(request: &[u8; 36], pixels: &mut [u8]) -> io::Result<()> {
    let [field, failed, count, len] = request[..4].try_into().unwrap();
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid greeter state");
    if field > 2 || failed > 2 || count > 72 || len > 32 {
        return Err(invalid());
    }
    let name = &request[4..4 + usize::from(len)];
    if !name
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
        || request[4 + name.len()..].iter().any(|b| *b != 0)
        || (field == 2 && name.is_empty())
    {
        return Err(invalid());
    }
    let name = std::str::from_utf8(name).map_err(|_| invalid())?;
    let mut c = Canvas::new(pixels, WIDTH, HEIGHT);
    let surface = Color::rgb(31u8, 34u8, 42u8);
    let foreground = Color::rgb(247u8, 247u8, 250u8);
    let secondary = Color::rgb(196u8, 198u8, 208u8);
    let accent = Color::rgb(255u8, 142u8, 158u8);
    c.fill_rect(0, 0, WIDTH, HEIGHT, surface);
    c.draw_text_sized(
        24,
        16,
        if field == 2 {
            "Unlock session"
        } else {
            "Sign in"
        },
        foreground,
        28.,
    );
    if failed == 2 {
        c.draw_text_sized(24, 76, "No local account is available.", foreground, 20.);
        c.draw_text_sized(
            24,
            126,
            "Configure an image user to sign in.",
            secondary,
            16.,
        );
        c.draw_text_sized(24, 190, "Enter to check again", secondary, 16.);
        return Ok(());
    }
    c.draw_text_sized(24, 66, "User", secondary, 15.);
    c.draw_text_sized(24, 85, name, foreground, 20.);
    c.fill_rect(
        24,
        112,
        WIDTH - 48,
        2,
        if field == 0 { accent } else { secondary },
    );
    c.draw_text_sized(24, 124, "Password", secondary, 15.);
    for i in 0..u32::from(count.min(36)) {
        c.fill_rect(26 + (i * 11) as i32, 153, 5, 5, foreground);
    }
    c.fill_rect(
        24,
        171,
        WIDTH - 48,
        2,
        if field > 0 { accent } else { secondary },
    );
    let hint = match (failed, field) {
        (1, _) => "Authentication failed. Try again.",
        (_, 0) => "Enter to continue",
        (_, 1) => "Enter to sign in",
        _ => "Enter to unlock",
    };
    c.draw_text_sized(
        24,
        190,
        hint,
        if failed == 1 { accent } else { secondary },
        16.,
    );
    Ok(())
}

fn main() -> io::Result<()> {
    let font = std::fs::read("/share/fonts/Mplus1-Regular.ttf")?;
    scarlet_ui::graphics::set_default_font(font.leak())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid system font"))?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut request = [0; 36];
    let mut pixels = vec![0; FRAME_BYTES];
    while input.read(&mut request[..1])? != 0 {
        input.read_exact(&mut request[1..])?;
        render(&request, &mut pixels)?;
        output.write_all(&pixels)?;
        output.flush()?;
    }
    Ok(())
}
