//! The Askama templates (plan §6.4 / decision D1): the full landing page,
//! the `main.html` content partial that `POST /api/load` and
//! `POST /api/reset` swap in for HTMX, and the `GET /api/events` list
//! fragment. Askama HTML-escapes every expression by default — the
//! escaping-regression test in `tests/api.rs` locks safe rendering of
//! hostile names, types and dates into the fragments.

use askama::Template;
use event_core::EventRow;

use crate::state::LoadedSummary;

/// The full landing page (`GET /`) — index.html includes `main.html`, so
/// the HTMX swaps and the initial render share one content partial.
#[derive(Debug, Template)]
#[template(path = "index.html")]
pub struct IndexTemplate<'a> {
    /// The loaded file's summary, or `None` for the upload-only page.
    pub loaded: Option<&'a LoadedSummary>,
}

/// The main content partial (upload form or loaded-file actions) — the
/// HTMX swap target for `POST /api/load` and `POST /api/reset`.
#[derive(Debug, Template)]
#[template(path = "main.html")]
pub struct MainTemplate<'a> {
    /// The loaded file's summary, or `None` to show the upload form.
    pub loaded: Option<&'a LoadedSummary>,
}

/// The `GET /api/events` list fragment — one row per `EventRow` in the
/// view's deterministic order.
#[derive(Debug, Template)]
#[template(path = "events_fragment.html")]
pub struct EventsFragmentTemplate {
    /// The display-shaped rows (strings precomputed in Rust; the template
    /// still escapes every one of them).
    pub rows: Vec<FragmentRow>,
    /// The run's reference year, shown under the table.
    pub reference_year: i32,
}

/// One row of the list fragment — display-shaped fields. Markup is never
/// trusted: every field is emitted through Askama's default HTML escaping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentRow {
    /// The subject's display name (`"—"` for orphans).
    pub person_name: String,
    /// The event `<type>` verbatim.
    pub event_type: String,
    /// The Gramps display date string.
    pub date_text: String,
    /// The place chain (`""` when the event has no resolvable place).
    pub place: String,
    /// The elapsed-years label (rule 4: `"—"` for future, `"this year"`
    /// for zero, else the number).
    pub elapsed: String,
    /// Human flags: `private`, `Feb 29` (D6 fold), `range` (rule 2).
    pub flags: String,
}

impl FragmentRow {
    /// Shape an [`EventRow`] for the fragment table.
    pub fn of(row: &EventRow) -> FragmentRow {
        FragmentRow {
            person_name: row.person_name.clone(),
            event_type: row.event_type.clone(),
            date_text: row.event_date_text.clone(),
            place: row.place.clone().unwrap_or_default(),
            elapsed: elapsed_label(row.elapsed_years),
            flags: flags_of(row),
        }
    }
}

/// The elapsed-years label per plan §8 rule 4: negative (event after the
/// reference year) renders `"—"`, zero renders `"this year"`, otherwise
/// the plain number. `None` (no year) also renders `"—"`.
fn elapsed_label(elapsed: Option<i32>) -> String {
    match elapsed {
        None => "—".to_string(),
        Some(years) if years < 0 => "—".to_string(),
        Some(0) => "this year".to_string(),
        Some(years) => years.to_string(),
    }
}

/// The row's human-readable flags, joined with `", "`: `private` (plan
/// §8.7), `Feb 29` (the D6 fold), `range` (rule 2, daterange/datespan).
fn flags_of(row: &EventRow) -> String {
    let mut flags = Vec::new();
    if row.private {
        flags.push("private");
    }
    if row.leap_day_folded {
        flags.push("Feb 29");
    }
    if row.date_is_range {
        flags.push("range");
    }
    flags.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_labels_follow_rule_4() {
        assert_eq!(elapsed_label(None), "—");
        assert_eq!(elapsed_label(Some(-1)), "—");
        assert_eq!(elapsed_label(Some(0)), "this year");
        assert_eq!(elapsed_label(Some(6)), "6");
    }

    #[test]
    fn fragment_row_flags_are_deterministic() {
        let mut row = EventRow {
            person_id: Some("I0000".into()),
            person_name: "A B".into(),
            event_id: Some("E0000".into()),
            event_type: "Birth".into(),
            event_date: Some("2000-03-03".into()),
            event_date_text: "2000-03-03".into(),
            event_date_stop: None,
            date_is_range: true,
            year: Some(2000),
            month: Some(3),
            day: Some(3),
            anniversary_month: Some(3),
            anniversary_day: Some(3),
            leap_day_folded: true,
            place: Some("Ur".into()),
            role: "Primary".into(),
            age_at_event: None,
            reference_year: 2026,
            elapsed_years: Some(26),
            private: true,
        };
        let frag = FragmentRow::of(&row);
        assert_eq!(
            frag.flags,
            "private, Feb 29, range".to_string(),
            "flags in a fixed order"
        );

        row.private = false;
        row.leap_day_folded = false;
        row.date_is_range = false;
        let frag = FragmentRow::of(&row);
        assert_eq!(frag.flags, "");
    }

    #[test]
    fn fragment_row_maps_place_and_elapsed() {
        let row = EventRow {
            person_id: None,
            person_name: "—".into(),
            event_id: None,
            event_type: "Birth".into(),
            event_date: None,
            event_date_text: String::new(),
            event_date_stop: None,
            date_is_range: false,
            year: None,
            month: None,
            day: None,
            anniversary_month: None,
            anniversary_day: None,
            leap_day_folded: false,
            place: None,
            role: String::new(),
            age_at_event: None,
            reference_year: 2030,
            elapsed_years: None,
            private: false,
        };
        let frag = FragmentRow::of(&row);
        assert_eq!(frag.place, "");
        assert_eq!(frag.elapsed, "—");
    }
}
