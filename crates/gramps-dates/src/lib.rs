//! gramps-dates — the Gramps date model.
//!
//! `GrampsDate` plus parsing of all four date element forms (`dateval`,
//! `daterange`, `datespan`, `datestr`): modifiers, quality, calendars,
//! dual dating, new-year starts, partial and BC dates, and Gregorian
//! normalization. Scaffolded in milestone 1; the type and parsers ship
//! in later milestones.

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
