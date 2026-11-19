//! Anonymous, display-only ScarletUI painter. No display/input device, desktop
//! socket, credential, verifier, clipboard service or IME is opened here.
#[path = "../greeter_ui.rs"]
mod greeter_ui;
#[allow(dead_code)]
#[path = "../soft_keyboard/layout.rs"]
mod keyboard_layout;
use std::io::{self, Read, Write};

mod fonts;

fn main() -> io::Result<()> {
    fonts::initialize()?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut request = [0; greeter_ui::REQUEST_BYTES];
    while input.read(&mut request[..1])? != 0 {
        input.read_exact(&mut request[1..])?;
        let screen = greeter_ui::Screen::decode(&request)?;
        let mut pipeline = screen.pipeline();
        let frame = pipeline
            .render()
            .ok_or_else(|| io::Error::other("greeter frame failed"))?;
        if frame.data().len() != screen.pixel_bytes() {
            return Err(io::Error::other("greeter frame size"));
        }
        output.write_all(frame.data())?;
        output.flush()?;
    }
    Ok(())
}
