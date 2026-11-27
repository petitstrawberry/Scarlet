use super::*;

const LEFT: Id = Id::Left(0);
const RIGHT: Id = Id::Right(0);
const PAIR: Layout = Layout::Pair(LEFT, RIGHT);

fn sample(keys: &[u16]) -> Sample {
    Sample {
        keys: keys.iter().copied().map(bit).fold(0, |a, b| a | b),
        ..Sample::default()
    }
}

fn assert_events(frame: &[Event], expected: &[Event]) {
    for event in expected {
        assert!(frame.contains(event), "missing {event:?} in {frame:?}");
    }
}

#[test]
fn raw_initial_frame_and_overflow_recover_complete_right_state() {
    let held = Sample {
        stick: [3810, 89],
        ..sample(&[EAST, THUMBR])
    };
    let mut frame = Vec::new();
    upright(&mut frame, RIGHT, held);
    let mut decoder = Decoder::default();
    let mut seat = Seat::default();
    // First open, including a daemon restart while the stick is held.
    for &event in &frame {
        assert!(decoder.event(RIGHT, event).is_none());
    }
    seat.update(RIGHT, decoder.event(RIGHT, SYN).unwrap());
    let layout = Layout::Upright(RIGHT);
    assert_eq!(seat.pads[&layout], 1);
    let output = seat.snapshot(layout);
    assert_events(
        &output,
        &[
            (KEY, EAST, 1),
            (KEY, THUMBR, 1),
            (ABS, RX, 3810),
            (ABS, RY, 89),
        ],
    );
    assert!(
        !output
            .iter()
            .any(|&(ty, c, _)| ty == ABS && [X, Y, HAT_X, HAT_Y].contains(&c))
    );

    for &event in &frame[..3] {
        decoder.event(RIGHT, event);
    }
    decoder.event(RIGHT, (0, 3, 0));
    assert_eq!(decoder.event(RIGHT, SYN), None);
    // Missing RX cannot release a previously held state.
    for &event in &frame {
        if event.1 != RX || event.0 != ABS {
            decoder.event(RIGHT, event);
        }
    }
    assert_eq!(decoder.event(RIGHT, SYN), None);
    frame.clear();
    upright(&mut frame, RIGHT, Sample::default());
    for &event in &frame {
        decoder.event(RIGHT, event);
    }
    seat.update(RIGHT, decoder.event(RIGHT, SYN).unwrap());
    assert_events(
        &seat.snapshot(layout),
        &[(KEY, EAST, 0), (KEY, THUMBR, 0), (ABS, RX, CENTER)],
    );
}

#[test]
fn cross_half_chord_requires_overlap_and_accepts_any_shoulder_in_either_order() {
    for l in [L, ZL] {
        for r in [R, ZR] {
            for right_first in [false, true] {
                let mut s = Seat::default();
                let (a, ka, b, kb) = if right_first {
                    (RIGHT, r, LEFT, l)
                } else {
                    (LEFT, l, RIGHT, r)
                };
                s.update(a, sample(&[ka]));
                s.update(a, sample(&[]));
                s.update(b, sample(&[kb]));
                assert_eq!(s.pads.len(), 2);
                assert_eq!(s.pads[&Layout::Upright(a)], 1);
                assert_eq!(s.pads[&Layout::Upright(b)], 2);
                s.update(a, sample(&[ka]));
                assert_eq!(s.pads, BTreeMap::from([(PAIR, 1)]));
                assert_events(&s.snapshot(PAIR), &[(KEY, l, 0), (KEY, r, 0)]);
                // Held selection buttons become usable after independent releases.
                s.update(a, sample(&[]));
                s.update(a, sample(&[ka]));
                assert_events(&s.snapshot(PAIR), &[(KEY, ka, 1), (KEY, kb, 0)]);
            }
        }
    }
}

#[test]
fn combined_sticks_hat_and_face_buttons_have_independent_releases() {
    let mut s = Seat::default();
    s.update(LEFT, sample(&[L]));
    s.update(RIGHT, sample(&[R]));
    s.update(
        LEFT,
        Sample {
            stick: [900, 2100],
            hat: [-1, 1],
            ..sample(&[MINUS])
        },
    );
    s.update(
        RIGHT,
        Sample {
            stick: [3500, 1200],
            ..sample(&[EAST, THUMBR])
        },
    );
    assert_events(
        &s.snapshot(PAIR),
        &[
            (ABS, X, 900),
            (ABS, Y, 2100),
            (ABS, HAT_X, -1),
            (ABS, HAT_Y, 1),
            (ABS, RX, 3500),
            (ABS, RY, 1200),
            (KEY, MINUS, 1),
            (KEY, EAST, 1),
            (KEY, THUMBR, 1),
        ],
    );
    s.update(RIGHT, Sample::default());
    assert_events(
        &s.snapshot(PAIR),
        &[
            (ABS, RX, CENTER),
            (KEY, EAST, 0),
            (KEY, THUMBR, 0),
            (ABS, X, 900),
            (ABS, HAT_X, -1),
            (KEY, MINUS, 1),
        ],
    );
}

