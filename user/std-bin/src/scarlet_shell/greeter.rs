//! Decode wallpaper as the anonymous shell identity. The trusted editor owns
//! the SWS surface and paints the shell form over these display-only pixels.
mod wallpaper;
use std::io::{self, Read, Write};

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut request = [0u8; 8];
    while input.read(&mut request[..1])? != 0 {
        input.read_exact(&mut request[1..])?;
        let w = u32::from_le_bytes(request[..4].try_into().unwrap());
        let h = u32::from_le_bytes(request[4..].try_into().unwrap());
        if w == 0 || h == 0 || w > 1440 || h > 1440 {
            return Err(io::Error::other("invalid wallpaper size"));
        }
        let image = wallpaper::load_background_state().login_bitmap(w, h);
        for row in image.pixels().chunks_exact(w as usize) {
            let bytes: Vec<u8> = row.iter().flat_map(|p| p.to_le_bytes()).collect();
            output.write_all(&bytes)?;
        }
        output.flush()?;
    }
    Ok(())
}
