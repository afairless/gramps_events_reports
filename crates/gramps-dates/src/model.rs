//! Core date model types for Gramps dates.
//!
//! Gramps stores dates as one of four interchangeable XML element forms
//! (`dateval`, `daterange`, `datespan`, `datestr`). This module defines the
//! typed model they all map onto — [`GrampsDate`] plus its component enums
//! [`Calendar`], [`Modifier`], [`Quality`], and [`NewYear`] — and the
//! [`DateError`] used by the parsers in [`crate::parse`].
//!
//! **Partial dates.** Following the Gramps internal convention, missing date
//! components are stored as `0` rather than as `Option`s: `(year, 0, 0)` is a
//! year-only date, `(year, month, 0)` a year-month date, `(year, month, day)`
//! a full date. BC years are negative (`-550`). This keeps the model flat and
//! mirrors what Gramps itself does in `date.py`.

use std::fmt;

use thiserror::Error;

/// The seven calendars Gramps supports, named as in `Date.calendar_names`
/// (`date.py`).
///
/// Per decision D4, v1 only SDN-converts Gregorian and Julian; the other five
/// calendars degrade gracefully to their stored text with a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Calendar {
    Gregorian,
    Julian,
    Hebrew,
    FrenchRepublican,
    Persian,
    Islamic,
    Swedish,
}

impl Calendar {
    /// Map a Gramps `cformat` attribute value to a [`Calendar`].
    ///
    /// `"Gregorian"` is the default and may be omitted entirely by the
    /// exporter; the remaining names match `Date.calendar_names` verbatim.
    /// Unknown names return `None`.
    pub fn from_gramps_str(s: &str) -> Option<Calendar> {
        Some(match s {
            "Gregorian" => Calendar::Gregorian,
            "Julian" => Calendar::Julian,
            "Hebrew" => Calendar::Hebrew,
            "French Republican" => Calendar::FrenchRepublican,
            "Persian" => Calendar::Persian,
            "Islamic" => Calendar::Islamic,
            "Swedish" => Calendar::Swedish,
            _ => return None,
        })
    }

    /// The Gramps calendar name, as written in `cformat=` attributes and in
    /// display-string suffixes (inverse of [`Calendar::from_gramps_str`]).
    pub const fn as_str(self) -> &'static str {
        match self {
            Calendar::Gregorian => "Gregorian",
            Calendar::Julian => "Julian",
            Calendar::Hebrew => "Hebrew",
            Calendar::FrenchRepublican => "French Republican",
            Calendar::Persian => "Persian",
            Calendar::Islamic => "Islamic",
            Calendar::Swedish => "Swedish",
        }
    }
}

/// Date modifier, matching Gramps `Date.MOD_*` (`date.py`).
///
/// `dateval` elements carry `None`, `Before`, `After` or `About` (the
/// `type=` attribute); `Range` and `Span` come from the `daterange` /
/// `datespan` elements and `TextOnly` from `datestr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modifier {
    None,
    Before,
    After,
    About,
    Range,
    Span,
    TextOnly,
}

impl Modifier {
    /// Map a `dateval` `type=` attribute value to a [`Modifier`].
    ///
    /// Grammar-valid values are `before`, `after` and `about`; an absent
    /// attribute (or any other value) maps to `None`.
    pub fn from_gramps_str(s: &str) -> Option<Modifier> {
        Some(match s {
            "before" => Modifier::Before,
            "after" => Modifier::After,
            "about" => Modifier::About,
            _ => return None,
        })
    }
}

/// Quality of a date, matching Gramps `Date.QUAL_*`.
///
/// Expresses the uncertainty *of the date itself*, separate from the
/// modifier: `Estimated` is an approximation, `Calculated` was derived from
/// other data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Quality {
    None,
    Estimated,
    Calculated,
}

impl Quality {
    /// Map a `quality=` attribute value to a [`Quality`].
    ///
    /// Grammar-valid values are `estimated` and `calculated`; an absent
    /// attribute (or any other value) maps to `None`.
    pub fn from_gramps_str(s: &str) -> Option<Quality> {
        Some(match s {
            "estimated" => Quality::Estimated,
            "calculated" => Quality::Calculated,
            _ => return None,
        })
    }
}

/// New Year start convention for a date, matching Gramps `Date.NEWYEAR_*`.
///
/// Historical calendars did not all start the year on January 1; the
/// convention is recorded per date and affects Gregorian conversion
/// (`newyear_to_str` emits `Jan1`, `Mar1`, `Mar25`, `Sep1`; January 1 is the
/// default and is stored as an absent attribute).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NewYear {
    Jan1,
    Mar1,
    Mar25,
    Sep1,
}

