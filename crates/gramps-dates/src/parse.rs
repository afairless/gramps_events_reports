//! Parsing of the four Gramps date element forms.
//!
//! Gramps serializes a date as one of four interchangeable XML elements (see
//! `plugins/export/exportxml.py`):
//!
//! ```xml
//! <dateval val="YYYY-MM-DD" [type=before|after|about]
//!          [quality=estimated|calculated] [cformat=...]
//!          [dualdated="1"] [newyear=Jan1|Mar1|Mar25|Sep1]/>
//! <daterange start="..." stop="..." [quality=...] [cformat=...]
//!            [dualdated="1"] [newyear=...]/>
//! <datespan start="..." stop="..." [quality=...] [cformat=...]
//!            [dualdated="1"] [newyear=...]/>
//! <datestr val="free text"/>
//! ```
//!
//! `val`/`start`/`stop` are ISO-ish dates in the form `YYYY`, `YYYY-MM` or
//! `YYYY-MM-DD`; missing month/day mean "partial" and parse to `0` (see
//! [`crate::model`]). BC years are negative (`-0550-04-22`), and an unknown
//! year may be written as `????` (Gramps' exporter emits `?` for years it
//! cannot represent).

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
    let modifier = match attrs.kind {
        None => Modifier::None,
        Some(s) => {
            Modifier::from_gramps_str(s).ok_or_else(|| DateError::InvalidModifier(s.to_string()))?
        }
    };
    let mut d = GrampsDate {
        calendar: parse_calendar(attrs.cformat)?,
        modifier,
        quality: parse_quality(attrs.quality)?,
        ymd,
        stop: None,
        dual_dated: parse_dual_dated(attrs.dualdated)?,
        new_year: parse_new_year(attrs.newyear)?,
        display: String::new(),
    };
    // The parsers stamp the full Gramps display string at parse time
    // (the `Date.__str__` equivalent, e.g. "bef 1914-01-01").
    d.display = d.compute_display();
    Ok(d)
}

/// String attributes of the `<daterange>` / `<datespan>` elements, exactly
/// as read from XML. `start` and `stop` are required (grampsxml.dtd); every
/// other attribute is optional and matches the grammar Gramps' exporter
/// writes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DateRangeAttrs<'a> {
    /// The `start` attribute: `YYYY`, `YYYY-MM` or `YYYY-MM-DD`; BC years
    /// are negative; unknown years may be `????`. Required.
    pub start: &'a str,
    /// The `stop` attribute: `YYYY`, `YYYY-MM` or `YYYY-MM-DD`. Required.
    pub stop: &'a str,
    /// The `quality` attribute: `estimated` or `calculated`.
    pub quality: Option<&'a str>,
    /// The `cformat` attribute: one of the seven Gramps calendar names.
    pub cformat: Option<&'a str>,
    /// The `dualdated` attribute: `"1"` when the date is dual dated.
    pub dualdated: Option<&'a str>,
    /// The `newyear` attribute: `Jan1`, `Mar1`, `Mar25` or `Sep1`.
    pub newyear: Option<&'a str>,
}

/// Parse a `<daterange>` element into a [`GrampsDate`].
///
/// Both endpoints are validated and ordered (`stop` must not sort before
/// `start`); the display string is the compound `start - stop` form.
///
/// ```
/// use gramps_dates::parse::{DateRangeAttrs, parse_daterange};
/// use gramps_dates::Modifier;
///
/// let d = parse_daterange(DateRangeAttrs {
///     start: "1914-01-01",
///     stop: "1918-04-01",
///     ..DateRangeAttrs::default()
/// })
/// .unwrap();
///
/// assert_eq!(d.modifier, Modifier::Range);
/// assert_eq!(d.ymd, (1914, 1, 1));
/// assert_eq!(d.stop, Some((1918, 4, 1)));
/// assert!(d.is_range());
/// assert_eq!(d.to_string(), "1914-01-01 - 1918-04-01");
/// ```
pub fn parse_daterange(attrs: DateRangeAttrs<'_>) -> Result<GrampsDate, DateError> {
    parse_compound(attrs, Modifier::Range)
}

