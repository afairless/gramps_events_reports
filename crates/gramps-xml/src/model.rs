//! Typed model of a parsed Gramps database.
//!
//! This module mirrors the `grampsxml.dtd` sections the v1 pipeline
//! consumes: [`Database`] holds the header, tags, events, people, families
//! and places sections, with every primary record keyed by its unique
//! `handle`. Records carry handles, not resolved links — cross-reference
//! resolution (handle → record index, eventref roles, place hierarchies)
//! lives in `event-core` (plan §7.1).
//!
//! Full-record scope (plan §12 step 5): person names (multiple names,
//! surname prefix/`prim`), `eventref` roles, family members, the place
//! parent chain, privacy flags on every primary record, and the
//! `header`/`tags` sections. Unknown elements and attributes are ignored at
//! parse time (forward compatibility), so the model deliberately captures
//! only the fields v1 consumes.

use gramps_dates::GrampsDate;

/// A parsed `.gramps` database: the sections the v1 pipeline consumes, plus
/// the warnings raised while decoding them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Database {
    pub header: Header,
    /// The `<tags>` section: named, colored markers that records point at
    /// via `tagref` links. The tag→record linkage is resolved later, in
    /// `event-core`.
    pub tags: Vec<Tag>,
    pub events: Vec<Event>,
    pub people: Vec<Person>,
    pub families: Vec<Family>,
    pub places: Vec<Place>,
    /// Soft parser notes. A record whose *recoverable* field is malformed
    /// (bad attribute value, malformed date, ...) is skipped so the rest of
    /// the file stays readable, and each occurrence is recorded here (plan
    /// §7.1).
    pub warnings: Vec<String>,
    /// Structured records of the events skipped for a malformed date
    /// element, in document order (plan §7.1 as amended: reversed
    /// `daterange`/`datespan` endpoints are no longer a hard error). Every
    /// entry also has a human message in [`Database::warnings`]; the
    /// structured form is what the error report and web UI consume.
    pub date_issues: Vec<DateIssue>,
}

/// One event omitted because a date element failed to parse.
///
/// A malformed date element (`dateval`, `daterange`, `datespan` or
/// `datestr`) never aborts the parse: the event is skipped, a human message
/// is appended to [`Database::warnings`], and this structured record is
/// pushed onto [`Database::date_issues`] so downstream consumers can render
/// an error report without re-parsing warning strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateIssue {
    /// The `<event>` handle.
    pub event_handle: String,
    /// The Gramps id (`id="E0000"`); falls back to the handle when the
    /// attribute is absent.
    pub event_id: String,
    /// The `<type>` text, when it had been read before the event was
    /// skipped — empty when the file omits `<type>` entirely.
    pub event_type: String,
    /// The date element tag that failed: `dateval` | `daterange` |
    /// `datespan` | `datestr`.
    pub date_kind: String,
    /// The [`gramps_dates::DateError`] display string, e.g.
    /// `daterange/datespan stop "1914" sorts before start "1918"`.
    pub message: String,
}

/// Contents of the `<header>` section: the `<created>` stamp and, when
/// present, the exporter's `<researcher>` name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Header {
    /// The `date` attribute of `<header><created .../>` — when the tree was
    /// exported.
    pub created_date: Option<String>,
    /// The `version` attribute of `<created>` — the Gramps release that
    /// exported the file (e.g. `"5.1.6"`).
    pub version: Option<String>,
    /// The `<resname>` text of the `<researcher>` section — who exported
    /// the tree.
    pub researcher_name: Option<String>,
}

/// A `<tag>` record — a named marker color from the `<tags>` section.
///
/// Records reference tags via `tagref` links; the tag→record linkage is
/// resolved later, in `event-core`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// Unique DTD `ID`; the key `tagref` links point at.
    pub handle: String,
    /// The `name` attribute, e.g. `"ToDo"`.
    pub name: String,
    /// The `color` attribute as Gramps stores it (e.g. `"#fb9408"`).
    pub color: String,
    /// The `priority` attribute — lower sorts first in Gramps' UI.
    pub priority: i32,
    /// The `change` attribute — last-modified Unix timestamp.
    pub change: i64,
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
    /// The `<cause>` element text, if present.
    pub cause: Option<String>,
    /// The `<description>` element text, if present.
    pub description: Option<String>,
    /// `priv="1"` — excluded from default output (plan §8.7).
    pub private: bool,
    /// The `change` attribute — last-modified Unix timestamp.
    pub change: i64,
}

