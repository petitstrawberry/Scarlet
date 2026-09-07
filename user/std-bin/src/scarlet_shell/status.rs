//! Data-only status-area model for the Scarlet desktop StatusBar.
//!
//! This module deliberately keeps status collection separate from ScarletUI
//! rendering. Callers obtain real kernel and SAS snapshots, then pass them to
//! [`StatusProvider`] to produce a normalized, ordered model suitable for
//! compact laptop or touch-first shell-bar presentations.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use sas_protocol::{CONTROL_FLAG_MUTED, ControlState, MASTER_VOLUME_UNITY_Q16};
use scarlet_desktop_config::{ClockFormat, StatusItemId, StatusPreferences};
use scarlet_os::scheduler::CpuUsageInfo;

/// Selects the text treatment used for a status item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusPresentation {
    /// Short labels for a pointer-oriented, space-constrained StatusBar.
    Compact,
    /// Explicit labels for a touch-first StatusBar.
    Touch,
}

/// A real CPU utilization sample suitable for status-area presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuStatus {
    /// A utilization percentage could not yet be calculated from two samples.
    Unavailable,
    /// Rounded aggregate CPU utilization, capped at 100 percent.
    Utilization(u8),
}

impl CpuStatus {
    /// Returns the CPU percentage when a valid delta sample is available.
    ///
    /// # Returns
    ///
    /// The rounded utilization percentage, or `None` before a valid delta is
    /// available.
    pub const fn percent(self) -> Option<u8> {
        match self {
            Self::Unavailable => None,
            Self::Utilization(percent) => Some(percent),
        }
    }

    /// Formats this status for the selected StatusBar presentation.
    ///
    /// # Arguments
    ///
    /// * `presentation` - The StatusBar presentation that will render the label.
    ///
    /// # Returns
    ///
    /// A concise label derived only from the sampled CPU status.
    pub fn label(self, presentation: StatusPresentation) -> String {
        match (self, presentation) {
            (Self::Unavailable, StatusPresentation::Compact) => String::from("CPU —"),
            (Self::Unavailable, StatusPresentation::Touch) => String::from("CPU unavailable"),
            (Self::Utilization(percent), StatusPresentation::Compact) => {
                percent_label("CPU ", percent)
            }
            (Self::Utilization(percent), StatusPresentation::Touch) => {
                percent_label("CPU ", percent)
            }
        }
    }
}

/// A real SAS master-volume status suitable for status-area presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioStatus {
    /// SAS did not provide a current control state.
    Unavailable,
    /// The real SAS control state reports that master output is muted.
    Muted,
    /// The real SAS control state reports an unmuted master volume percentage.
    Volume(u8),
}

impl AudioStatus {
    /// Formats this status for the selected StatusBar presentation.
    ///
    /// # Arguments
    ///
    /// * `presentation` - The StatusBar presentation that will render the label.
    ///
    /// # Returns
    ///
    /// A concise label derived only from the real SAS state.
    pub fn label(self, presentation: StatusPresentation) -> String {
        match (self, presentation) {
            (Self::Unavailable, StatusPresentation::Compact) => String::from("Audio —"),
            (Self::Unavailable, StatusPresentation::Touch) => String::from("Audio unavailable"),
            (Self::Muted, StatusPresentation::Compact) => String::from("Muted"),
            (Self::Muted, StatusPresentation::Touch) => String::from("Audio muted"),
            (Self::Volume(percent), StatusPresentation::Compact) => percent_label("Vol ", percent),
            (Self::Volume(percent), StatusPresentation::Touch) => percent_label("Volume ", percent),
        }
    }
}

/// A displayable optional status item and its current real-data snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusDescriptor {
    /// Stable optional-item identifier used by the preferences model.
    pub id: StatusItemId,
    /// Current value for the item.
    pub snapshot: StatusItemSnapshot,
    /// Label preformatted for the requested StatusBar presentation.
    pub label: String,
}

/// The current value associated with an optional status item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusItemSnapshot {
    /// Aggregate CPU utilization derived from kernel accounting deltas.
    Cpu(CpuStatus),
    /// Master audio state derived from the SAS control state.
    Audio(AudioStatus),
}

impl StatusItemSnapshot {
    /// Formats this snapshot for the selected StatusBar presentation.
    ///
    /// # Arguments
    ///
    /// * `presentation` - The StatusBar presentation that will render the label.
    ///
    /// # Returns
    ///
    /// A presentation-specific label for this snapshot.
    pub fn label(self, presentation: StatusPresentation) -> String {
        match self {
            Self::Cpu(status) => status.label(presentation),
            Self::Audio(status) => status.label(presentation),
        }
    }
}

