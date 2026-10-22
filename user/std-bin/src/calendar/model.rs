//! Bounded Gregorian date arithmetic for the desktop month calendar.

pub const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
pub const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Date {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl Default for Date {
    fn default() -> Self {
        Self {
            year: 1970,
            month: 1,
            day: 1,
        }
    }
}

pub fn leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

pub fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl Date {
    pub fn new(year: i32, month: u8, day: u8) -> Option<Self> {
        if !(1..=9999).contains(&year)
            || !(1..=12).contains(&month)
            || day == 0
            || day > days_in_month(year, month)
        {
            return None;
        }
        Some(Self { year, month, day })
    }

    pub fn from_unix_seconds(seconds: i64, utc_offset: i64) -> Option<Self> {
        Self::from_day_number(seconds.checked_add(utc_offset)?.div_euclid(86400))
    }

    /// Days since 1970-01-01, using complete Gregorian 400-year eras.
    pub fn day_number(self) -> i64 {
        let year = i64::from(self.year) - i64::from(self.month <= 2);
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let march_month = i64::from(self.month) + if self.month > 2 { -3 } else { 9 };
        let day_of_year = (153 * march_month + 2) / 5 + i64::from(self.day) - 1;
        era * 146097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year
            - 719468
    }

    pub fn from_day_number(day: i64) -> Option<Self> {
        // These bounds also keep all following arithmetic far from overflow.
        if !(-719162..=2932896).contains(&day) {
            return None;
        }
        let z = day + 719468;
        let era = z.div_euclid(146097);
        let day_of_era = z - era * 146097;
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
        let year = year_of_era + era * 400;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let march_month = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * march_month + 2) / 5 + 1;
        let month = march_month + if march_month < 10 { 3 } else { -9 };
        Self::new(
            (year + i64::from(month <= 2)) as i32,
            month as u8,
            day as u8,
        )
    }

    /// Monday is zero, Sunday is six, including dates before the Unix epoch.
    pub fn weekday(self) -> usize {
        (self.day_number() + 3).rem_euclid(7) as usize
    }

    pub fn shift_days(self, delta: i64) -> Self {
        Self::from_day_number(
            self.day_number()
                .saturating_add(delta)
                .clamp(-719162, 2932896),
        )
        .unwrap()
    }

    /// Preserve the selected day where possible, otherwise clamp to month end.
    pub fn shift_months(self, delta: i32) -> Self {
        let month_index = (i64::from(self.year) - 1) * 12 + i64::from(self.month) - 1;
        let month_index = (month_index + i64::from(delta)).clamp(0, 9999 * 12 - 1);
        let year = (month_index / 12 + 1) as i32;
        let month = (month_index % 12 + 1) as u8;
        Self {
            year,
            month,
            day: self.day.min(days_in_month(year, month)),
        }
    }

    pub fn month_cells(self) -> [Option<Self>; 42] {
        let mut cells = [None; 42];
        let offset = Self { day: 1, ..self }.weekday();
        for day in 1..=days_in_month(self.year, self.month) {
            cells[offset + usize::from(day) - 1] = Some(Self { day, ..self });
        }
        cells
    }

    pub fn month_title(self) -> String {
        format!("{} {}", MONTHS[usize::from(self.month) - 1], self.year)
    }

    pub fn label(self) -> String {
        format!(
            "{}, {} {} {}",
            WEEKDAYS[self.weekday()],
            MONTHS[usize::from(self.month) - 1],
            self.day,
            self.year
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn d(year: i32, month: u8, day: u8) -> Date {
        Date::new(year, month, day).unwrap()
    }

    #[test]
    fn leap_centuries_and_invalid_dates() {
        assert!(!leap_year(1900));
        assert!(leap_year(2000));
        assert!(!leap_year(2100));
        assert!(leap_year(2024));
        for (year, month, day) in [
            (0, 1, 1),
            (10000, 1, 1),
            (2026, 0, 1),
            (2026, 13, 1),
            (2026, 1, 0),
            (1900, 2, 29),
            (2026, 4, 31),
        ] {
            assert_eq!(Date::new(year, month, day), None);
        }
        assert_eq!(days_in_month(2000, 2), 29);
    }

    #[test]
    fn unix_epoch_negative_offsets_and_weekdays() {
        assert_eq!(d(1970, 1, 1).day_number(), 0);
        assert_eq!(d(1970, 1, 1).weekday(), 3);
        assert_eq!(Date::from_unix_seconds(0, -1), Some(d(1969, 12, 31)));
        assert_eq!(Date::from_unix_seconds(86399, 1), Some(d(1970, 1, 2)));
        assert_eq!(Date::from_unix_seconds(i64::MAX, 1), None);
        assert_eq!(d(2026, 10, 22).weekday(), 3);
    }

    #[test]
    fn navigation_crosses_years_and_clamps_month_ends() {
        assert_eq!(d(2024, 1, 31).shift_months(1), d(2024, 2, 29));
        assert_eq!(d(2023, 1, 31).shift_months(1), d(2023, 2, 28));
        assert_eq!(d(2024, 2, 29).shift_months(12), d(2025, 2, 28));
        assert_eq!(d(2025, 1, 1).shift_days(-1), d(2024, 12, 31));
        assert_eq!(d(2024, 12, 31).shift_days(1), d(2025, 1, 1));
        assert_eq!(d(2024, 12, 31).shift_months(1), d(2025, 1, 31));
    }

    #[test]
    fn supported_date_boundaries_are_bounded() {
        let first = d(1, 1, 1);
        let last = d(9999, 12, 31);
        assert_eq!(first.shift_days(i64::MIN), first);
        assert_eq!(last.shift_days(i64::MAX), last);
        assert_eq!(first.shift_months(i32::MIN), first);
        assert_eq!(last.shift_months(i32::MAX), last);
        assert_eq!(Date::from_day_number(first.day_number() - 1), None);
        assert_eq!(Date::from_day_number(last.day_number() + 1), None);
    }

    #[test]
    fn every_day_in_a_gregorian_cycle_roundtrips() {
        for day in d(2000, 1, 1).day_number()..d(2400, 1, 1).day_number() {
            assert_eq!(Date::from_day_number(day).unwrap().day_number(), day);
        }
        assert_eq!(Date::from_day_number(-719162), Some(d(1, 1, 1)));
        assert_eq!(Date::from_day_number(2932896), Some(d(9999, 12, 31)));
    }

    #[test]
    fn month_grid_aligns_weekdays_and_includes_six_week_months() {
        let cells = d(2026, 3, 1).month_cells(); // Sunday start, 31 days.
        assert_eq!(cells.iter().flatten().count(), 31);
        assert_eq!(cells[6], Some(d(2026, 3, 1)));
        assert_eq!(cells[36], Some(d(2026, 3, 31)));
        for (index, date) in cells
            .iter()
            .enumerate()
            .filter_map(|(i, d)| d.map(|d| (i, d)))
        {
            assert_eq!(index % 7, date.weekday());
        }
        assert_eq!(d(2024, 2, 1).month_cells().iter().flatten().count(), 29);
    }
}
