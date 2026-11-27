use super::*;
use joycon_layout::{RX, RY, SYN, X, Y};
use std::cell::RefCell;

#[derive(Debug, PartialEq)]
enum Action {
    Create(GamepadSpec),
    Write(Vec<Event>),
    Close,
}
thread_local! { static ACTIONS: RefCell<Vec<Action>> = const {RefCell::new(Vec::new())}; }

struct Fake;
impl Producer for Fake {
    fn create(spec: &GamepadSpec) -> Result<Self, HandleError> {
        ACTIONS.with_borrow_mut(|actions| actions.push(Action::Create(*spec)));
        Ok(Self)
    }
    fn publish(&self, frame: &[Event]) -> Result<(), StreamError> {
        ACTIONS.with_borrow_mut(|actions| actions.push(Action::Write(frame.to_vec())));
        Ok(())
    }
}
impl Drop for Fake {
    fn drop(&mut self) {
        ACTIONS.with_borrow_mut(|actions| actions.push(Action::Close));
    }
}
fn actions() -> Vec<Action> {
    ACTIONS.take()
}

// Round-trip using the exact bytes and per-side controls supplied by the raw ABI.
fn right(held: &[u16], x: i32) -> Sample {
    let mut decoder = Decoder::default();
    for key in [
        0x130, 0x131, 0x133, 0x134, 0x137, 0x139, 0x13b, 0x13c, 0x13e, 0x2c3, 0x2c4,
    ] {
        decoder.event(Id::Right(0), (KEY, key, i32::from(held.contains(&key))));
    }
    decoder.event(Id::Right(0), (ABS, RX, x));
    decoder.event(Id::Right(0), (ABS, RY, CENTER));
    decoder.event(Id::Right(0), SYN).unwrap()
}

#[test]
fn publication_releases_old_layout_before_creation_and_deduplicates_reports() {
    let mut outputs: BTreeMap<_, Output<Fake>> = BTreeMap::new();
    let mut seat = Seat::default();
    let id = Id::Right(0);
    let now = Instant::now();
    publish(&mut outputs, &seat, now);
    assert!(actions().is_empty());
    seat.update(id, right(&[0x131], 901));
    publish(&mut outputs, &seat, now);
    let initial = actions();
    let [Action::Create(spec), Action::Write(frame)] = initial.as_slice() else {
        panic!("{initial:?}")
    };
    assert_eq!(spec.axis_count, 2);
    assert_eq!([spec.axes[0].code, spec.axes[1].code], [RX, RY]);
    assert!(frame.contains(&(KEY, 0x131, 1)));
    assert!(frame.contains(&(ABS, RX, 901)));
    for _ in 0..3 {
        seat.update(id, right(&[0x131], 901));
        publish(&mut outputs, &seat, now);
    }
    assert!(actions().is_empty());
    seat.update(id, right(&[0x2c3, 0x2c4], 901));
    publish(&mut outputs, &seat, now);
    let switched = actions();
    let [Action::Close, Action::Create(spec), Action::Write(frame)] = switched.as_slice() else {
        panic!("{switched:?}")
    };
    assert_eq!([spec.axes[0].code, spec.axes[1].code], [X, Y]);
    assert!(frame.contains(&(KEY, 0x130, 0)));
    seat.remove(id);
    publish(&mut outputs, &seat, now);
    assert_eq!(actions(), vec![Action::Close]);
    publish(&mut outputs, &seat, now);
    assert!(actions().is_empty());
}

