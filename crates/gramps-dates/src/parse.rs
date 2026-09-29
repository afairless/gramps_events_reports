//! Parsing of the `dateval` element form.
//!
//! Gramps serializes a plain date as a `<dateval>` element (see
//! `plugins/export/exportxml.py`):
//!
//! ```xml
//! <dateval val="YYYY-MM-DD" [type=before|after|about]
//!          [quality=estimated|calculated] [cformat=...]
//!          [dualdated="1"] [newyear=Jan1|Mar1|Mar25|Sep1]/>
//! ```
//!
//! `val` is an ISO-ish date in the form `YYYY`, `YYYY-MM` or `YYYY-MM-DD`;
//! missing month/day mean "partial" and parse to `0` (see [`crate::model`]).
//! BC years are negative (`-0550-04-22`), and an unknown year may be written
//! as `????` (Gramps' exporter emits `?` for years it cannot represent).

use crate::model::{Calendar, DateError, GrampsDate, Modifier, NewYear, Quality};

/// String attributes of a `<dateval>` element, exactly as read from XML.
///
/// Only `val` is required; every other attribute is optional and matches the
/// grammar Gramps' exporter writes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DatevalAttrs<'a> {
    /// The `val` attribute: `YYYY`, `YYYY-MM` or `YYYY-MM-DD`; BC years are
    /// negative; unknown years may be `????`. Required.
    pub val: &'a str,
    /// The `type` attribute: `before`, `after` or `about`.
    pub kind: Option<&'a str>,
    /// The `quality` attribute: `estimated` or `calculated`.
    pub quality: Option<&'a str>,
    /// The `cformat` attribute: one of the seven Gramps calendar names.
    pub cformat: Option<&'a str>,
    /// The `dualdated` attribute: `"1"` when the date is dual dated.
    pub dualdated: Option<&'a str>,
    /// The `newyear` attribute: `Jan1`, `Mar1`, `Mar25` or `Sep1`.
    pub newyear: Option<&'a str>,
}

/// Parse a `<dateval>` element into a [`GrampsDate`].
///
/// ```
/// use gramps_dates::parse::{parse_dateval, DatevalAttrs};
/// use gramps_dates::{Calendar, Modifier, NewYear, Quality};
///
/// let d = parse_dateval(DatevalAttrs {
///     val: "1914-08-15",
///     kind: Some("before"),
///     quality: Some("calculated"),
///     ..DatevalAttrs::default()
/// })
/// .unwrap();
///
/// assert_eq!(d.ymd, (1914, 8, 15));
/// assert_eq!(d.modifier, Modifier::Before);
/// assert_eq!(d.quality, Quality::Calculated);
/// assert_eq!(d.calendar, Calendar::Gregorian);
/// assert_eq!(d.new_year, NewYear::Jan1);
/// assert!(!d.dual_dated);
/// assert!(!d.is_range());
/// ```
pub fn parse_dateval(attrs: DatevalAttrs<'_>) -> Result<GrampsDate, DateError> {
    let ymd = parse_val(attrs.val)?;
    let calendar = match attrs.cformat {
        None => Calendar::Gregorian,
        Some(s) => {
            Calendar::from_gramps_str(s).ok_or_else(|| DateError::InvalidCalendar(s.to_string()))?
        }
    };
    let modifier = match attrs.kind {
        None => Modifier::None,
        Some(s) => {
            Modifier::from_gramps_str(s).ok_or_else(|| DateError::InvalidModifier(s.to_string()))?
        }
    };
    let quality = match attrs.quality {
        None => Quality::None,
        Some(s) => {
            Quality::from_gramps_str(s).ok_or_else(|| DateError::InvalidQuality(s.to_string()))?
        }
    };
    let new_year = match attrs.newyear {
        None => NewYear::Jan1,
        Some(s) => {
            NewYear::from_gramps_str(s).ok_or_else(|| DateError::InvalidNewYear(s.to_string()))?
        }
    };
    let dual_dated = match attrs.dualdated {
        None => false,
        Some("1") => true,
        Some(other) => return Err(DateError::InvalidDualDated(other.to_string())),
    };
    Ok(GrampsDate {
        calendar,
        modifier,
        quality,
        ymd,
        stop: None,
        dual_dated,
        new_year,
        // Milestone 4 replaces this with the full Gramps display-string
        // formatting (modifier prefixes, month names, BC suffix, ranges).
        display: attrs.val.trim().to_string(),
    })
}

