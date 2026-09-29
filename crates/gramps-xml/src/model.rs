//! Typed model of a minimal Gramps database.
//!
//! This is the *skeleton* mirror of the `grampsxml.dtd` sections the v1
//! pipeline consumes: [`Database`] holds the header and the `events`,
//! `people`, `families` and `places` sections, with every primary record
//! keyed by its unique `handle`. Records carry handles, not resolved links
//! — cross-reference resolution (handle → record index, eventref roles,
//! place hierarchies) lives in `event-core` (plan §7.1).
//!
//! Deliberately out of scope here, landing in the full-record milestone:
//! person names, eventrefs, family members, place parent chains, privacy
//! flags and richer `header`/`tags` parsing. Unknown elements and
//! attributes are ignored at parse time (forward compatibility).

use gramps_dates::GrampsDate;

/// A parsed `.gramps` database: the sections the v1 pipeline consumes, plus
/// the warnings raised while decoding them.
///
/// Skeleton scope (container + minimal records): dates are fully wired into
/// [`Event`] via [`GrampsDate`]; every other record field is deliberately
/// minimal until the full-record milestone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Database {
    pub header: Header,
    pub events: Vec<Event>,
    pub people: Vec<Person>,
    pub families: Vec<Family>,
    pub places: Vec<Place>,
    /// Soft parser notes. A record whose *recoverable* field is malformed
    /// (bad attribute value, malformed date, ...) is skipped so the rest of
    /// the file stays readable, and each occurrence is recorded here (plan
    /// §7.1).
    pub warnings: Vec<String>,
}

/// Contents of the `<header>` section — the skeleton keeps just the export
/// stamp; `researcher`, `name-formats`, `tags` and friends land in the
/// full-record milestone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Header {
    /// The `date` attribute of `<header><created .../>` — when the tree was
    /// exported.
    pub created_date: Option<String>,
    /// The `version` attribute of `<created>` — the Gramps release that
    /// exported the file (e.g. `"5.1.6"`).
    pub version: Option<String>,
}

/// An `<event>` record.
///
/// The DTD's four interchangeable date elements (`dateval`, `daterange`,
/// `datespan`, `datestr`) are wired here: [`Event::date`] carries the fully
/// parsed [`GrampsDate`] (modifier, quality, calendar, stop endpoint, display
/// string) or `None` when the event carries no date element at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Unique DTD `ID`; the key every cross-reference points at.
    pub handle: String,
    /// The `id` attribute, e.g. `"E0000"`.
    pub gramps_id: Option<String>,
    /// The `<type>` element verbatim — `"Birth"`, `"Marriage"`, or any
    /// user-defined type (custom types are the rule, not the exception, so
    /// this stays an open string rather than an enum).
    pub event_type: String,
    /// The parsed date element, or `None` for undated events.
    pub date: Option<GrampsDate>,
    /// The `hlink` of the event's `<place>` element, if present.
    pub place_handle: Option<String>,
    /// The `change` attribute — last-modified Unix timestamp.
    pub change: i64,
}

/// A `<person>` record — skeleton: handle + id only. Names, gender,
/// eventrefs and family links land in the full-record milestone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub handle: String,
    pub gramps_id: Option<String>,
}

/// A `<family>` record — skeleton: handle + id only. Spouses, children and
/// family eventrefs land in the full-record milestone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Family {
    pub handle: String,
    pub gramps_id: Option<String>,
}

/// A `<placeobj>` record — skeleton: handle, id, display name and kind.
/// The parent-chain (`placeref`) hierarchy lands in the full-record
/// milestone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub handle: String,
    pub gramps_id: Option<String>,
    /// The `value` attribute of the first `<pname>` child (empty when the
    /// place carries no name element).
    pub name: String,
    /// The `type` attribute (`"Country"`, `"City"`, `"State"`, ...).
    pub place_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The model stays flat and derivable so containers can be compared
    /// end-to-end (plain XML vs gzip vs zip must yield identical databases).
    #[test]
    fn model_types_are_compareable_and_defaultable() {
        let db = Database::default();
        assert_eq!(db, Database::default());
        assert!(db.events.is_empty());
        assert!(db.people.is_empty());
        assert!(db.families.is_empty());
        assert!(db.places.is_empty());
        assert_eq!(db.header, Header::default());
        assert!(db.warnings.is_empty());
    }
}