/// Shared real-data state for StatusBar and Control Center status presentations.
///
/// This model contains only real observations. A missing kernel or SAS
/// observation remains `None`; callers must not substitute synthetic values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusProviderSnapshot {
    /// Normalized optional-item preferences and the fixed clock format.
    pub preferences: StatusPreferences,
    /// Latest delta-derived aggregate CPU percentage, when available.
    pub cpu_percent: Option<u8>,
    /// Latest real SAS master-volume percentage, when available.
    pub audio_volume_percent: Option<u8>,
    /// Latest real SAS master mute state, when available.
    pub audio_muted: Option<bool>,
}

impl StatusProviderSnapshot {
    /// Returns visible optional items in normalized preference order.
    ///
    /// # Arguments
    ///
    /// * `presentation` - The target compact or touch status presentation.
    ///
    /// # Returns
    ///
    /// CPU/audio descriptors with labels preformatted for `presentation`. The
    /// clock is deliberately excluded because renderers pin it far-right.
    pub fn visible_items(&self, presentation: StatusPresentation) -> Vec<StatusDescriptor> {
        visible_status_descriptors(
            &self.preferences,
            self.cpu_status(),
            self.audio_status(),
            presentation,
        )
    }

    /// Formats the independent fixed trailing clock for a local time.
    ///
    /// # Arguments
    ///
    /// * `hour` - Local wall-clock hour.
    /// * `minute` - Local wall-clock minute.
    ///
    /// # Returns
    ///
    /// A label using the clock format in [`Self::preferences`].
    pub fn clock_label(&self, hour: u8, minute: u8) -> String {
        format_clock(hour, minute, self.preferences.clock_format)
    }

    fn cpu_status(&self) -> CpuStatus {
        self.cpu_percent
            .map(CpuStatus::Utilization)
            .unwrap_or(CpuStatus::Unavailable)
    }

    fn audio_status(&self) -> AudioStatus {
        match (self.audio_volume_percent, self.audio_muted) {
            (_, Some(true)) => AudioStatus::Muted,
            (Some(percent), Some(false)) => AudioStatus::Volume(percent),
            _ => AudioStatus::Unavailable,
        }
    }
}

/// Samples cumulative CPU accounting and constructs StatusBar status snapshots.
#[derive(Clone, Debug, Default)]
pub struct StatusProvider {
    cpu_sampler: CpuUsageSampler,
}

impl StatusProvider {
    /// Creates a provider with no prior CPU sample.
    ///
    /// # Returns
    ///
    /// A provider whose first CPU observation is unavailable until a later
    /// cumulative counter sample arrives.
    pub const fn new() -> Self {
        Self {
            cpu_sampler: CpuUsageSampler::new(),
        }
    }

    /// Builds shared status state from real kernel and SAS observations.
    ///
    /// # Arguments
    ///
    /// * `preferences` - User preferences for optional-item order and visibility.
    /// * `cpu_usage` - Current kernel CPU accounting observation, if available.
    /// * `audio_state` - Current SAS master control state, if available.
    /// # Returns
    ///
    /// Cloneable shared state for a [`scarlet_ui::State`] that can feed both
    /// the StatusBar and Control Center without a second backend.
    pub fn snapshot(
        &mut self,
        preferences: &StatusPreferences,
        cpu_usage: Option<CpuUsageInfo>,
        audio_state: Option<ControlState>,
    ) -> StatusProviderSnapshot {
        let (audio_volume_percent, audio_muted) = audio_observation(audio_state);
        StatusProviderSnapshot {
            preferences: preferences.clone().normalize(),
            cpu_percent: self.cpu_sampler.sample(cpu_usage).percent(),
            audio_volume_percent,
            audio_muted,
        }
    }
}

/// Returns visible optional descriptors in normalized user-preference order.
///
/// # Arguments
///
/// * `preferences` - User preferences for optional-item order and visibility.
/// * `cpu` - Current kernel-derived CPU status.
/// * `audio` - Current SAS-derived audio status.
///
/// # Returns
///
/// The CPU/audio descriptors enabled by preferences. The clock is deliberately
/// excluded because it is fixed and rendered separately at the far-right edge.
pub fn visible_status_descriptors(
    preferences: &StatusPreferences,
    cpu: CpuStatus,
    audio: AudioStatus,
    presentation: StatusPresentation,
) -> Vec<StatusDescriptor> {
    let preferences = preferences.clone().normalize();
    let mut descriptors = Vec::new();
    for id in preferences.order.iter().copied() {
        if !preferences.is_visible(id) {
            continue;
        }
        let snapshot = match id {
            StatusItemId::Cpu => StatusItemSnapshot::Cpu(cpu),
            StatusItemId::Audio => StatusItemSnapshot::Audio(audio),
        };
        descriptors.push(StatusDescriptor {
            id,
            label: snapshot.label(presentation),
            snapshot,
        });
    }
    descriptors
}