/// Parse a `<datespan>` element into a [`GrampsDate`].
///
/// A span is a compound date whose event *lasted* from the start to the stop
/// endpoint (e.g. a tenure); it is stored with [`Modifier::Span`]. All the
/// validation of [`parse_daterange`] applies.
///
/// ```
/// use gramps_dates::parse::{DateRangeAttrs, parse_datespan};
/// use gramps_dates::Modifier;
///
/// let d = parse_datespan(DateRangeAttrs {
///     start: "1822-11-01",
///     stop: "1823-04-01",
///     ..DateRangeAttrs::default()
/// })
/// .unwrap();
///
/// assert_eq!(d.modifier, Modifier::Span);
/// assert_eq!(d.ymd, (1822, 11, 1));
/// assert_eq!(d.stop, Some((1823, 4, 1)));
/// ```
pub fn parse_datespan(attrs: DateRangeAttrs<'_>) -> Result<GrampsDate, DateError> {
    parse_compound(attrs, Modifier::Span)
}

/// Parse a `<datestr>` element into a [`GrampsDate`].
///
/// Text-only dates have no calendar math: the verbatim text is stored in
/// `GrampsDate::display`, the modifier is [`Modifier::TextOnly`] and `ymd`
/// stays `(0, 0, 0)`.
///
/// ```
/// use gramps_dates::parse::parse_datestr;
/// use gramps_dates::Modifier;
///
/// let d = parse_datestr("circa the harvest festival").unwrap();
///
/// assert_eq!(d.modifier, Modifier::TextOnly);
/// assert_eq!(d.ymd, (0, 0, 0));
/// assert_eq!(d.year(), None);
/// assert!(!d.is_range());
/// assert_eq!(d.text(), Some("circa the harvest festival"));
/// assert_eq!(d.to_string(), "circa the harvest festival");
/// ```
pub fn parse_datestr(val: &str) -> Result<GrampsDate, DateError> {
    if val.is_empty() {
        return Err(DateError::EmptyVal);
    }
    Ok(GrampsDate {
        calendar: Calendar::Gregorian,
        modifier: Modifier::TextOnly,
        quality: Quality::None,
        ymd: (0, 0, 0),
        stop: None,
        dual_dated: false,
        new_year: NewYear::Jan1,
        display: val.to_string(),
    })
}

/// Shared implementation of the `daterange`/`datespan` parsers: validates
/// both endpoints, enforces the ordering (`stop` must not sort before
/// `start` — a hard parse error per the plan §7.1), and computes the
/// compound display string.
fn parse_compound(attrs: DateRangeAttrs<'_>, modifier: Modifier) -> Result<GrampsDate, DateError> {
    let start = parse_val(attrs.start)?;
    let stop = parse_val(attrs.stop)?;
    if stop < start {
        return Err(DateError::RangeStartAfterStop(
            attrs.start.trim().to_string(),
            attrs.stop.trim().to_string(),
        ));
    }
    let mut d = GrampsDate {
        calendar: parse_calendar(attrs.cformat)?,
        modifier,
        quality: parse_quality(attrs.quality)?,
        ymd: start,
        stop: Some(stop),
        dual_dated: parse_dual_dated(attrs.dualdated)?,
        new_year: parse_new_year(attrs.newyear)?,
        display: String::new(),
    };
    d.display = d.compute_display();
    Ok(d)
}

/// Parse the shared `cformat=` attribute (defaults to Gregorian).
fn parse_calendar(cformat: Option<&str>) -> Result<Calendar, DateError> {
    match cformat {
        None => Ok(Calendar::Gregorian),
        Some(s) => Ok(Calendar::from_gramps_str(s)
            .ok_or_else(|| DateError::InvalidCalendar(s.to_string()))?),
    }
}

/// Parse the shared `quality=` attribute (defaults to `None`).
fn parse_quality(quality: Option<&str>) -> Result<Quality, DateError> {
    match quality {
        None => Ok(Quality::None),
        Some(s) => {
            Ok(Quality::from_gramps_str(s)
                .ok_or_else(|| DateError::InvalidQuality(s.to_string()))?)
        }
    }
}