#[test]
fn distinct_press_release_in_one_drain_are_published_while_led_control_fails() {
    let mut outputs: BTreeMap<_, Output<Fake>> = BTreeMap::new();
    let mut seat = Seat::default();
    let mut light = PlayerLight::default();
    let start = Instant::now();
    let mut sent = Vec::new();
    for held in [&[0x131][..], &[][..]] {
        seat.update(Id::Right(0), right(held, CENTER));
        publish(&mut outputs, &seat, start);
        light.assign(1, start, |player| {
            sent.push(player);
            Err(HandleError::PermissionDenied)
        });
    }
    assert_eq!(sent, [1]);
    let result = actions();
    let [
        Action::Create(_),
        Action::Write(press),
        Action::Write(release),
    ] = result.as_slice()
    else {
        panic!("{result:?}")
    };
    assert!(press.contains(&(KEY, 0x131, 1)));
    assert!(release.contains(&(KEY, 0x131, 0)));
    light.assign(2, start + RESCAN, |p| {
        sent.push(p);
        Ok(0)
    });
    light.assign(2, start + RESCAN, |_| panic!("duplicate LED assignment"));
    light.assign(1, start + RESCAN, |p| {
        sent.push(p);
        Ok(0)
    });
    assert_eq!(sent, [1, 2, 1]);
}

#[test]
fn output_encoding_has_every_declared_control_once_and_final_sync() {
    let mut seat = Seat::default();
    let id = Id::Right(0);
    seat.update(id, right(&[0x131], 3111));
    let frame = seat.snapshot(Layout::Upright(id));
    let spec = descriptor(&frame);
    let bytes = encode(&frame);
    assert_eq!(
        bytes.len(),
        16 * (usize::from(spec.button_count + spec.axis_count) + 1)
    );
    let mut unique = std::collections::BTreeSet::new();
    for (i, record) in bytes.chunks_exact(RECORD).enumerate() {
        let ty = u16::from_ne_bytes(record[8..10].try_into().unwrap());
        let code = u16::from_ne_bytes(record[10..12].try_into().unwrap());
        let value = i32::from_ne_bytes(record[12..16].try_into().unwrap());
        assert_eq!(record[..8], [0; 8]);
        assert_eq!((ty, code, value), frame[i]);
        assert!(unique.insert((ty, code)));
    }
    assert_eq!(frame.last(), Some(&SYN));
}

#[test]
fn release_on_previously_quiet_reader_precedes_later_partner_press() {
    let left = Id::Left(0);
    let right_id = Id::Right(0);
    let mut d = Decoder::default();
    for key in [0x136, 0x138, 0x13a, 0x13d, 0x2c0, 0x2c1, 0x2c2] {
        d.event(left, (KEY, key, i32::from(key == 0x136)));
    }
    for (code, value) in [(X, CENTER), (Y, CENTER), (HAT_X, 0), (HAT_Y, 0)] {
        d.event(left, (ABS, code, value));
    }
    let mut seat = Seat::default();
    seat.update(left, d.event(left, SYN).unwrap());
    seat.update(right_id, right(&[], CENTER));

    // Only right was readable when poll returned for a stick update. While
    // draining, left releases after its descriptor was visited, then right presses.
    let mut readers = BTreeMap::from([
        (left, Vec::new()),
        (
            right_id,
            vec![
                (9, right_id, right(&[], 3100)),
                (12, right_id, right(&[0x137], 3100)),
            ],
        ),
    ]);
    let mut pending = Vec::new();
    let drain = |q: &mut Vec<TimedSample>, _, frames: &mut Vec<TimedSample>| {
        frames.append(q);
        true
    };
    let (first, _) = collect_through(&mut readers, &mut pending, 10, drain);
    assert_eq!(first.len(), 1);
    for (_, id, state) in first {
        seat.update(id, state);
    }
    readers
        .get_mut(&left)
        .unwrap()
        .push((11, left, Sample::default()));
    let (next, _) = collect_through(&mut readers, &mut pending, 15, drain);
    assert_eq!(next.iter().map(|e| e.0).collect::<Vec<_>>(), [11, 12]);
    for (_, id, state) in next {
        seat.update(id, state);
    }
    assert_eq!(seat.pads.len(), 2);
    assert!(!seat.pads.contains_key(&Layout::Pair(left, right_id)));
}
