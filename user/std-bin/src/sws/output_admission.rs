//! Admission depends on an output's slot and the images it will actually read.
//! The caller retains these read sets until the corresponding GPU fence retires.
pub(super) fn can_admit<'a>(
    output: usize,
    readers: &[u32],
    flights: impl IntoIterator<Item = (usize, &'a [u32])>,
) -> bool {
    !flights.into_iter().any(|(pending_output, pending_readers)| {
        output == pending_output || readers.iter().any(|window| pending_readers.contains(window))
    })
}

#[cfg(test)]
mod tests {
    use super::can_admit;

    #[test]
    fn a_slow_output_does_not_gate_an_unrelated_output() {
        let slow = [20, 21];
        assert!(can_admit(0, &[10, 11], [(1, slow.as_slice())]));
        assert!(!can_admit(1, &[20, 21], [(1, slow.as_slice())]));
    }

    #[test]
    fn spanning_image_waits_for_its_last_reader() {
        let first = [10, 99];
        let second = [20, 99];
        assert!(!can_admit(2, &[99], [(0, first.as_slice()), (1, second.as_slice())]));
        // Output 0 has retired, but output 1 still samples image 99.
        assert!(!can_admit(0, &[99], [(1, second.as_slice())]));
        assert!(can_admit(0, &[10], [(1, second.as_slice())]));
        assert!(can_admit(0, &[99], []));
    }

    #[test]
    fn repeated_fast_frames_do_not_consume_the_slow_outputs_slot() {
        let slow = [20];
        for _ in 0..8 {
            assert!(can_admit(0, &[10], [(1, slow.as_slice())]));
            let fast = [10];
            assert!(!can_admit(0, &[10], [(1, slow.as_slice()), (0, fast.as_slice())]));
            // Retiring only the fast fence permits its next frame.
        }
    }
}
