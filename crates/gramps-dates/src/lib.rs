//! gramps-dates — the Gramps date model.
//!
//! Gramps stores dates in four interchangeable XML element forms (`dateval`,
//! `daterange`, `datespan`, `datestr`). This crate owns the typed model they
//! all map onto — [`GrampsDate`] plus its component enums [`Calendar`],
//! [`Modifier`], [`Quality`], and [`NewYear`] — and the parsers for those
//! forms:
//!
//! - [`parse::parse_dateval`] — the `dateval` element: ISO-ish `YYYY[-MM[-DD]]`
//!   values with `before`/`after`/`about` modifiers, quality, calendar, dual
//!   dating and new-year starts.
//! - [`parse::parse_daterange`], [`parse::parse_datespan`] and
//!   [`parse::parse_datestr`] — the compound `daterange`/`datespan` elements
//!   (start/stop endpoints, quality, calendar, dual dating, new-year starts)
//!   and the text-only `datestr` element.
//! - [`crate::display`] — the Gramps `Date.__str__` display-string formatter;
//!   every parser stamps the computed string onto `GrampsDate::display`.
//! - [`crate::convert`] — Gregorian/Julian SDN normalization into jiff civil
//!   dates ([`GrampsDate::to_gregorian`], [`GrampsDate::start`],
//!   [`GrampsDate::stop`], [`GrampsDate::anniversary_key`]); the other five
//!   calendars keep their stored text with a one-time warning (decision D4).
//!
//! **Partial-date convention.** Missing date components are stored as `0`
//! rather than `Option`s: `(year, 0, 0)` is year-only, `(year, month, 0)` is
//! year-month, `(year, month, day)` is full. BC years are negative.

pub mod convert;
pub mod display;
pub mod model;
pub mod parse;

pub use model::{Calendar, DateError, GrampsDate, Modifier, NewYear, Quality};
pub use parse::{
    DateRangeAttrs, DatevalAttrs, parse_daterange, parse_datespan, parse_datestr, parse_dateval,
};

/// Canonical crate name — smoke tests (and later workspace integration
/// tests) use this to assert crate linkage.
pub const CRATE_NAME: &str = "gramps-dates";

#[cfg(test)]
mod tests {
    use crate::CRATE_NAME;

    #[test]
    fn smoke_crate_builds() {
        assert_eq!(CRATE_NAME, "gramps-dates");
    }
}
