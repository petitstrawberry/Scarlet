use scarlet_os::Handle;
use sws_client_c::scene::SwsRect;

pub struct Input {
    keyboard: Handle,
    touch: Handle,
    keys: [bool; 256],
    down: bool,
    last: (i32, i32),
    width: u32,
    height: u32,
}
fn send(handle: &Handle, events: &[(u16, u16, i32)]) -> Result<(), &'static str> {
    let mut bytes = Vec::with_capacity((events.len() + 1) * 16);
    for &(kind, code, value) in events.iter().chain(std::iter::once(&(0, 0, 0))) {
        bytes.extend_from_slice(&0u64.to_ne_bytes());
        bytes.extend_from_slice(&kind.to_ne_bytes());
        bytes.extend_from_slice(&code.to_ne_bytes());
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    handle
        .as_stream()
        .map_err(|_| "input stream")?
        .write_all(&bytes)
        .map_err(|_| "input delivery")
}
pub fn fit(content: SwsRect, width: u32, height: u32) -> SwsRect {
    let (w, h) = if u64::from(content.width) * u64::from(height)
        <= u64::from(content.height) * u64::from(width)
    {
        (
            content.width,
            (u64::from(content.width) * u64::from(height) / u64::from(width)) as u32,
        )
    } else {
        (
            (u64::from(content.height) * u64::from(width) / u64::from(height)) as u32,
            content.height,
        )
    };
    SwsRect {
        x: content.x + (content.width - w) / 2,
        y: content.y + (content.height - h) / 2,
        width: w,
        height: h,
    }
}
fn map(r: SwsRect, width: u32, height: u32, x: i32, y: i32) -> Option<(i32, i32)> {
    if r.width == 0 || r.height == 0 {
        return None;
    }
    Some((
        ((i64::from(x) - i64::from(r.x)).clamp(0, i64::from(r.width) - 1) * i64::from(width)
            / i64::from(r.width)) as i32,
        ((i64::from(y) - i64::from(r.y)).clamp(0, i64::from(r.height) - 1) * i64::from(height)
            / i64::from(r.height)) as i32,
    ))
}
pub fn contains(r: SwsRect, x: i32, y: i32) -> bool {
    i64::from(x) >= i64::from(r.x)
        && i64::from(y) >= i64::from(r.y)
        && i64::from(x) < i64::from(r.x) + i64::from(r.width)
        && i64::from(y) < i64::from(r.y) + i64::from(r.height)
}
impl Input {
    pub fn new(keyboard: Handle, touch: Handle, width: u32, height: u32) -> Self {
        Self {
            keyboard,
            touch,
            keys: [false; 256],
            down: false,
            last: (0, 0),
            width,
            height,
        }
    }
    pub fn key(&mut self, code: u16, value: i32) -> Result<(), &'static str> {
        if !(1..=255).contains(&code) || !(0..=2).contains(&value) {
            return Ok(());
        }
        send(&self.keyboard, &[(1, code, value)])?;
        self.keys[code as usize] = value != 0;
        Ok(())
    }
    pub fn pointer(
        &mut self,
        r: SwsRect,
        x: i32,
        y: i32,
        button: Option<bool>,
        inside: bool,
    ) -> Result<(), &'static str> {
        if button == Some(true) && !inside {
            return Ok(());
        }
        if !self.down && button != Some(true) {
            return Ok(());
        }
        let Some((x, y)) = map(r, self.width, self.height, x, y) else {
            return self.release_touch();
        };
        let down = button.unwrap_or(self.down);
        send(
            &self.touch,
            &[(3, 0, x), (3, 1, y), (1, 0x14a, i32::from(down))],
        )?;
        self.last = (x, y);
        self.down = down;
        Ok(())
    }
    fn release_touch(&mut self) -> Result<(), &'static str> {
        if self.down {
            send(
                &self.touch,
                &[(3, 0, self.last.0), (3, 1, self.last.1), (1, 0x14a, 0)],
            )?;
            self.down = false;
        }
        Ok(())
    }
    pub fn release(&mut self) -> Result<(), &'static str> {
        self.release_touch()?;
        for code in 1..256 {
            if self.keys[code] {
                send(&self.keyboard, &[(1, code as u16, 0)])?;
                self.keys[code] = false;
            }
        }
        Ok(())
    }
}
impl Drop for Input {
    fn drop(&mut self) {
        let _ = self.release();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn letterbox_mapping_matches_physical_content_and_clamps_captured_drag() {
        let r = fit(
            SwsRect {
                x: 2,
                y: 42,
                width: 800,
                height: 480,
            },
            640,
            480,
        );
        assert_eq!((r.x, r.y, r.width, r.height), (82, 42, 640, 480));
        assert!(!contains(r, 81, 42));
        assert_eq!(map(r, 640, 480, 82, 42), Some((0, 0)));
        assert_eq!(map(r, 640, 480, 1000, -1), Some((639, 0)));
    }
}