/// Parse the `val` attribute into a partial-date `(year, month, day)` triple.
///
/// Accepted forms: `YYYY`, `YYYY-MM`, `YYYY-MM-DD` (month/day may be `0` or
/// missing for partial dates), with an optional leading `-` for BC years and
/// `????` for an unknown year. Rejects anything else.
fn parse_val(val: &str) -> Result<(i32, u32, u32), DateError> {
    let val = val.trim();
    if val.is_empty() {
        return Err(DateError::EmptyVal);
    }
    let (negative, rest) = match val.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, val),
    };

    let mut parts = rest.split('-');
    let year_part = parts.next().unwrap_or("");
    let month_part = parts.next();
    let day_part = parts.next();
    if parts.next().is_some() {
        return Err(DateError::InvalidVal(val.to_string()));
    }

    let year_u =
        parse_component(year_part).ok_or_else(|| DateError::InvalidVal(val.to_string()))?;
    let year: i32 = if negative {
        i32::try_from(year_u)
            .ok()
            .and_then(i32::checked_neg)
            .ok_or_else(|| DateError::InvalidVal(val.to_string()))?
    } else {
        i32::try_from(year_u).map_err(|_| DateError::InvalidVal(val.to_string()))?
    };

    let month = match month_part {
        None => 0,
        Some(m) => {
            let m = parse_component(m).ok_or_else(|| DateError::InvalidVal(val.to_string()))?;
            if m > 12 {
                return Err(DateError::InvalidMonth(m));
            }
            m
        }
    };
    let day = match day_part {
        None => 0,
        Some(d) => {
            let d = parse_component(d).ok_or_else(|| DateError::InvalidVal(val.to_string()))?;
            if d > 31 {
                return Err(DateError::InvalidDay(d));
            }
            d
        }
    };

    Ok((year, month, day))
}

