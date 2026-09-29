//! Resolved model types: events after handle resolution.
//!
//! [`PersonDisplay`] is a person as the subject of one event — the
//! displayable name plus their per-event role. [`ResolvedEvent`] is an
//! event linked to its subjects and place path by
//! [`crate::resolve::collect_events`].
//!
//! Derived values (elapsed years, age at event, anniversary keys, leap-day
//! folding) and the `ReportOptions` filtering pipeline are the milestone-7
//! unit; they are deliberately absent here so this module stays the
//! resolution contract alone.

use gramps_dates::GrampsDate;
use jiff::civil::Date;

/// A person as the subject of a specific event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonDisplay {
    /// The person's Gramps id (`id="I0000"`), when the exporter set one.
    pub gramps_id: Option<String>,
    /// The person's `handle` — the stable key subject dedup relies on.
    pub handle: String,
    /// The display name, rendered `First Surname` from the primary
    /// (first non-alternate) name (plan §8.8), surname prefix included.
    /// Orphan-placeholder subjects render as `"—"`; a person with no name
    /// renders as an empty string.
    pub name: String,
    /// The subject's role on this event: `"Primary"` for primary-role
    /// people, their eventref role for any-role people, the family
    /// eventref role (e.g. `"Family"`) for couple members, `""` for the
    /// orphan placeholder.
    pub role: String,
}

/// An event resolved against its database — [`crate::resolve::collect_events`]
/// output, one per `<event>` record, in document order.
///
/// An event referenced by several people carries one [`PersonDisplay`] per
/// person; the views expand that into one row per (event, subject) later
/// (plan §8.6 dedup rule).
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedEvent {
    /// The `<type>` element verbatim — `"Birth"`, `"Marriage"`, or a
    /// user-defined type; never empty in practice.
    pub event_type: String,
    /// The parsed Gramps date, when the event carries one of the four date
    /// elements; `None` for undated events (plan §8 rule 13).
    pub date: Option<GrampsDate>,
    /// The normalized proleptic-Gregorian start date (`date.start()`);
    /// `None` when the date is not normalizable — partial dates, unknown
    /// years, non-converted calendars (D4), undated events.
    pub gregorian: Option<Date>,
    /// The resolved subjects in emergence order: primary-role people first;
    /// any-role people; family couples as father-then-mother; a lone
    /// orphan placeholder `"—"`.
    pub subjects: Vec<PersonDisplay>,
    /// The event-level role of its subjects — `"Primary"` when any primary
    /// ref exists, otherwise the first referring person's role, the first
    /// family eventref's role for couples, `""` for orphans.
    pub role: String,
    /// The event's place followed by its `placeref` parent chain ascending,
    /// e.g. `["Ur", "England"]`; `None` when the event carries no place
    /// handle or the handle does not resolve.
    pub place_path: Option<Vec<String>>,
    /// `priv="1"` on the event record — excluded from default output
    /// (plan §8.7); the include-private filter lands in milestone 7.
    pub private: bool,
    /// True when no person or family references the event (plan §8.6e, D7):
    /// the subject is the `"—"` placeholder and `subjects` has exactly one
    /// entry.
    pub orphan: bool,
}
