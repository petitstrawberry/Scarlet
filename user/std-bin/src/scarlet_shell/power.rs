//! Pure presentation policy for real power-supply observations.
use alloc::string::String;
use scarlet_abi::power_supply::{ChargeState, PowerSupplySnapshot, SupplyKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerIcon {
    Empty,
    One,
    Two,
    Three,
    Four,
    Charging,
    Plugged,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerStatus {
    pub battery_detected: bool,
    pub percent: Option<u8>,
    pub charge_state: Option<ChargeState>,
    pub external_power: Option<bool>,
}

impl PowerStatus {
    /// Select the first present battery by stable supply ID. Capacities from
    /// multiple batteries cannot be averaged without their energy capacities.
    pub fn from_supplies(supplies: &[PowerSupplySnapshot]) -> Self {
        let battery = supplies
            .iter()
            .find(|s| s.kind == SupplyKind::Battery && s.state.present != Some(false));
        let mut state = Self::default();
        if let Some(battery) = battery {
            state.battery_detected = true;
            if !battery.read_failed {
                state.percent = battery
                    .state
                    .capacity_permille
                    .map(|v| ((v.min(1000) + 5) / 10) as u8);
                state.charge_state = battery.state.charge_state;
            }
        }
        let mut any = false;
        let mut all_offline = true;
        for source in supplies.iter().filter(|s| {
            matches!(
                s.kind,
                SupplyKind::Mains | SupplyKind::Usb | SupplyKind::Ups
            )
        }) {
            any = true;
            let online = if source.read_failed {
                None
            } else {
                source.state.online
            };
            if online == Some(true) {
                state.external_power = Some(true);
                break;
            }
            all_offline &= online == Some(false);
        }
        if state.external_power.is_none() && any && all_offline {
            state.external_power = Some(false);
        }
        state
    }

    /// Forget failed observations while keeping a known battery visible.
    pub fn unavailable(self) -> Self {
        Self {
            battery_detected: self.battery_detected,
            ..Default::default()
        }
    }

    pub fn icon(self) -> Option<PowerIcon> {
        if self.battery_detected
            && self.charge_state == Some(ChargeState::Charging)
            && self.external_power != Some(false)
        {
            return Some(PowerIcon::Charging);
        }
        if self.external_power == Some(true) {
            return Some(PowerIcon::Plugged);
        }
        if !self.battery_detected {
            return None;
        }
        Some(match self.percent {
            None => PowerIcon::Unknown,
            Some(0..=10) => PowerIcon::Empty,
            Some(11..=25) => PowerIcon::One,
            Some(26..=50) => PowerIcon::Two,
            Some(51..=75) => PowerIcon::Three,
            Some(_) => PowerIcon::Four,
        })
    }

    /// Numeric detail belongs in Control Center; the bar renders only icon().
    pub fn detail(self) -> String {
        let source = match (self.charge_state, self.external_power) {
            (Some(ChargeState::Charging), power) if power != Some(false) => "Charging",
            (Some(ChargeState::Full), Some(true)) => "Fully charged",
            (Some(ChargeState::Discharging), Some(true)) => "External + battery",
            (_, Some(true)) => "External power",
            (_, Some(false)) if self.battery_detected => "On battery",
            _ => "Power unknown",
        };
        if self.battery_detected {
            match self.percent {
                Some(percent) => alloc::format!("Battery {}%\n{}", percent, source),
                None => alloc::format!("Battery unavailable\n{}", source),
            }
        } else {
            String::from(source)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scarlet_abi::power_supply::PowerSupplyState;
    fn battery(percent: Option<u32>) -> PowerSupplySnapshot {
        PowerSupplySnapshot {
            kind: SupplyKind::Battery,
            state: PowerSupplyState {
                present: Some(true),
                capacity_permille: percent,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    fn source(online: Option<bool>) -> PowerSupplySnapshot {
        PowerSupplySnapshot {
            kind: SupplyKind::Usb,
            state: PowerSupplyState {
                online,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    #[test]
    fn bar_levels_do_not_turn_missing_data_into_an_empty_battery() {
        assert_eq!(PowerStatus::default().icon(), None);
        for (percent, icon) in [
            (None, PowerIcon::Unknown),
            (Some(0), PowerIcon::Empty),
            (Some(200), PowerIcon::One),
            (Some(400), PowerIcon::Two),
            (Some(700), PowerIcon::Three),
            (Some(1000), PowerIcon::Four),
        ] {
            assert_eq!(
                PowerStatus::from_supplies(&[battery(percent)]).icon(),
                Some(icon)
            );
        }
        let previous = PowerStatus::from_supplies(&[battery(Some(500))]);
        assert_eq!(previous.unavailable().icon(), Some(PowerIcon::Unknown));
        assert_eq!(previous.unavailable().percent, None);
    }
    #[test]
    fn charging_and_plugged_icons_follow_distinct_real_states() {
        let mut sample = battery(Some(550));
        sample.state.charge_state = Some(ChargeState::Charging);
        let charging = PowerStatus::from_supplies(&[sample, source(Some(true))]);
        assert_eq!(charging.icon(), Some(PowerIcon::Charging));
        assert_eq!(charging.detail(), "Battery 55%\nCharging");
        for charge in [
            ChargeState::Full,
            ChargeState::NotCharging,
            ChargeState::Discharging,
        ] {
            sample.state.charge_state = Some(charge);
            assert_eq!(
                PowerStatus::from_supplies(&[sample, source(Some(true))]).icon(),
                Some(PowerIcon::Plugged)
            );
        }
        sample.state.charge_state = Some(ChargeState::Charging);
        assert_eq!(
            PowerStatus::from_supplies(&[sample, source(Some(false))]).icon(),
            Some(PowerIcon::Three)
        );
    }
    #[test]
    fn unknown_external_sources_do_not_report_disconnected() {
        let state = PowerStatus::from_supplies(&[battery(None), source(Some(false)), source(None)]);
        assert_eq!(state.external_power, None);
        assert_eq!(
            PowerStatus::from_supplies(&[source(None), source(Some(true))]).external_power,
            Some(true)
        );
        let failed = PowerSupplySnapshot {
            read_failed: true,
            ..battery(Some(500))
        };
        assert_eq!(PowerStatus::from_supplies(&[failed]).percent, None);
        let absent = PowerSupplySnapshot {
            state: PowerSupplyState {
                present: Some(false),
                ..Default::default()
            },
            ..battery(Some(400))
        };
        assert_eq!(
            PowerStatus::from_supplies(&[absent, battery(Some(900))]).percent,
            Some(90)
        );
    }
}