/// The gender marker of a `<person>` — the DTD's `gender` element holds
/// `M`, `F`, or `U`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gender {
    /// `M`
    Male,
    /// `F`
    Female,
    /// `U` — unknown/undisclosed.
    Unknown,
}

/// A surname within a [`PersonName`] — the DTD's repeated `surname*`
/// elements, each carrying an optional `prefix` (e.g. `"van der"`) and a
/// `prim` flag marking the primary surname of the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Surname {
    /// The surname text.
    pub value: String,
    /// The `prefix` attribute (e.g. `"van"` in "van Beethoven").
    pub prefix: Option<String>,
    /// The `prim` attribute: `"1"` marks the primary surname of the name.
    pub prim: bool,
}

/// A `<name>` element of a [`Person`]: the primary name plus any alternates
/// (married names, "also known as" names, ...), in document order. The
/// first non-`alt` name is the person's primary display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonName {
    /// The `type` attribute — `"Birth Name"`, `"Married Name"`, ... (empty
    /// when absent).
    pub name_type: String,
    /// True when the `alt` attribute is `"1"` — an alternate name.
    pub alt: bool,
    /// The `<first>` element text.
    pub first: Option<String>,
    /// The `surname*` elements (multiple surnames, with prefix/`prim`).
    pub surnames: Vec<Surname>,
    /// The `<suffix>` element text (e.g. `"Jr."`).
    pub suffix: Option<String>,
}

/// A person's or family's `<eventref>`: a link to an event plus the role the
/// subject plays in it (`"Primary"`, `"Witness"`, `"Family"`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRef {
    /// The `hlink` — the event's handle.
    pub event_handle: String,
    /// The `role` attribute verbatim; empty when the exporter omitted it.
    pub role: String,
}

/// A `<person>` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub handle: String,
    pub gramps_id: Option<String>,
    /// The `<gender>` element — `M`, `F`, or `U` (`Unknown` when the
    /// element is absent).
    pub gender: Gender,
    /// The `name*` elements in document order (primary first).
    pub names: Vec<PersonName>,
    /// The `eventref*` links — (event handle, role) pairs.
    pub event_refs: Vec<EventRef>,
    /// The `childof*` family handles — families this person is a child of.
    pub child_of: Vec<String>,
    /// The `parentin*` family handles — families this person is a parent in.
    pub parent_in: Vec<String>,
    /// `priv="1"` — excluded from default output (plan §8.7).
    pub private: bool,
}

/// A `<family>` record.
///
/// Marriage, divorce and separation events hang off the family's
/// `eventref*` links rather than off either spouse (§3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Family {
    pub handle: String,
    pub gramps_id: Option<String>,
    /// The `<rel type=.../>` relationship kind, if present.
    pub rel: Option<String>,
    /// The `father` person handle, if present.
    pub father: Option<String>,
    /// The `mother` person handle, if present.
    pub mother: Option<String>,
    /// The `childref*` person handles, in document order.
    pub children: Vec<String>,
    /// The `eventref*` links — (event handle, role) pairs.
    pub event_refs: Vec<EventRef>,
    /// `priv="1"` — excluded from default output (plan §8.7).
    pub private: bool,
}

/// A `<placeobj>` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub handle: String,
    pub gramps_id: Option<String>,
    /// The `value` attribute of the first `<pname>` child (empty when the
    /// place carries no name element).
    pub name: String,
    /// The `type` attribute (`"Country"`, `"City"`, `"State"`, ...).
    pub place_type: Option<String>,
    /// The `hlink` of the first `<placeref>` child — the parent place in
    /// the place hierarchy, if present.
    pub parent_handle: Option<String>,
    /// `priv="1"` — excluded from default output (plan §8.7).
    pub private: bool,
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
        assert!(db.tags.is_empty());
        assert!(db.events.is_empty());
        assert!(db.people.is_empty());
        assert!(db.families.is_empty());
        assert!(db.places.is_empty());
        assert_eq!(db.header, Header::default());
        assert!(db.warnings.is_empty());
        assert!(db.date_issues.is_empty());
    }
}
