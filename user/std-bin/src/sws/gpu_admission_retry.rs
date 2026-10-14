/// Admission retries have their own deadline: an unsuccessful submission is
/// neither a presented frame nor a reason to block processing input events.
#[derive(Default)]
pub(super) struct GpuAdmissionRetry {
    deadline_ns: Option<u64>,
}

impl GpuAdmissionRetry {
    pub(super) fn defer(&mut self, now_ns: u64, frame_interval_ns: u64) {
        self.deadline_ns = Some(now_ns.saturating_add(frame_interval_ns));
    }

    pub(super) fn remaining_ns(&self, now_ns: u64) -> Option<u64> {
        self.deadline_ns
            .and_then(|deadline| deadline.checked_sub(now_ns))
            .filter(|remaining| *remaining > 0)
    }

    pub(super) fn clear(&mut self) {
        self.deadline_ns = None;
    }
}

#[cfg(test)]
mod tests {
    use super::GpuAdmissionRetry;

    #[test]
    fn input_wakes_cannot_make_a_busy_frame_ready_before_its_retry_deadline() {
        let mut retry = GpuAdmissionRetry::default();
        assert_eq!(retry.remaining_ns(100), None);
        retry.defer(100, 16);
        // Each wake can process input, but consulting the gate never consumes
        // it. Only reaching its deadline permits another GPU submission.
        for now in 100..116 {
            assert_eq!(retry.remaining_ns(now), Some(116 - now));
        }
        assert_eq!(retry.remaining_ns(116), None);
        assert_eq!(retry.remaining_ns(117), None);
    }

    #[test]
    fn repeated_busy_submission_starts_another_bounded_wait() {
        let mut retry = GpuAdmissionRetry::default();
        retry.defer(100, 16);
        assert_eq!(retry.remaining_ns(116), None);
        retry.defer(116, 16);
        assert_eq!(retry.remaining_ns(116), Some(16));
        assert_eq!(retry.remaining_ns(131), Some(1));
        assert_eq!(retry.remaining_ns(132), None);
    }

    #[test]
    fn success_or_backend_removal_clears_the_retry_deadline() {
        let mut retry = GpuAdmissionRetry::default();
        retry.defer(100, 16);
        retry.clear();
        assert_eq!(retry.remaining_ns(101), None);
    }

    #[test]
    fn clock_limit_cannot_wrap_a_retry_into_the_past() {
        let mut retry = GpuAdmissionRetry::default();
        retry.defer(u64::MAX - 4, 16);
        assert_eq!(retry.remaining_ns(u64::MAX - 4), Some(4));
        assert_eq!(retry.remaining_ns(u64::MAX), None);
    }
}
