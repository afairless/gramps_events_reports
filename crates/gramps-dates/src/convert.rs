//! Gregorian/Julian normalization via SDN (Serial Date Number) conversion.
//!
//! The algorithms are a direct port of Gramps 5.1.6 `gen/lib/gcalendar.py`
//! (`gregorian_sdn`/`gregorian_ymd`/`julian_sdn`/`julian_ymd`), which are
//! themselves the classic public-domain SDN formulas from Peter Meyer's
//! "Calendrical Calculations". Floor division semantics match Python's `//`
//! so negative (BC) years behave identically.
//!
//! Per decision D4 the other five Gramps calendars (Hebrew, French
//! Republican, Persian, Islamic, Swedish) are **not** converted in v1:
//! [`to_civil`] returns `None` for them and emits a one-time warning.
//!
//! The `jiff::civil::Date` type is the crate's civil-date representation
//! (proleptic Gregorian), so "Gregorian date" and "normalized date" are the
//! same thing here.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use jiff::civil::Date;

use crate::model::{Calendar, GrampsDate};

/// Constants from `gcalendar.py` (Gregorian).
const GRG_SDN_OFFSET: i64 = 32045;
const GRG_DAYS_PER_400_YEARS: i64 = 146097;
const GRG_DAYS_PER_4_YEARS: i64 = 1461;
const GRG_DAYS_PER_5_MONTHS: i64 = 153;
/// Constants from `gcalendar.py` (Julian).
const JLN_SDN_OFFSET: i64 = 32083;
const JLN_DAYS_PER_4_YEARS: i64 = 1461;
const JLN_DAYS_PER_5_MONTHS: i64 = 153;