/// Formats a local time using a configured 12-hour or 24-hour format.
///
/// # Arguments
///
/// * `hour` - Local hour. Values outside 0 through 23 are reduced modulo 24.
/// * `minute` - Local minute. Values outside 0 through 59 are reduced modulo 60.
/// * `format` - The configured clock presentation.
///
/// # Returns
///
/// A compact clock label. The clock is intentionally independent of optional
/// status-item visibility and ordering.
pub fn format_clock(hour: u8, minute: u8, format: ClockFormat) -> String {
    let hour = hour % 24;
    let minute = minute % 60;
    let mut label = String::new();
    match format {
        ClockFormat::TwentyFourHour => {
            let _ = write!(label, "{:02}:{:02}", hour, minute);
        }
        ClockFormat::TwelveHour => {
            let meridiem = if hour < 12 { "AM" } else { "PM" };
            let hour = match hour % 12 {
                0 => 12,
                value => value,
            };
            let _ = write!(label, "{}:{:02} {}", hour, minute, meridiem);
        }
    }
    label
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CpuCounters {
    busy_time_ns: u64,
    idle_time_ns: u64,
}

impl From<CpuUsageInfo> for CpuCounters {
    fn from(usage: CpuUsageInfo) -> Self {
        Self {
            busy_time_ns: usage.busy_time_ns(),
            idle_time_ns: usage.idle_time_ns(),
        }
    }
}

/// Delta sampler for the kernel's cumulative busy and idle counters.
#[derive(Clone, Debug, Default)]
pub struct CpuUsageSampler {
    previous: Option<CpuCounters>,
}

impl CpuUsageSampler {
    /// Creates a sampler with no prior cumulative CPU accounting observation.
    ///
    /// # Returns
    ///
    /// A sampler whose first observation is unavailable by design.
    pub const fn new() -> Self {
        Self { previous: None }
    }

    /// Samples the current cumulative kernel CPU counters.
    ///
    /// # Arguments
    ///
    /// * `current` - Current kernel accounting observation, if available.
    ///
    /// # Returns
    ///
    /// A rounded utilization percentage based on busy/idle deltas. The first,
    /// unavailable, reset, and zero-delta observations return unavailable.
    pub fn sample(&mut self, current: Option<CpuUsageInfo>) -> CpuStatus {
        self.sample_counters(current.map(CpuCounters::from))
    }

    fn sample_counters(&mut self, current: Option<CpuCounters>) -> CpuStatus {
        let Some(current) = current else {
            self.previous = None;
            return CpuStatus::Unavailable;
        };
        let Some(previous) = self.previous.replace(current) else {
            return CpuStatus::Unavailable;
        };

        if current.busy_time_ns < previous.busy_time_ns
            || current.idle_time_ns < previous.idle_time_ns
        {
            return CpuStatus::Unavailable;
        }

        let busy_delta = current.busy_time_ns - previous.busy_time_ns;
        let idle_delta = current.idle_time_ns - previous.idle_time_ns;
        let total_delta = busy_delta.saturating_add(idle_delta);
        if total_delta == 0 {
            return CpuStatus::Unavailable;
        }

        let percent = (((busy_delta as u128 * 100) + (total_delta as u128 / 2))
            / total_delta as u128)
            .min(100) as u8;
        CpuStatus::Utilization(percent)
    }
}

fn percent_label(prefix: &str, percent: u8) -> String {
    let mut label = String::from(prefix);
    let _ = write!(label, "{}%", percent);
    label
}

fn q16_to_percent(volume_q16: u32) -> u8 {
    ((volume_q16 as u64 * 100 + (MASTER_VOLUME_UNITY_Q16 / 2) as u64)
        / MASTER_VOLUME_UNITY_Q16 as u64)
        .min(100) as u8
}

fn audio_observation(state: Option<ControlState>) -> (Option<u8>, Option<bool>) {
    match state {
        Some(state) => (
            Some(q16_to_percent(state.master_volume_q16)),
            Some(state.flags & CONTROL_FLAG_MUTED != 0),
        ),
        None => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AudioStatus, CpuCounters, CpuStatus, CpuUsageSampler, StatusItemSnapshot,
        StatusPresentation, StatusProviderSnapshot, audio_observation, format_clock,
        visible_status_descriptors,
    };
    use alloc::vec;
    use sas_protocol::{CONTROL_FLAG_MUTED, ControlState, MASTER_VOLUME_UNITY_Q16};
    use scarlet_desktop_config::{ClockFormat, StatusItemId, StatusPreferences};

    fn counters(busy_time_ns: u64, idle_time_ns: u64) -> CpuCounters {
        CpuCounters {
            busy_time_ns,
            idle_time_ns,
        }
    }

    #[test]
    fn cpu_sampler_requires_two_valid_cumulative_observations() {
        let mut sampler = CpuUsageSampler::new();
        assert_eq!(
            sampler.sample_counters(Some(counters(50, 50))),
            CpuStatus::Unavailable
        );
        assert_eq!(
            sampler.sample_counters(Some(counters(110, 90))),
            CpuStatus::Utilization(60)
        );
    }

    #[test]
    fn cpu_sampler_marks_unavailable_reset_and_zero_delta_as_unavailable() {
        let mut sampler = CpuUsageSampler::new();
        assert_eq!(sampler.sample_counters(None), CpuStatus::Unavailable);
        assert_eq!(
            sampler.sample_counters(Some(counters(10, 90))),
            CpuStatus::Unavailable
        );
        assert_eq!(
            sampler.sample_counters(Some(counters(10, 90))),
            CpuStatus::Unavailable
        );
        assert_eq!(
            sampler.sample_counters(Some(counters(5, 95))),
            CpuStatus::Unavailable
        );
        assert_eq!(sampler.sample_counters(None), CpuStatus::Unavailable);
        assert_eq!(
            sampler.sample_counters(Some(counters(20, 80))),
            CpuStatus::Unavailable
        );
    }

    #[test]
    fn cpu_sampler_rounds_and_caps_malformed_busy_deltas() {
        let mut sampler = CpuUsageSampler::new();
        assert_eq!(
            sampler.sample_counters(Some(counters(0, 0))),
            CpuStatus::Unavailable
        );
        assert_eq!(
            sampler.sample_counters(Some(counters(1, 2))),
            CpuStatus::Utilization(33)
        );
        assert_eq!(
            sampler.sample_counters(Some(counters(101, 2))),
            CpuStatus::Utilization(100)
        );
    }

    #[test]
    fn audio_status_uses_real_sas_mute_and_capped_volume() {
        let muted = ControlState::new(MASTER_VOLUME_UNITY_Q16, CONTROL_FLAG_MUTED, 0, "", "", "");
        assert_eq!(audio_observation(Some(muted)), (Some(100), Some(true)));

        let loud = ControlState::new(MASTER_VOLUME_UNITY_Q16 * 2, 0, 0, "", "", "");
        assert_eq!(audio_observation(Some(loud)), (Some(100), Some(false)));
        assert_eq!(audio_observation(None), (None, None));
    }

    #[test]
    fn labels_are_concise_on_compact_status_bars_and_explicit_for_touch() {
        assert_eq!(
            CpuStatus::Utilization(42).label(StatusPresentation::Compact),
            "CPU 42%"
        );
        assert_eq!(
            AudioStatus::Volume(42).label(StatusPresentation::Compact),
            "Vol 42%"
        );
        assert_eq!(
            AudioStatus::Muted.label(StatusPresentation::Touch),
            "Audio muted"
        );
        assert_eq!(
            AudioStatus::Unavailable.label(StatusPresentation::Compact),
            "Audio —"
        );
    }

    #[test]
    fn clock_formatter_handles_midnight_noon_and_normalizes_inputs() {
        assert_eq!(format_clock(0, 5, ClockFormat::TwentyFourHour), "00:05");
        assert_eq!(format_clock(0, 5, ClockFormat::TwelveHour), "12:05 AM");
        assert_eq!(format_clock(12, 0, ClockFormat::TwelveHour), "12:00 PM");
        assert_eq!(format_clock(25, 61, ClockFormat::TwelveHour), "1:01 AM");
    }

    #[test]
    fn descriptors_follow_normalized_order_and_exclude_hidden_items() {
        let preferences = StatusPreferences {
            order: vec![StatusItemId::Audio, StatusItemId::Audio],
            visible: vec![StatusItemId::Audio],
            clock_format: ClockFormat::TwentyFourHour,
        };
        let descriptors = visible_status_descriptors(
            &preferences,
            CpuStatus::Utilization(50),
            AudioStatus::Muted,
            StatusPresentation::Compact,
        );
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].id, StatusItemId::Audio);
        assert_eq!(
            descriptors[0].snapshot,
            StatusItemSnapshot::Audio(AudioStatus::Muted)
        );
        assert_eq!(descriptors[0].label, "Muted");
    }

    #[test]
    fn shared_snapshot_is_state_safe_and_formats_independent_clock() {
        let snapshot = StatusProviderSnapshot {
            preferences: StatusPreferences::default(),
            cpu_percent: Some(50),
            audio_volume_percent: Some(35),
            audio_muted: Some(false),
        };
        assert_eq!(snapshot.clone(), snapshot);
        assert_eq!(snapshot.clock_label(13, 7), "13:07");
        let items = snapshot.visible_items(StatusPresentation::Touch);
        assert_eq!(items[0].label, "CPU 50%");
        assert_eq!(items[1].label, "Volume 35%");
    }
}