impl NewYear {
    /// Map a `newyear=` attribute value to a [`NewYear`].
    ///
    /// Grammar-valid values are `Jan1`, `Mar1`, `Mar25` and `Sep1`; an
    /// absent attribute (or any other value) maps to `Jan1`.
    pub fn from_gramps_str(s: &str) -> Option<NewYear> {
        Some(match s {
            "Jan1" => NewYear::Jan1,
            "Mar1" => NewYear::Mar1,
            "Mar25" => NewYear::Mar25,
            "Sep1" => NewYear::Sep1,
            _ => return None,
        })
    }
}

/// Errors produced while parsing Gramps date elements.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DateError {
    /// The `val` attribute is missing or empty.
    #[error("date value is empty: expected YYYY, YYYY-MM or YYYY-MM-DD")]
    EmptyVal,
    /// The `val` attribute is not a well-formed ISO-ish date.
    #[error(
        "invalid date value {0:?}: expected YYYY, YYYY-MM or YYYY-MM-DD (month and day may be 0)"
    )]
    InvalidVal(String),
    /// Month outside the partial-date range 0..=12.
    #[error("invalid month {0}: expected 0-12")]
    InvalidMonth(u32),
    /// Day outside the partial-date range 0..=31.
    #[error("invalid day {0}: expected 0-31")]
    InvalidDay(u32),
    /// Unknown `type=` attribute on a `dateval` (valid: before|after|about).
    #[error("invalid dateval type {0:?}: expected before, after or about")]
    InvalidModifier(String),
    /// Unknown `quality=` attribute (valid: estimated|calculated).
    #[error("invalid dateval quality {0:?}: expected estimated or calculated")]
    InvalidQuality(String),
    /// Unknown `cformat=` calendar name.
    #[error("unknown calendar {0:?}")]
    InvalidCalendar(String),
    /// `dualdated=` attribute is not the accepted `"1"`.
    #[error("invalid dualdated value {0:?}: expected \"1\"")]
    InvalidDualDated(String),
    /// Unknown `newyear=` attribute (valid: Jan1|Mar1|Mar25|Sep1).
    #[error("invalid newyear {0:?}: expected Jan1, Mar1, Mar25 or Sep1")]
    InvalidNewYear(String),
    /// A `daterange`/`datespan` whose stop endpoint sorts before its start
    /// endpoint — treated as a hard parse error (plan §7.1).
    #[error("daterange/datespan stop {1:?} sorts before start {0:?}")]
    RangeStartAfterStop(String, String),
}

/// A Gramps date.
///
/// The struct is deliberately flat and mirrors `date.py`: a `dateval` fills
/// `ymd` with the single date; `daterange`/`datespan` additionally fill
/// `stop` (both endpoints). `display` carries the human-readable rendering
/// Gramps would print — the same strings `Date.__str__` produces, e.g.
/// `"bef 1914-01-01"`, `"est 1822-11-00 - 1823-04-00"` or
/// `"1900-01-01 (Julian)"` — computed at parse time by
/// [`GrampsDate::compute_display`] (`crate::display`).
///
/// **Text-only dates** (`datestr`): the verbatim text is stored in
/// `display`, the modifier is [`Modifier::TextOnly`] and `ymd` stays
/// `(0, 0, 0)`; use [`GrampsDate::text`] to read it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrampsDate {
    pub calendar: Calendar,
    pub modifier: Modifier,
    pub quality: Quality,
    /// ISO-ish date, with month/day 0 meaning "unknown" (partial dates).
    /// BC years are negative.
    pub ymd: (i32, u32, u32),
    /// End point of a `daterange`/`datespan`. Always `None` for datevals
    /// (start and stop are equal); month/day 0 again means partial.
    pub stop: Option<(i32, u32, u32)>,
    /// `dualdated="1"` — e.g. dates straddling the Julian/Gregorian overlap.
    pub dual_dated: bool,
    pub new_year: NewYear,
    /// What Gramps would print for this date, e.g. `"bef 1914-01-01"`;
    /// for [`Modifier::TextOnly`] dates this field holds the verbatim text.
    pub display: String,
}

impl GrampsDate {
    /// True if the modifier is `Range` or `Span` (any quality).
    pub fn is_range(&self) -> bool {
        matches!(self.modifier, Modifier::Range | Modifier::Span)
    }