#[test]
fn either_halfs_rails_split_the_pair_and_release_rearms_join() {
    for (mini, sl, sr, shoulder, other, other_shoulder) in [
        (LEFT, LEFT_SL, LEFT_SR, L, RIGHT, R),
        (RIGHT, RIGHT_SL, RIGHT_SR, R, LEFT, L),
    ] {
        let mut s = Seat::default();
        s.update(LEFT, sample(&[L]));
        s.update(RIGHT, sample(&[R]));
        s.update(mini, sample(&[sl, shoulder]));
        assert_eq!(s.pads.len(), 1);
        let held = sample(&[sl, sr, shoulder]);
        s.update(mini, held);
        let split = BTreeMap::from([(Layout::Mini(mini), 1), (Layout::Upright(other), 2)]);
        assert_eq!(s.pads, split);
        s.update(mini, held);
        s.update(other, sample(&[other_shoulder]));
        assert_eq!(s.pads, split);
        assert_events(&s.snapshot(Layout::Mini(mini)), &[(KEY, L, 0), (KEY, R, 0)]);
        s.update(mini, sample(&[]));
        s.update(mini, sample(&[sl, sr]));
        // In mini mode SL/SR themselves are gameplay shoulders.
        assert_events(&s.snapshot(Layout::Mini(mini)), &[(KEY, L, 1), (KEY, R, 1)]);
        s.update(other, sample(&[]));
        s.update(other, sample(&[other_shoulder]));
        s.update(mini, sample(&[shoulder]));
        assert_eq!(s.pads, BTreeMap::from([(PAIR, 1)]));
    }
}

#[test]
fn sideways_axes_face_positions_shoulders_and_stick_click_follow_sdl() {
    let mut seat = Seat::default();
    seat.update(LEFT, sample(&[LEFT_SL, LEFT_SR]));
    seat.update(RIGHT, sample(&[RIGHT_SL, RIGHT_SR]));
    for (hat, face) in [
        ([-1, 0], SOUTH),
        ([0, 1], EAST),
        ([1, 0], NORTH),
        ([0, -1], WEST),
    ] {
        seat.update(
            LEFT,
            Sample {
                hat,
                stick: [800, 3100],
                ..sample(&[MINUS, CAPTURE, THUMBL, L, ZL])
            },
        );
        let result = seat.snapshot(Layout::Mini(LEFT));
        assert_events(
            &result,
            &[
                (ABS, X, 3100),
                (ABS, Y, 3295),
                (KEY, face, 1),
                (KEY, PLUS, 1),
                (KEY, HOME, 1),
                (KEY, THUMBL, 1),
                (KEY, LEFT_SL, 1),
                (KEY, LEFT_SR, 1),
            ],
        );
        assert_eq!(
            result
                .iter()
                .filter(|&&(ty, key, v)| ty == KEY
                    && [SOUTH, EAST, WEST, NORTH].contains(&key)
                    && v == 1)
                .count(),
            1
        );
    }
    for (src, dst) in [(EAST, SOUTH), (NORTH, EAST), (WEST, NORTH), (SOUTH, WEST)] {
        seat.update(
            RIGHT,
            Sample {
                stick: [900, 3000],
                ..sample(&[src, THUMBR, PLUS, HOME])
            },
        );
        let result = seat.snapshot(Layout::Mini(RIGHT));
        assert_events(
            &result,
            &[
                (ABS, X, 1095),
                (ABS, Y, 900),
                (KEY, dst, 1),
                (KEY, THUMBL, 1),
                (KEY, PLUS, 1),
                (KEY, HOME, 1),
            ],
        );
        assert_eq!(
            result
                .iter()
                .filter(|&&(ty, k, v)| ty == KEY
                    && [SOUTH, EAST, NORTH, WEST].contains(&k)
                    && v == 1)
                .count(),
            1
        );
        assert!(
            result
                .iter()
                .all(|&(ty, c, _)| ty != ABS || [X, Y].contains(&c))
        );
    }
    // Releasing the outer left shoulders allows the right mini's paddles to be tested independently.
    seat.update(LEFT, Sample::default());
    seat.update(RIGHT, sample(&[R, ZR]));
    assert_events(
        &seat.snapshot(Layout::Mini(RIGHT)),
        &[
            (KEY, RIGHT_SR, 1),
            (KEY, RIGHT_SL, 1),
            (ABS, X, CENTER),
            (ABS, Y, CENTER),
        ],
    );
    assert_eq!(
        [reverse(0), reverse(CENTER), reverse(MAX)],
        [MAX, CENTER, 0]
    );
}

#[test]
fn unplug_preserves_survivor_identity_and_reconnect_starts_clean() {
    for (survivor, other) in [(LEFT, RIGHT), (RIGHT, LEFT)] {
        let mut s = Seat::default();
        s.update(LEFT, sample(&[L, MINUS]));
        s.update(RIGHT, sample(&[R, EAST]));
        s.remove(other);
        assert_eq!(s.pads, BTreeMap::from([(Layout::Upright(survivor), 1)]));
        let key = if survivor == LEFT { MINUS } else { EAST };
        assert_events(&s.snapshot(Layout::Upright(survivor)), &[(KEY, key, 1)]);
        s.update(other, Sample::default());
        assert_eq!(s.pads[&Layout::Upright(other)], 2);
        assert_events(
            &s.snapshot(Layout::Upright(other)),
            &[(KEY, if other == RIGHT { EAST } else { MINUS }, 0)],
        );
        s.remove(survivor);
        assert_eq!(s.pads[&Layout::Upright(other)], 2);
        s.remove(other);
        assert!(s.pads.is_empty());
    }
}

#[test]
fn independent_halves_get_first_free_player_and_never_pair_same_side() {
    let mut s = Seat::default();
    for i in 0..4 {
        let id = Id::Right(i);
        s.update(id, sample(&[R, ZR]));
        assert_eq!(s.pads[&Layout::Upright(id)], i + 1);
    }
    s.remove(Id::Right(1));
    s.update(LEFT, Sample::default());
    assert_eq!(s.pads[&Layout::Upright(LEFT)], 2);
    s.update(LEFT, sample(&[L]));
    assert_eq!(s.pads[&Layout::Pair(LEFT, Id::Right(0))], 1);
    assert_eq!(s.pads[&Layout::Upright(Id::Right(2))], 3);
    assert_eq!(s.pads[&Layout::Upright(Id::Right(3))], 4);
}