/// Python-style floor division (`//`): rounds toward negative infinity.
fn div_floor(a: i64, b: i64) -> i64 {
    let q = a / b;
    let r = a % b;
    if r != 0 && ((r < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

/// Python-style modulo (`%`): always non-negative for a positive divisor.
fn mod_floor(a: i64, b: i64) -> i64 {
    let q = div_floor(a, b);
    a - q * b
}

/// Gregorian calendar date → SDN (port of `gregorian_sdn`).
pub(crate) fn gregorian_sdn(year: i32, month: u32, day: u32) -> i64 {
    let mut year = i64::from(year);
    let mut month = i64::from(month);
    year += if year < 0 { 4801 } else { 4800 };
    if month > 2 {
        month -= 3;
    } else {
        month += 9;
        year -= 1;
    }
    div_floor(div_floor(year, 100) * GRG_DAYS_PER_400_YEARS, 4)
        + div_floor(mod_floor(year, 100) * GRG_DAYS_PER_4_YEARS, 4)
        + div_floor(month * GRG_DAYS_PER_5_MONTHS + 2, 5)
        + i64::from(day)
        - GRG_SDN_OFFSET
}

/// SDN → Gregorian calendar date (port of `gregorian_ymd`).
pub(crate) fn gregorian_ymd(sdn: i64) -> (i32, u32, u32) {
    let mut temp = (GRG_SDN_OFFSET + sdn) * 4 - 1;
    let century = div_floor(temp, GRG_DAYS_PER_400_YEARS);
    temp = (mod_floor(temp, GRG_DAYS_PER_400_YEARS) / 4) * 4 + 3;
    let mut year = century * 100 + div_floor(temp, GRG_DAYS_PER_4_YEARS);
    let day_of_year = mod_floor(temp, GRG_DAYS_PER_4_YEARS) / 4 + 1;
    let temp = day_of_year * 5 - 3;
    let mut month = div_floor(temp, GRG_DAYS_PER_5_MONTHS);
    let day = mod_floor(temp, GRG_DAYS_PER_5_MONTHS) / 5 + 1;
    if month < 10 {
        month += 3;
    } else {
        year += 1;
        month -= 9;
    }
    year -= 4800;
    if year <= 0 {
        year -= 1;
    }
    (year as i32, month as u32, day as u32)
}

/// Julian calendar date → SDN (port of `julian_sdn`).
pub(crate) fn julian_sdn(year: i32, month: u32, day: u32) -> i64 {
    let mut year = i64::from(year);
    let mut month = i64::from(month);
    year += if year < 0 { 4801 } else { 4800 };
    if month > 2 {
        month -= 3;
    } else {
        month += 9;
        year -= 1;
    }
    div_floor(year * JLN_DAYS_PER_4_YEARS, 4)
        + div_floor(month * JLN_DAYS_PER_5_MONTHS + 2, 5)
        + i64::from(day)
        - JLN_SDN_OFFSET
}

/// SDN → Julian calendar date (port of `julian_ymd`).
pub(crate) fn julian_ymd(sdn: i64) -> (i32, u32, u32) {
    let temp = (sdn + JLN_SDN_OFFSET) * 4 - 1;
    let mut year = div_floor(temp, JLN_DAYS_PER_4_YEARS);
    let day_of_year = mod_floor(temp, JLN_DAYS_PER_4_YEARS) / 4 + 1;
    let temp = day_of_year * 5 - 3;
    let mut month = div_floor(temp, JLN_DAYS_PER_5_MONTHS);
    let day = mod_floor(temp, JLN_DAYS_PER_5_MONTHS) / 5 + 1;
    if month < 10 {
        month += 3;
    } else {
        year += 1;
        month -= 9;
    }
    year -= 4800;
    if year <= 0 {
        year -= 1;
    }
    (year as i32, month as u32, day as u32)
}

/// The calendar a stored `GrampsDate` actually behaves as.
///
/// Dual-dated (slash) dates are Julian — Gramps coerces the calendar in
/// `Date.set` when the slash flag is present.
pub(crate) fn effective_calendar(calendar: Calendar, dual_dated: bool) -> Calendar {
    if dual_dated {
        Calendar::Julian
    } else {
        calendar
    }
}

/// One-time warning that a calendar is not SDN-converted in v1 (decision D4).
///
/// Warnings are emitted at most once per calendar type per process; the set
/// is created lazily on first use (`OnceLock::get_or_init`).
static NON_GREGORIAN_WARNED: OnceLock<Mutex<HashSet<Calendar>>> = OnceLock::new();

fn warn_non_gregorian(calendar: Calendar) {
    let mut warned = NON_GREGORIAN_WARNED
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if warned.insert(calendar) {
        let calendar_name = calendar.as_str();
        eprintln!(
            "warning: Gramps {calendar_name} dates are not converted in v1 \
             (decision D4); keeping the stored text and excluding them from \
             anniversary math"
        );
    }
}

/// Normalize a full `(year, month, day)` triple in `calendar` to a proleptic
/// Gregorian civil date.
///
/// Returns `None` when the date cannot be normalized:
///
/// - the year, month or day is unknown (partial dates — `0` components),
/// - the calendar is one of the five non-converted calendars (D4; warns once),
/// - the triple is not a real date in that calendar (e.g. Feb 30), detected
///   by a round-trip through SDN.
pub(crate) fn to_civil(calendar: Calendar, ymd: (i32, u32, u32)) -> Option<Date> {
    let (year, month, day) = ymd;
    if year == 0 || month == 0 || day == 0 || month > 12 {
        return None;
    }
    let sdn = match calendar {
        Calendar::Gregorian => gregorian_sdn(year, month, day),
        Calendar::Julian => julian_sdn(year, month, day),
        other => {
            warn_non_gregorian(other);
            return None;
        }
    };
    // Round-trip validity guard: an invalid date (e.g. Feb 30, or a day
    // that does not exist in the month) would shift into the next month
    // under SDN, so a mismatch means the input was not a real date.
    let back = match calendar {
        Calendar::Gregorian => gregorian_ymd(sdn),
        Calendar::Julian => julian_ymd(sdn),
        Calendar::Hebrew
        | Calendar::FrenchRepublican
        | Calendar::Persian
        | Calendar::Islamic
        | Calendar::Swedish => unreachable!("filtered by the match above"),
    };
    if back != (year, month, day) {
        return None;
    }
    let (gy, gm, gd) = if calendar == Calendar::Julian {
        gregorian_ymd(sdn)
    } else {
        (year, month, day)
    };
    let gy = i16::try_from(gy).ok()?;
    let gm = i8::try_from(gm).ok()?;
    let gd = i8::try_from(gd).ok()?;
    Date::new(gy, gm, gd).ok()
}

impl GrampsDate {
    /// The normalized (proleptic Gregorian) start date of this date.
    ///
    /// - a single `dateval` → that date, if full and convertible;
    /// - a `daterange`/`datespan` → its start endpoint;
    /// - partial dates (month/day `0`), unknown years, and the five
    ///   non-converted calendars (D4) → `None`.
    pub fn start(&self) -> Option<Date> {
        let calendar = effective_calendar(self.calendar, self.dual_dated);
        to_civil(calendar, self.ymd)
    }

    /// The normalized (proleptic Gregorian) stop date of a `daterange`/
    /// `datespan`; `None` for single dates and non-convertible endpoints.
    pub fn stop(&self) -> Option<Date> {
        let calendar = effective_calendar(self.calendar, self.dual_dated);
        self.stop.and_then(|ymd| to_civil(calendar, ymd))
    }

    /// Convert the calendar to Gregorian, mirroring Gramps `gregorian()`.
    ///
    /// Equivalent to [`GrampsDate::start`] — the anniversary anchor always
    /// comes from the start endpoint — and `None` whenever the date is not
    /// Gregorian-normalizable (partial, unknown year, or a non-converted
    /// calendar per D4).
    pub fn to_gregorian(&self) -> Option<Date> {
        self.start()
    }

    /// The anniversary anchor `(month, day)` per the project plan §8 rules 1
    /// and 3:
    ///
    /// - full date (or range/span start) → `(month, day)`;
    /// - date with a month but no day → `(month, 1)`;
    /// - year-only dates (or range starts) → `None` (excluded from the
    ///   anniversary calendar);
    /// - Julian dates are anchored on their **Gregorian** month/day, so a
    ///   full Julian date is required; partial Julian dates → `None`;
    /// - non-converted calendars (D4) → `None` with a one-time warning.
    pub fn anniversary_key(&self) -> Option<(u32, u32)> {
        match effective_calendar(self.calendar, self.dual_dated) {
            Calendar::Gregorian => {
                if self.ymd.0 == 0 {
                    return None;
                }
                let month = self.ymd.1;
                if month == 0 {
                    return None;
                }
                let day = self.ymd.2;
                if day == 0 {
                    return Some((month, 1));
                }
                // Full date: must be a real date (Feb 30 would anchor to
                // March 2 otherwise).
                if gregorian_ymd(gregorian_sdn(self.ymd.0, month, day)) != self.ymd {
                    return None;
                }
                Some((month, day))
            }
            Calendar::Julian => {
                let date = self.start()?;
                Some((u32::from(date.month() as u8), u32::from(date.day() as u8)))
            }
            other => {
                warn_non_gregorian(other);
                None
            }
        }
    }

    /// True if this date is a single date that could hypothetically have
    /// been a compound one — kept for symmetry with `is_range`; text dates
    /// and plain datevals are indistinguishable from an anchor standpoint.
    #[cfg(test)]
    #[allow(dead_code)]
    fn _anchor_debug(&self) -> Option<(u32, u32)> {
        self.anniversary_key()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Modifier;
    use crate::model::{NewYear, Quality};

    fn date(
        ymd: (i32, u32, u32),
        modifier: Modifier,
        calendar: Calendar,
        dual_dated: bool,
        stop: Option<(i32, u32, u32)>,
    ) -> GrampsDate {
        GrampsDate {
            calendar,
            modifier,
            quality: Quality::None,
            ymd,
            stop,
            dual_dated,
            new_year: NewYear::Jan1,
            display: "".to_string(),
        }
    }

    fn greg(ymd: (i32, u32, u32)) -> GrampsDate {
        date(ymd, Modifier::None, Calendar::Gregorian, false, None)
    }

    fn julian(ymd: (i32, u32, u32), stop: Option<(i32, u32, u32)>) -> GrampsDate {
        date(
            ymd,
            if stop.is_some() {
                Modifier::Range
            } else {
                Modifier::None
            },
            Calendar::Julian,
            false,
            stop,
        )
    }

    // Golden SDN values from Gramps 5.1.6 `gcalendar.py`.
    #[test]
    fn sdn_values_match_gramps() {
        assert_eq!(gregorian_sdn(1970, 1, 1), 2440588);
        assert_eq!(gregorian_sdn(1900, 1, 1), 2415021);
        assert_eq!(gregorian_sdn(2000, 1, 1), 2451545);
        assert_eq!(gregorian_sdn(-44, 3, 15), 1705428);
        assert_eq!(gregorian_sdn(1822, 11, 1), 2386836);
        assert_eq!(julian_sdn(1900, 1, 1), 2415033);
        assert_eq!(julian_sdn(-44, 3, 15), 1705426);
        assert_eq!(julian_sdn(1822, 11, 1), 2386848);
        assert_eq!(gregorian_ymd(1), (-4714, 11, 25));
        assert_eq!(julian_ymd(1), (-4713, 1, 2));
        assert_eq!(gregorian_ymd(gregorian_sdn(1900, 1, 1)), (1900, 1, 1));
    }

    // Golden Julian→Gregorian conversions, captured from
    // `gramps.gen.lib.date.Date.convert_calendar`.
    #[test]
    fn julian_to_gregorian_golden() {
        for (jul, grg) in [
            ((1918, 1, 31), (1918, 2, 13)),
            ((1582, 10, 4), (1582, 10, 14)),
            ((1900, 2, 28), (1900, 3, 12)),
            ((2016, 2, 29), (2016, 3, 13)),
            ((-44, 3, 15), (-44, 3, 13)),
            ((1822, 11, 1), (1822, 11, 13)),
        ] {
            let d = julian(jul, None);
            let out = d.to_gregorian().expect("convertible Julian date");
            assert_eq!(
                (
                    out.year(),
                    u32::from(out.month() as u8),
                    u32::from(out.day() as u8)
                ),
                grg,
                "Julian {jul:?}"
            );
        }
    }

    #[test]
    fn gregorian_to_gregorian_is_identity() {
        let d = greg((1918, 2, 13));
        assert_eq!(d.to_gregorian().unwrap().to_string(), "1918-02-13");
    }

    #[test]
    fn start_stop_accessors() {
        let single = greg((1900, 1, 1));
        assert_eq!(single.start().unwrap().to_string(), "1900-01-01");
        assert_eq!(single.stop(), None);

        let range = date(
            (1822, 11, 1),
            Modifier::Range,
            Calendar::Gregorian,
            false,
            Some((1823, 4, 1)),
        );
        assert_eq!(range.start().unwrap().to_string(), "1822-11-01");
        assert_eq!(range.stop().unwrap().to_string(), "1823-04-01");
        assert!(range.is_range());
    }

    #[test]
    fn partial_and_unknown_dates_do_not_normalize() {
        assert_eq!(greg((1822, 0, 0)).start(), None); // year-only
        assert_eq!(greg((1822, 11, 0)).start(), None); // month-only
        assert_eq!(greg((0, 5, 15)).start(), None); // unknown year
        assert_eq!(greg((2023, 2, 30)).start(), None); // Feb 30
        assert_eq!(greg((2021, 4, 31)).start(), None); // Apr 31
        assert_eq!(greg((1900, 13, 1)).start(), None); // month 13
    }

    #[test]
    fn anniversary_anchors_per_plan_rules() {
        assert_eq!(greg((1900, 1, 1)).anniversary_key(), Some((1, 1)));
        assert_eq!(greg((2000, 12, 31)).anniversary_key(), Some((12, 31)));
        // month-only anchors at day 1 (rule 3)
        assert_eq!(greg((1822, 11, 0)).anniversary_key(), Some((11, 1)));
        // year-only has no anchor (rule 1)
        assert_eq!(greg((1822, 0, 0)).anniversary_key(), None);
        // unknown year → no anchor
        assert_eq!(greg((0, 5, 15)).anniversary_key(), None);
        // invalid day (Apr 31) → no anchor
        assert_eq!(greg((2021, 4, 31)).anniversary_key(), None);
        // range anchors at its start
        let range = date(
            (1822, 11, 0),
            Modifier::Range,
            Calendar::Gregorian,
            false,
            Some((1823, 4, 0)),
        );
        assert_eq!(range.anniversary_key(), Some((11, 1)));
        let range = date(
            (1914, 1, 1),
            Modifier::Span,
            Calendar::Gregorian,
            false,
            Some((1918, 4, 1)),
        );
        assert_eq!(range.anniversary_key(), Some((1, 1)));
        // year-only range → no anchor
        let range = date(
            (1822, 0, 0),
            Modifier::Range,
            Calendar::Gregorian,
            false,
            Some((1824, 0, 0)),
        );
        assert_eq!(range.anniversary_key(), None);
    }

    #[test]
    fn julian_anniversaries_use_the_gregorian_month_day() {
        // Julian 1822-11-01 → Gregorian 1822-11-13
        assert_eq!(
            julian((1822, 11, 1), None).anniversary_key(),
            Some((11, 13))
        );
        // full Julian date required; partial Julian → None
        assert_eq!(julian((1822, 11, 0), None).anniversary_key(), None);
    }

    #[test]
    fn dual_dated_dates_are_treated_as_julian() {
        let d = date(
            (1583, 10, 4),
            Modifier::None,
            Calendar::Gregorian,
            true,
            None,
        );
        // Julian 1583-10-04 → Gregorian 1583-10-14
        assert_eq!(d.to_gregorian().unwrap().to_string(), "1583-10-14");
        assert_eq!(d.anniversary_key(), Some((10, 14)));
    }

    #[test]
    fn non_gregorian_calendars_fall_back_to_none_with_warning() {
        // A fresh test process: reset the warn-once set and capture.
        reset_warnings();
        let d = date((5720, 1, 2), Modifier::None, Calendar::Hebrew, false, None);
        assert_eq!(d.to_gregorian(), None);
        assert_eq!(d.anniversary_key(), None);
        d.to_gregorian(); // second call must not warn again
        assert_eq!(warned_calendars(), HashSet::from([Calendar::Hebrew]));
        reset_warnings();
        for cal in [
            Calendar::Hebrew,
            Calendar::FrenchRepublican,
            Calendar::Persian,
            Calendar::Islamic,
            Calendar::Swedish,
        ] {
            let d = date((1000, 1, 1), Modifier::None, cal, false, None);
            assert_eq!(d.to_gregorian(), None, "{cal:?}");
        }
        assert_eq!(
            warned_calendars(),
            HashSet::from([
                Calendar::Hebrew,
                Calendar::FrenchRepublican,
                Calendar::Persian,
                Calendar::Islamic,
                Calendar::Swedish,
            ])
        );
        reset_warnings();
    }

    #[test]
    fn text_dates_have_no_date_math() {
        let mut d = date(
            (0, 0, 0),
            Modifier::TextOnly,
            Calendar::Gregorian,
            false,
            None,
        );
        d.display = "circa the harvest festival".to_string();
        assert_eq!(d.to_gregorian(), None);
        assert_eq!(d.anniversary_key(), None);
        assert_eq!(d.start(), None);
        assert_eq!(d.stop(), None);
        assert_eq!(d.year(), None);
        assert!(!d.is_range());
    }

    #[cfg(test)]
    pub(crate) fn reset_warnings() {
        NON_GREGORIAN_WARNED
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    #[cfg(test)]
    pub(crate) fn warned_calendars() -> HashSet<Calendar> {
        NON_GREGORIAN_WARNED
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    use proptest::prelude::*;

    proptest! {
        /// `gregorian_ymd ∘ gregorian_sdn` and `julian_ymd ∘ julian_sdn` are
        /// the identity on valid dates.
        #[test]
        fn sdn_round_trips_are_identity(
            year in any_nonzero_year(),
            month in 1u32..=12,
            day in 1u32..=28,
        ) {
            prop_assert_eq!(gregorian_ymd(gregorian_sdn(year, month, day)), (year, month, day));
            prop_assert_eq!(julian_ymd(julian_sdn(year, month, day)), (year, month, day));
        }

        /// Julian → Gregorian → Julian is the identity (same instant).
        #[test]
        fn julian_gregorian_is_a_homeomorphism(
            year in any_nonzero_year(),
            month in 1u32..=12,
            day in 1u32..=28,
        ) {
            let grg = gregorian_ymd(julian_sdn(year, month, day));
            prop_assert_eq!(julian_ymd(gregorian_sdn(grg.0, grg.1, grg.2)), (year, month, day));
        }

        /// Conversion preserves chronological order.
        #[test]
        fn julian_to_gregorian_preserves_order(
            y1 in any_nonzero_year(),
            y2 in any_nonzero_year(),
            m1 in 1u32..=12,
            m2 in 1u32..=12,
            d1 in 1u32..=28,
            d2 in 1u32..=28,
        ) {
            let a = julian_sdn(y1, m1, d1);
            let b = julian_sdn(y2, m2, d2);
            let ga = gregorian_sdn_from_ymd(gregorian_ymd(a));
            let gb = gregorian_sdn_from_ymd(gregorian_ymd(b));
            prop_assert_eq!(ga < gb, a < b);
        }
    }

    fn gregorian_sdn_from_ymd(ymd: (i32, u32, u32)) -> i64 {
        gregorian_sdn(ymd.0, ymd.1, ymd.2)
    }

    /// Any year in the SDN epoch scheme's representable range except 0.
    ///
    /// The Gramps epoch scheme (like the proleptic calendars it ports) has
    /// no year 0: `gregorian_sdn(0, …)` equals `gregorian_sdn(1, …)`, so
    /// `ymd` round-trips to year 1. The model's own convention is the same —
    /// year 0 means "unknown year" and is excluded from normalization by
    /// `to_civil` — so the property ranges skip it.
    fn any_nonzero_year() -> impl Strategy<Value = i32> {
        (-9999i32..=9999).prop_filter("SDN scheme has no year 0", |y| *y != 0)
    }
}