    /// The start year of the date, or `None` when the year is unknown (0).
    pub fn year(&self) -> Option<i32> {
        (self.ymd.0 != 0).then_some(self.ymd.0)
    }

    /// The verbatim text of a `datestr` date, or `None` for any other form.
    pub fn text(&self) -> Option<&str> {
        (self.modifier == Modifier::TextOnly).then_some(self.display.as_str())
    }
}

impl fmt::Display for GrampsDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_mapping_matches_gramps_names() {
        assert_eq!(
            Calendar::from_gramps_str("Gregorian"),
            Some(Calendar::Gregorian)
        );
        assert_eq!(Calendar::from_gramps_str("Julian"), Some(Calendar::Julian));
        assert_eq!(Calendar::from_gramps_str("Hebrew"), Some(Calendar::Hebrew));
        assert_eq!(
            Calendar::from_gramps_str("French Republican"),
            Some(Calendar::FrenchRepublican)
        );
        assert_eq!(
            Calendar::from_gramps_str("Persian"),
            Some(Calendar::Persian)
        );
        assert_eq!(
            Calendar::from_gramps_str("Islamic"),
            Some(Calendar::Islamic)
        );
        assert_eq!(
            Calendar::from_gramps_str("Swedish"),
            Some(Calendar::Swedish)
        );
        assert_eq!(Calendar::from_gramps_str("Mayan"), None);
        assert_eq!(Calendar::from_gramps_str(""), None);
    }

    #[test]
    fn modifier_mapping_covers_dateval_types() {
        assert_eq!(Modifier::from_gramps_str("before"), Some(Modifier::Before));
        assert_eq!(Modifier::from_gramps_str("after"), Some(Modifier::After));
        assert_eq!(Modifier::from_gramps_str("about"), Some(Modifier::About));
        assert_eq!(Modifier::from_gramps_str("between"), None);
    }

    #[test]
    fn quality_mapping_covers_gramps_values() {
        assert_eq!(
            Quality::from_gramps_str("estimated"),
            Some(Quality::Estimated)
        );
        assert_eq!(
            Quality::from_gramps_str("calculated"),
            Some(Quality::Calculated)
        );
        assert_eq!(Quality::from_gramps_str("sure"), None);
    }

    #[test]
    fn newyear_mapping_covers_gramps_values() {
        assert_eq!(NewYear::from_gramps_str("Jan1"), Some(NewYear::Jan1));
        assert_eq!(NewYear::from_gramps_str("Mar1"), Some(NewYear::Mar1));
        assert_eq!(NewYear::from_gramps_str("Mar25"), Some(NewYear::Mar25));
        assert_eq!(NewYear::from_gramps_str("Sep1"), Some(NewYear::Sep1));
        assert_eq!(NewYear::from_gramps_str("June1"), None);
    }

    #[test]
    fn is_range_holds_for_range_and_span_only() {
        let base = |modifier| GrampsDate {
            calendar: Calendar::Gregorian,
            modifier,
            quality: Quality::None,
            ymd: (1900, 1, 1),
            stop: None,
            dual_dated: false,
            new_year: NewYear::Jan1,
            display: "1900-01-01".to_string(),
        };
        assert!(base(Modifier::Range).is_range());
        assert!(base(Modifier::Span).is_range());
        for modifier in [
            Modifier::None,
            Modifier::Before,
            Modifier::After,
            Modifier::About,
            Modifier::TextOnly,
        ] {
            assert!(
                !base(modifier).is_range(),
                "{modifier:?} must not be a range"
            );
        }
    }

    #[test]
    fn year_is_option_over_unknown_years() {
        let date = |year| GrampsDate {
            calendar: Calendar::Gregorian,
            modifier: Modifier::None,
            quality: Quality::None,
            ymd: (year, 5, 15),
            stop: None,
            dual_dated: false,
            new_year: NewYear::Jan1,
            display: String::new(),
        };
        assert_eq!(date(1822).year(), Some(1822));
        assert_eq!(date(-550).year(), Some(-550));
        assert_eq!(date(0).year(), None);
    }

    #[test]
    fn display_renders_the_stored_text() {
        let date = GrampsDate {
            calendar: Calendar::Julian,
            modifier: Modifier::About,
            quality: Quality::Estimated,
            ymd: (1900, 1, 1),
            stop: None,
            dual_dated: true,
            new_year: NewYear::Mar25,
            display: "about 1900".to_string(),
        };
        assert_eq!(date.to_string(), "about 1900");
    }
}