/// Parse one `YYYY`/`MM`/`DD` component.
///
/// A run of `?` (Gramps' "unknown" marker) parses to `0`; any other
/// non-numeric content fails, as does an empty component.
fn parse_component(s: &str) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    if s.chars().all(|c| c == '?') {
        return Some(0);
    }
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(val: &str) -> Result<GrampsDate, DateError> {
        parse_dateval(DatevalAttrs {
            val,
            ..DatevalAttrs::default()
        })
    }

    #[test]
    fn full_date() {
        let d = parse("2000-03-03").unwrap();
        assert_eq!(d.ymd, (2000, 3, 3));
        assert_eq!(d.calendar, Calendar::Gregorian);
        assert_eq!(d.modifier, Modifier::None);
        assert_eq!(d.quality, Quality::None);
        assert!(!d.dual_dated);
        assert_eq!(d.new_year, NewYear::Jan1);
        assert_eq!(d.stop, None);
        assert_eq!(d.year(), Some(2000));
    }

    #[test]
    fn partial_dates_use_zero_components() {
        // year-only
        let d = parse("1822").unwrap();
        assert_eq!(d.ymd, (1822, 0, 0));
        assert_eq!(d.year(), Some(1822));
        // year-month
        let d = parse("1822-11").unwrap();
        assert_eq!(d.ymd, (1822, 11, 0));
        // explicit zero month/day is tolerated
        let d = parse("1822-00-00").unwrap();
        assert_eq!(d.ymd, (1822, 0, 0));
    }

    #[test]
    fn bc_years_are_negative() {
        let d = parse("-0550-04-22").unwrap();
        assert_eq!(d.ymd, (-550, 4, 22));
        assert_eq!(d.year(), Some(-550));
    }

    #[test]
    fn unknown_year_parses_to_zero() {
        let d = parse("????-05-15").unwrap();
        assert_eq!(d.ymd, (0, 5, 15));
        assert_eq!(d.year(), None);
    }

    #[test]
    fn zero_year_is_unknown() {
        assert_eq!(parse("0000").unwrap().ymd, (0, 0, 0));
    }

    #[test]
    fn leading_and_trailing_whitespace_is_tolerated() {
        let d = parse("  1822-11  ").unwrap();
        assert_eq!(d.ymd, (1822, 11, 0));
        assert_eq!(d.display, "1822-11");
    }

    #[test]
    fn about_before_after_modifiers() {
        let d = parse_dateval(DatevalAttrs {
            val: "1900-01-01",
            kind: Some("about"),
            ..DatevalAttrs::default()
        })
        .unwrap();
        assert_eq!(d.modifier, Modifier::About);
        let d = parse_dateval(DatevalAttrs {
            val: "1914-06-28",
            kind: Some("before"),
            ..DatevalAttrs::default()
        })
        .unwrap();
        assert_eq!(d.modifier, Modifier::Before);
        let d = parse_dateval(DatevalAttrs {
            val: "1945-05-08",
            kind: Some("after"),
            ..DatevalAttrs::default()
        })
        .unwrap();
        assert_eq!(d.modifier, Modifier::After);
    }

    #[test]
    fn qualities() {
        let d = parse_dateval(DatevalAttrs {
            val: "1918-11-11",
            quality: Some("estimated"),
            ..DatevalAttrs::default()
        })
        .unwrap();
        assert_eq!(d.quality, Quality::Estimated);
        let d = parse_dateval(DatevalAttrs {
            val: "1914-08-15",
            kind: Some("before"),
            quality: Some("calculated"),
            ..DatevalAttrs::default()
        })
        .unwrap();
        assert_eq!(d.quality, Quality::Calculated);
        assert_eq!(d.modifier, Modifier::Before);
    }

    #[test]
    fn dual_dated_flag() {
        let d = parse_dateval(DatevalAttrs {
            val: "1582-10-04",
            dualdated: Some("1"),
            ..DatevalAttrs::default()
        })
        .unwrap();
        assert!(d.dual_dated);
    }

    #[test]
    fn all_non_default_calendars() {
        for (cformat, expected) in [
            ("Julian", Calendar::Julian),
            ("Hebrew", Calendar::Hebrew),
            ("French Republican", Calendar::FrenchRepublican),
            ("Persian", Calendar::Persian),
            ("Islamic", Calendar::Islamic),
            ("Swedish", Calendar::Swedish),
        ] {
            let d = parse_dateval(DatevalAttrs {
                val: "1900-01-01",
                cformat: Some(cformat),
                ..DatevalAttrs::default()
            })
            .unwrap();
            assert_eq!(d.calendar, expected, "cformat={cformat:?}");
        }
    }

    #[test]
    fn explicit_gregorian_cformat_is_accepted() {
        let d = parse_dateval(DatevalAttrs {
            val: "1900-01-01",
            cformat: Some("Gregorian"),
            ..DatevalAttrs::default()
        })
        .unwrap();
        assert_eq!(d.calendar, Calendar::Gregorian);
    }

    #[test]
    fn new_year_starts() {
        let d = parse_dateval(DatevalAttrs {
            val: "1580-03-26",
            newyear: Some("Mar25"),
            ..DatevalAttrs::default()
        })
        .unwrap();
        assert_eq!(d.new_year, NewYear::Mar25);
        for (s, expected) in [
            ("Jan1", NewYear::Jan1),
            ("Mar1", NewYear::Mar1),
            ("Sep1", NewYear::Sep1),
        ] {
            let d = parse_dateval(DatevalAttrs {
                val: "1900-01-01",
                newyear: Some(s),
                ..DatevalAttrs::default()
            })
            .unwrap();
            assert_eq!(d.new_year, expected, "newyear={s:?}");
        }
    }

    #[test]
    fn every_attribute_combined() {
        let d = parse_dateval(DatevalAttrs {
            val: "1700-02-18",
            kind: Some("about"),
            quality: Some("estimated"),
            cformat: Some("Julian"),
            dualdated: Some("1"),
            newyear: Some("Mar1"),
        })
        .unwrap();
        assert_eq!(d.ymd, (1700, 2, 18));
        assert_eq!(d.calendar, Calendar::Julian);
        assert_eq!(d.modifier, Modifier::About);
        assert_eq!(d.quality, Quality::Estimated);
        assert!(d.dual_dated);
        assert_eq!(d.new_year, NewYear::Mar1);
        assert_eq!(d.display, "1700-02-18");
    }

    #[test]
    fn datevals_are_never_ranges() {
        let d = parse("1914-06-28").unwrap();
        assert!(!d.is_range());
    }

    #[test]
    fn rejects_empty_val() {
        assert_eq!(parse(""), Err(DateError::EmptyVal));
        assert_eq!(parse("   "), Err(DateError::EmptyVal));
    }

    #[test]
    fn rejects_unparseable_vals() {
        for val in [
            "garbage",
            "1822-",
            "1822-11-",
            "1822-11-05-extra",
            "-",
            "1-2-3-4",
            "12?",
            "1?2",
        ] {
            assert!(
                matches!(parse(val), Err(DateError::InvalidVal(_))),
                "val={val:?}"
            );
        }
    }

    #[test]
    fn rejects_out_of_range_components() {
        assert_eq!(parse("1822-13"), Err(DateError::InvalidMonth(13)));
        // "18-22-11" is structurally fine; the month is the offending part.
        assert_eq!(parse("18-22-11"), Err(DateError::InvalidMonth(22)));
        assert_eq!(parse("1822-00-32"), Err(DateError::InvalidDay(32)));
    }

    #[test]
    fn rejects_bad_attributes() {
        let err = parse_dateval(DatevalAttrs {
            val: "1900-01-01",
            kind: Some("between"),
            ..DatevalAttrs::default()
        })
        .unwrap_err();
        assert_eq!(err, DateError::InvalidModifier("between".to_string()));

        let err = parse_dateval(DatevalAttrs {
            val: "1900-01-01",
            quality: Some("sure"),
            ..DatevalAttrs::default()
        })
        .unwrap_err();
        assert_eq!(err, DateError::InvalidQuality("sure".to_string()));

        let err = parse_dateval(DatevalAttrs {
            val: "1900-01-01",
            cformat: Some("Mayan"),
            ..DatevalAttrs::default()
        })
        .unwrap_err();
        assert_eq!(err, DateError::InvalidCalendar("Mayan".to_string()));

        let err = parse_dateval(DatevalAttrs {
            val: "1900-01-01",
            dualdated: Some("0"),
            ..DatevalAttrs::default()
        })
        .unwrap_err();
        assert_eq!(err, DateError::InvalidDualDated("0".to_string()));

        let err = parse_dateval(DatevalAttrs {
            val: "1900-01-01",
            newyear: Some("June1"),
            ..DatevalAttrs::default()
        })
        .unwrap_err();
        assert_eq!(err, DateError::InvalidNewYear("June1".to_string()));
    }
}