/// Parse the shared `newyear=` attribute (defaults to `Jan1`).
fn parse_new_year(newyear: Option<&str>) -> Result<NewYear, DateError> {
    match newyear {
        None => Ok(NewYear::Jan1),
        Some(s) => {
            Ok(NewYear::from_gramps_str(s)
                .ok_or_else(|| DateError::InvalidNewYear(s.to_string()))?)
        }
    }
}

/// Parse the shared `dualdated=` attribute; only the accepted `"1"` is true.
fn parse_dual_dated(dualdated: Option<&str>) -> Result<bool, DateError> {
    match dualdated {
        None => Ok(false),
        Some("1") => Ok(true),
        Some(other) => Err(DateError::InvalidDualDated(other.to_string())),
    }
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
        assert_eq!(d.display, "1822-11-00");
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
        assert_eq!(d.display, "est abt 1699/0-02-18 (Julian,Mar1)");
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

    // ---- compound (`daterange` / `datespan`) and text (`datestr`) ----

    #[test]
    fn display_strings_are_computed_at_parse_time() {
        let d = parse_dateval(DatevalAttrs {
            val: "1914-06-28",
            kind: Some("before"),
            ..DatevalAttrs::default()
        })
        .unwrap();
        assert_eq!(d.display, "bef 1914-06-28");
        let d = parse_dateval(DatevalAttrs {
            val: "1822-00-00",
            ..DatevalAttrs::default()
        })
        .unwrap();
        assert_eq!(d.display, "1822-00-00");
        let d = parse_dateval(DatevalAttrs {
            val: "1900-01-01",
            cformat: Some("Julian"),
            ..DatevalAttrs::default()
        })
        .unwrap();
        assert_eq!(d.display, "1900-01-01 (Julian)");
    }

    #[test]
    fn daterange_parses_both_endpoints() {
        let d = parse_daterange(DateRangeAttrs {
            start: "1914-01-01",
            stop: "1918-04-01",
            ..DateRangeAttrs::default()
        })
        .unwrap();
        assert_eq!(d.modifier, Modifier::Range);
        assert_eq!(d.ymd, (1914, 1, 1));
        assert_eq!(d.stop, Some((1918, 4, 1)));
        assert!(d.is_range());
        assert_eq!(d.display, "1914-01-01 - 1918-04-01");
    }

    #[test]
    fn daterange_accepts_partial_endpoints() {
        let d = parse_daterange(DateRangeAttrs {
            start: "1822-11-00",
            stop: "1823-04-00",
            ..DateRangeAttrs::default()
        })
        .unwrap();
        assert_eq!(d.ymd, (1822, 11, 0));
        assert_eq!(d.stop, Some((1823, 4, 0)));
        assert_eq!(d.display, "1822-11-00 - 1823-04-00");
        // year-only endpoints: the "1822-1824" range from the plan fixtures
        let d = parse_daterange(DateRangeAttrs {
            start: "1822-00-00",
            stop: "1824-00-00",
            ..DateRangeAttrs::default()
        })
        .unwrap();
        assert_eq!(d.ymd, (1822, 0, 0));
        assert_eq!(d.stop, Some((1824, 0, 0)));
        assert_eq!(d.display, "1822-00-00 - 1824-00-00");
    }

    #[test]
    fn datespan_uses_span_modifier() {
        let d = parse_datespan(DateRangeAttrs {
            start: "1822-11-01",
            stop: "1823-04-01",
            ..DateRangeAttrs::default()
        })
        .unwrap();
        assert_eq!(d.modifier, Modifier::Span);
        assert_eq!(d.ymd, (1822, 11, 1));
        assert_eq!(d.stop, Some((1823, 4, 1)));
        assert!(d.is_range());
        assert_eq!(d.display, "1822-11-01 - 1823-04-01");
    }

    #[test]
    fn compound_shared_attributes_apply() {
        let d = parse_datespan(DateRangeAttrs {
            start: "1822-00-00",
            stop: "1824-00-00",
            quality: Some("estimated"),
            cformat: Some("Julian"),
            dualdated: Some("1"),
            newyear: Some("Mar25"),
        })
        .unwrap();
        assert_eq!(d.modifier, Modifier::Span);
        assert_eq!(d.quality, Quality::Estimated);
        assert_eq!(d.calendar, Calendar::Julian);
        assert!(d.dual_dated);
        assert_eq!(d.new_year, NewYear::Mar25);
        // dual-dated compounds render both endpoints in slash form
        assert_eq!(d.display, "est 1821/2-00-00 - 1823/4-00-00 (Julian,Mar25)");
    }

    #[test]
    fn compound_stop_before_start_is_a_hard_error() {
        let err = parse_daterange(DateRangeAttrs {
            start: "1918-04-01",
            stop: "1914-01-01",
            ..DateRangeAttrs::default()
        })
        .unwrap_err();
        assert_eq!(
            err,
            DateError::RangeStartAfterStop("1918-04-01".to_string(), "1914-01-01".to_string())
        );
        // also rejects out-of-order partial endpoints
        let err = parse_datespan(DateRangeAttrs {
            start: "1823-04-00",
            stop: "1822-11-00",
            ..DateRangeAttrs::default()
        })
        .unwrap_err();
        assert_eq!(
            err,
            DateError::RangeStartAfterStop("1823-04-00".to_string(), "1822-11-00".to_string())
        );
        // equal endpoints are degenerate but ordered, so they parse
        let d = parse_daterange(DateRangeAttrs {
            start: "1914-01-01",
            stop: "1914-01-01",
            ..DateRangeAttrs::default()
        })
        .unwrap();
        assert_eq!(d.stop, Some((1914, 1, 1)));
    }

    #[test]
    fn compound_rejects_malformed_or_empty_endpoints() {
        assert_eq!(
            parse_daterange(DateRangeAttrs {
                start: "garbage",
                stop: "1918-01-01",
                ..DateRangeAttrs::default()
            })
            .unwrap_err(),
            DateError::InvalidVal("garbage".to_string())
        );
        assert_eq!(
            parse_daterange(DateRangeAttrs {
                start: "1914-13-01",
                stop: "1918-01-01",
                ..DateRangeAttrs::default()
            })
            .unwrap_err(),
            DateError::InvalidMonth(13)
        );
        assert_eq!(
            parse_datespan(DateRangeAttrs {
                start: "",
                stop: "1918-01-01",
                ..DateRangeAttrs::default()
            })
            .unwrap_err(),
            DateError::EmptyVal
        );
        // malformed optional attributes surface the dateval errors too
        assert_eq!(
            parse_daterange(DateRangeAttrs {
                start: "1914-01-01",
                stop: "1918-01-01",
                cformat: Some("Mayan"),
                ..DateRangeAttrs::default()
            })
            .unwrap_err(),
            DateError::InvalidCalendar("Mayan".to_string())
        );
    }

    #[test]
    fn datestr_is_text_only() {
        let d = parse_datestr("circa the harvest festival").unwrap();
        assert_eq!(d.modifier, Modifier::TextOnly);
        assert_eq!(d.ymd, (0, 0, 0));
        assert_eq!(d.stop, None);
        assert_eq!(d.year(), None);
        assert!(!d.is_range());
        assert_eq!(d.display, "circa the harvest festival");
        assert_eq!(d.text(), Some("circa the harvest festival"));
    }

    #[test]
    fn datestr_keeps_the_text_verbatim() {
        let d = parse_datestr("  between the wars  ").unwrap();
        assert_eq!(d.display, "  between the wars  ");
        assert_eq!(d.text(), Some("  between the wars  "));
    }

    #[test]
    fn datestr_rejects_only_the_empty_string() {
        assert_eq!(parse_datestr(""), Err(DateError::EmptyVal));
    }

    use proptest::prelude::*;

    proptest! {
        /// Every plain (Gregorian, Jan 1, unmodified) dateval round-trips
        /// through its own display string: `parse(display(parse(val)))` equals
        /// `parse(val)` (plan §11 property test).
        #[test]
        fn plain_datevals_round_trip_through_their_display_string(
            year in -9999i32..=9999,
            month in 0u32..=12,
            day in 0u32..=31,
        ) {
            let val = format!("{:04}-{:02}-{:02}", year, month, day);
            let d = parse(&val).unwrap();
            prop_assert_eq!(d.display.as_str(), val.as_str());
            let reparse = parse(d.display.as_str()).unwrap();
            prop_assert_eq!(reparse, d);
        }
    }
}
