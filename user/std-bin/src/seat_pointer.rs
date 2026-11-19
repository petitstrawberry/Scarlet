//! Bounded, single-contact login tap recognizer. Native events are committed
//! only at SYN_REPORT; a drag, second contact or lost report cancels the tap.
//! No synthetic events or public input channels are used.
#[derive(Clone, Copy)]
pub struct Axis {
    pub min: i32,
    pub max: i32,
}
impl Axis {
    fn pixel(self, value: i32, extent: u32) -> i32 {
        ((i64::from(value.clamp(self.min, self.max)) - i64::from(self.min))
            * i64::from(extent.saturating_sub(1))
            / (i64::from(self.max) - i64::from(self.min))) as i32
    }
}
#[derive(Clone, Copy, Default)]
struct Slot {
    active: bool,
    x: Option<i32>,
    y: Option<i32>,
}

pub struct Pointer {
    axes: Option<(Axis, Axis)>,
    mt: bool,
    slots: [Slot; 64],
    slot: Option<usize>,
    x: i32,
    y: i32,
    down: bool,
    origin: Option<(i32, i32)>,
    cancelled: bool,
    dropped: bool,
}
impl Pointer {
    pub fn new(axes: Option<(Axis, Axis)>, mt: bool) -> Self {
        Self {
            axes,
            mt,
            slots: [Slot::default(); 64],
            slot: Some(0),
            x: 0,
            y: 0,
            down: false,
            origin: None,
            cancelled: false,
            dropped: false,
        }
    }
    pub fn cancel(&mut self) {
        self.cancelled |= self.origin.is_some() || self.down || self.slots.iter().any(|s| s.active);
        self.origin = None;
    }
    pub fn event(
        &mut self,
        kind: u16,
        code: u16,
        value: i32,
        w: u32,
        h: u32,
    ) -> Option<(i32, i32)> {
        if kind == 0 && code == 3 {
            *self = Self::new(self.axes, self.mt);
            self.dropped = true;
            return None;
        }
        // Ignore the interrupted report. After SYN_REPORT a fresh button press
        // or tracking ID is required; coordinate-only motion cannot start a tap.
        if self.dropped {
            if (kind, code) == (0, 0) {
                self.dropped = false;
            }
            return None;
        }
        match (kind, code) {
            (3, 47) if self.mt => {
                self.slot = usize::try_from(value).ok().filter(|i| *i < 64);
            }
            (3, 57) if self.mt => {
                if let Some(i) = self.slot {
                    self.slots[i] = Slot {
                        active: value >= 0,
                        ..Slot::default()
                    };
                }
            }
            (3, 53 | 54) if self.mt => {
                if let Some(i) = self.slot {
                    if code == 53 {
                        self.slots[i].x = Some(value);
                    } else {
                        self.slots[i].y = Some(value);
                    }
                }
            }
            (3, 0 | 1) if !self.mt => {
                if code == 0 {
                    self.x = value;
                } else {
                    self.y = value;
                }
            }
            (2, 0 | 1) if self.axes.is_none() => {
                if code == 0 {
                    self.x = self
                        .x
                        .saturating_add(value)
                        .clamp(0, w.saturating_sub(1) as i32);
                } else {
                    self.y = self
                        .y
                        .saturating_add(value)
                        .clamp(0, h.saturating_sub(1) as i32);
                }
            }
            (1, 272 | 330) if !self.mt => self.down = value != 0,
            (0, 0) => {
                let (down, point) = if self.mt {
                    let mut contacts = self.slots.iter().filter(|s| s.active);
                    let first = contacts.next().copied();
                    if contacts.next().is_some() {
                        self.cancel();
                    }
                    (first.is_some(), first.and_then(|s| Some((s.x?, s.y?))))
                } else {
                    (self.down, Some((self.x, self.y)))
                };
                let point = point.map(|(x, y)| match self.axes {
                    Some((ax, ay)) => (ax.pixel(x, w), ay.pixel(y, h)),
                    None => (x, y),
                });
                if down {
                    if let Some(point) = point {
                        if let Some(origin) = self.origin {
                            if (point.0 - origin.0).abs() > 12 || (point.1 - origin.1).abs() > 12 {
                                self.cancel();
                            }
                        } else if !self.cancelled {
                            self.origin = Some(point);
                        }
                    }
                } else {
                    let tap = if self.cancelled {
                        None
                    } else {
                        self.origin.take()
                    };
                    self.origin = None;
                    self.cancelled = false;
                    return tap;
                }
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pointer(mt: bool) -> Pointer {
        Pointer::new(
            Some((
                Axis {
                    min: 100,
                    max: 1100,
                },
                Axis {
                    min: -500,
                    max: 500,
                },
            )),
            mt,
        )
    }
    fn event(p: &mut Pointer, kind: u16, code: u16, value: i32) -> Option<(i32, i32)> {
        p.event(kind, code, value, 801, 601)
    }
    #[test]
    fn absolute_touch_waits_for_release_and_scales_nonzero_ranges() {
        let mut p = pointer(false);
        event(&mut p, 3, 0, 600);
        event(&mut p, 3, 1, 0);
        event(&mut p, 1, 330, 1);
        assert_eq!(event(&mut p, 0, 0, 0), None);
        event(&mut p, 1, 330, 0);
        assert_eq!(event(&mut p, 0, 0, 0), Some((400, 300)));
        assert_eq!(event(&mut p, 0, 0, 0), None);
    }
    #[test]
    fn drag_and_dropped_report_cannot_type() {
        for drop in [false, true] {
            let mut p = pointer(false);
            event(&mut p, 1, 330, 1);
            event(&mut p, 0, 0, 0);
            if drop {
                event(&mut p, 0, 3, 0);
            } else {
                event(&mut p, 3, 0, 1100);
                event(&mut p, 0, 0, 0);
            }
            event(&mut p, 1, 330, 0);
            assert_eq!(event(&mut p, 0, 0, 0), None);
        }
    }
    #[test]
    fn multitouch_is_cancelled_and_slot_reuse_requires_new_coordinates() {
        let mut p = pointer(true);
        event(&mut p, 3, 57, 1);
        event(&mut p, 3, 53, 600);
        event(&mut p, 3, 54, 0);
        event(&mut p, 0, 0, 0);
        event(&mut p, 3, 47, 1);
        event(&mut p, 3, 57, 2);
        event(&mut p, 0, 0, 0);
        event(&mut p, 3, 57, -1);
        event(&mut p, 3, 47, 0);
        event(&mut p, 3, 57, -1);
        assert_eq!(event(&mut p, 0, 0, 0), None);
        event(&mut p, 3, 57, 3);
        event(&mut p, 0, 0, 0);
        event(&mut p, 3, 57, -1);
        assert_eq!(event(&mut p, 0, 0, 0), None);
    }
}
