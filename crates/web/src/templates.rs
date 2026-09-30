//! The Askama templates (plan §6.4 / decision D1): the full landing page,
//! the `main.html` content partial that the load/reset routes and the
//! views swap in for HTMX, and the four view fragments (`GET
//! /api/events?view=…`) rendered from the shared event-core view
//! builders. Askama HTML-escapes every expression by default — the
//! escaping-regression tests in `tests/api.rs` lock safe rendering of
//! hostile names, types and dates into every fragment.
//!
//! Milestone 14 (the full four-view UI) adds the options-form state
//! ([`TypeToggle`] / [`UiContext`]), the month/day/year aggregate shapes
//! for the three grid/grouped views, the composite [`FragmentRow::line`]
//! rendering the calendar / timeline / calendar-with-years fragments
//! share, and the timeline range-bar geometry ([`TimelineEntry`]): cross-
//! year and year-only ranges render as CSS bars spanning their full
//! start → stop extent (plan §8 rules 10–11 / D11).

use askama::Template;
use event_core::{
    CalendarView, CalendarWithYearsView, EventRow, LeapDayPolicy, ReportOptions, TimelineView,
};

use crate::state::{LoadedSummary, count_event_types};
use gramps_xml::Database;

// ---------------------------------------------------------------------
// Landing page / main content partial
// ---------------------------------------------------------------------

/// The full landing page (`GET /`) — index.html includes `main.html`, so
/// the HTMX swaps and the initial render share one content partial.
#[derive(Debug, Template)]
#[template(path = "index.html")]
pub struct IndexTemplate<'a> {
    /// The loaded file's summary, or `None` for the upload-only page.
    pub loaded: Option<&'a LoadedSummary>,
    /// The current run-option state the options form is pre-filled with.
    pub context: &'a UiContext,
}

/// The main content partial (upload form or loaded-file actions) — the
/// HTMX swap target for `POST /api/load` and `POST /api/reset`.
#[derive(Debug, Template)]
#[template(path = "main.html")]
pub struct MainTemplate<'a> {
    /// The loaded file's summary, or `None` to show the upload form.
    pub loaded: Option<&'a LoadedSummary>,
    /// The current run-option state the options form is pre-filled with.
    pub context: &'a UiContext,
}

/// The options-form state: every control's current value, plus the
/// event-type checkbox list with counts (plan §9 UI layout).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiContext {
    /// The event-type checkboxes, in the deterministic `inspect` order
    /// (count desc, then name — same list the CLI enumerates).
    pub types: Vec<TypeToggle>,
    /// The current reference year (rule 4).
    pub reference_year: i32,
    /// "show orphans" toggle state (D7).
    pub show_orphans: bool,
    /// "include private records" toggle state (§8.7).
    pub include_private: bool,
    /// "living only" toggle state (rule 15).
    pub living_only: bool,
    /// "keep Feb 29 (no fold)" — set when the leap-day policy is `Keep`
    /// (decision D6).
    pub leap_keep: bool,
    /// "deduplicate identical rows" toggle state (rule 6).
    pub dedupe_same: bool,
}

/// One event-type checkbox: the type name, its event count in the loaded
/// database, and whether the current options admit it (rule 14 —
/// exclusion wins via [`ReportOptions::type_allowed`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeToggle {
    /// The verbatim `<type>` string.
    pub event_type: String,
    /// How many events in the loaded database carry this type.
    pub count: usize,
    /// Checked — the current options admit this type.
    pub checked: bool,
}

/// Snapshot the options form's state from the loaded database and run
/// options (deterministic; never depends on hash iteration order).
pub fn ui_context(db: Option<&Database>, opts: &ReportOptions) -> UiContext {
    let mut types: Vec<TypeToggle> = Vec::new();
    if let Some(database) = db {
        for entry in count_event_types(database) {
            types.push(TypeToggle {
                event_type: entry.event_type.clone(),
                count: entry.count,
                checked: opts.type_allowed(&entry.event_type),
            });
        }
    }
    UiContext {
        types,
        reference_year: opts.reference_year,
        show_orphans: opts.show_orphans,
        include_private: opts.include_private,
        living_only: opts.living_only,
        leap_keep: opts.leap_day == LeapDayPolicy::Keep,
        dedupe_same: opts.dedupe_same,
    }
}

// ---------------------------------------------------------------------
// View fragments (plan §9: `GET /api/events?view=list|calendar|timeline|yrcal`)
// ---------------------------------------------------------------------

/// The `GET /api/events?view=list` fragment — one row per `EventRow` in
/// the view's deterministic order (rule 12).
#[derive(Debug, Template)]
#[template(path = "events_fragment.html")]
pub struct EventsFragmentTemplate {
    /// The display-shaped rows (strings precomputed in Rust; the template
    /// still escapes every one of them).
    pub rows: Vec<FragmentRow>,
    /// The run's reference year, shown under the table.
    pub reference_year: i32,
}

/// The `GET /api/events?view=calendar` fragment — the anniversary
/// calendar (rules 1/3): months with entries, each holding its anchored
/// days and the day's rows.
#[derive(Debug, Template)]
#[template(path = "calendar_fragment.html")]
pub struct CalendarFragmentTemplate {
    /// The months that have entries, ascending (1–12 order).
    pub months: Vec<CalendarMonthFrag>,
    /// The run's reference year, shown under the grids.
    pub reference_year: i32,
}

/// One month of the anniversary-calendar fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarMonthFrag {
    /// The English month name (`"March"`), deterministic ASCII.
    pub name: String,
    /// The days that have entries, ascending.
    pub days: Vec<CalendarDayFrag>,
}

/// One day of a month grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarDayFrag {
    /// 1–31.
    pub day: u32,
    /// The rows anchored on this day, sorted by (type, subject, id).
    pub entries: Vec<FragmentRow>,
}

impl CalendarFragmentTemplate {
    /// Shape an anniversary [`event_core::CalendarView`] for the fragment.
    pub fn of(view: &CalendarView, reference_year: i32) -> CalendarFragmentTemplate {
        CalendarFragmentTemplate {
            months: view
                .months
                .iter()
                .map(|month| CalendarMonthFrag {
                    name: MONTH_NAMES[(month.month - 1) as usize].to_string(),
                    days: month
                        .days
                        .iter()
                        .map(|day| CalendarDayFrag {
                            day: day.day,
                            entries: day.entries.iter().map(FragmentRow::of).collect(),
                        })
                        .collect(),
                })
                .collect(),
            reference_year,
        }
    }
}

/// The `GET /api/events?view=timeline` fragment — chronological groups
/// under year headers (rule 10) with the terminal "Undated" group last
/// (rule 13); range rows render as CSS bars spanning start → stop (D11).
#[derive(Debug, Template)]
#[template(path = "timeline_fragment.html")]
pub struct TimelineFragmentTemplate {
    /// The year groups in ascending order; the last (when any) is "Undated".
    pub years: Vec<TimelineYearFrag>,
    /// The run's reference year, shown under the timeline.
    pub reference_year: i32,
}

/// One year group of the timeline fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineYearFrag {
    /// The header year; `None` only on the terminal "Undated" group.
    pub year: Option<i32>,
    /// The group's entries in rule-12 order (undated: rule-13 order).
    pub entries: Vec<TimelineEntry>,
}

impl TimelineFragmentTemplate {
    /// Shape a [`event_core::TimelineView`] for the fragment.
    pub fn of(view: &TimelineView, reference_year: i32) -> TimelineFragmentTemplate {
        TimelineFragmentTemplate {
            years: view
                .years
                .iter()
                .map(|year| TimelineYearFrag {
                    year: year.year,
                    entries: year.rows.iter().map(TimelineEntry::of).collect(),
                })
                .collect(),
            reference_year,
        }
    }
}

/// One timeline entry — either a plain text row or a CSS range bar
/// (precomputed geometry: the template never does arithmetic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineEntry {
    /// The composite `"Name — Type (date) · … · elapsed"` line.
    pub line: String,
    /// True when this entry renders as a bar (a `daterange`/`datespan`).
    pub is_bar: bool,
    /// The bar's CSS classes: `"bar"`, or `"bar bar-spans"` when the bar
    /// runs past this year's group or is a year-only full-width bar.
    pub bar_class: String,
    /// The bar's `left` style value (e.g. `"50%"`).
    pub bar_left: String,
    /// The bar's `width` style value.
    pub bar_width: String,
    /// Optional trailing note text (e.g. the stop year of a cross-year
    /// range, `"→ 1823"`).
    pub bar_note: Option<String>,
}

impl TimelineEntry {
    /// Shape one row: a plain entry, or a bar when the row carries a
    /// range/span extent (rules 2/10, D11).
    pub fn of(row: &EventRow) -> TimelineEntry {
        let line = combined_line(row);
        if !row.date_is_range {
            return TimelineEntry {
                line,
                is_bar: false,
                bar_class: String::new(),
                bar_left: String::new(),
                bar_width: String::new(),
                bar_note: None,
            };
        }
        let (left, width, spans, note) = range_bar_geometry(row);
        TimelineEntry {
            line,
            is_bar: true,
            bar_class: if spans {
                "bar bar-spans".to_string()
            } else {
                "bar".to_string()
            },
            bar_left: left,
            bar_width: width,
            bar_note: note,
        }
    }
}

/// The `GET /api/events?view=yrcal` fragment — the calendar-with-years
/// grid (rule 11 / D12): a year-by-year month grid; rows sit on their
/// actual date cells, year-only rows list under `full_year` spanning the
/// whole year, and range rows carry their full extent text.
#[derive(Debug, Template)]
#[template(path = "yrcal_fragment.html")]
pub struct YrcalFragmentTemplate {
    /// The grid's years in ascending order (a contiguous span).
    pub years: Vec<YrcalYearFrag>,
    /// The run's reference year, shown under the grid.
    pub reference_year: i32,
}

/// One year page of the calendar-with-years fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YrcalYearFrag {
    /// The year.
    pub year: i32,
    /// The year-only rows (dates/ranges without a month) — rendered as
    /// bars spanning the whole year (rule 11).
    pub full_year: Vec<FragmentRow>,
    /// The months with entries, ascending.
    pub months: Vec<YrcalMonthFrag>,
}

/// One month cell of a year page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YrcalMonthFrag {
    /// The English month name.
    pub name: String,
    /// The days with entries, ascending.
    pub days: Vec<YrcalDayFrag>,
}

/// One day cell of a year page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YrcalDayFrag {
    /// 1–31.
    pub day: u32,
    /// The rows sitting on this date cell in rule-12 order.
    pub entries: Vec<FragmentRow>,
}

impl YrcalFragmentTemplate {
    /// Shape a [`event_core::CalendarWithYearsView`] for the fragment.
    pub fn of(view: &CalendarWithYearsView, reference_year: i32) -> YrcalFragmentTemplate {
        YrcalFragmentTemplate {
            years: view
                .years
                .iter()
                .map(|year| YrcalYearFrag {
                    year: year.year,
                    full_year: year.full_year.iter().map(FragmentRow::of).collect(),
                    months: year
                        .months
                        .iter()
                        .map(|month| YrcalMonthFrag {
                            name: MONTH_NAMES[(month.month - 1) as usize].to_string(),
                            days: month
                                .days
                                .iter()
                                .map(|day| YrcalDayFrag {
                                    day: day.day,
                                    entries: day.entries.iter().map(FragmentRow::of).collect(),
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
            reference_year,
        }
    }
}

// ---------------------------------------------------------------------
// Row shaping shared by every fragment
// ---------------------------------------------------------------------

/// One display row, used by all four fragments — the list table's columns
/// plus the composite [`FragmentRow::line`] the grid/grouped views render,
/// and the `is_range` flag the grids style (§8 rules 2/10). Markup is
/// never trusted: every field is emitted through Askama's default HTML
/// escaping.
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
    /// The composite `"Name — Type (date) · place · age · elapsed"` line
    /// (the same convention the CLI text and PDF renderers use).
    pub line: String,
    /// True when the row is a `daterange`/`datespan` of any quality.
    pub is_range: bool,
}

impl FragmentRow {
    /// Shape an [`EventRow`] for the fragments.
    pub fn of(row: &EventRow) -> FragmentRow {
        FragmentRow {
            person_name: row.person_name.clone(),
            event_type: row.event_type.clone(),
            date_text: row.event_date_text.clone(),
            place: row.place.clone().unwrap_or_default(),
            elapsed: elapsed_label(row.elapsed_years),
            flags: flags_of(row),
            line: combined_line(row),
            is_range: row.date_is_range,
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

/// The composite row line in the `Name — Type (date) · … · elapsed`
/// convention — the same shape the CLI text renderer and the PDF use
/// (plan §6.3 alt. A), flagging the Feb 29 fold (D6), the place, the age
/// at the event and privacy where present.
fn combined_line(row: &EventRow) -> String {
    let mut line = format!(
        "{} — {} ({})",
        row.person_name, row.event_type, row.event_date_text
    );
    if row.leap_day_folded {
        line.push_str(" · Feb 29");
    }
    if let Some(place) = &row.place {
        line.push_str(&format!(" · {place}"));
    }
    if let Some(age) = &row.age_at_event {
        line.push_str(&format!(" · age {age}"));
    }
    if row.private {
        line.push_str(" · private");
    }
    line.push_str(" · ");
    line.push_str(&elapsed_word(row.elapsed_years));
    line
}

/// The plan's elapsed wording for the composite line (§8.4): unknown and
/// negative render `"—"`, zero renders `"this year"`, otherwise
/// `"{n} year(s)"`.
fn elapsed_word(elapsed: Option<i32>) -> String {
    match elapsed {
        None => "—".to_string(),
        Some(years) if years < 0 => "—".to_string(),
        Some(0) => "this year".to_string(),
        Some(1) => "1 year".to_string(),
        Some(years) => format!("{years} years"),
    }
}

/// English month names for the calendar grids (deterministic, ASCII —
/// matches the CLI text renderer's list).
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

// ---------------------------------------------------------------------
// Timeline range-bar geometry (plan §8 rules 2/10, D11)
// ---------------------------------------------------------------------

/// Compute a range bar's geometry from its row: `(left, width, spans,
/// note)` where `left`/`width` are CSS percentages of the year track,
/// `spans` marks bars running past the group's year (or year-only
/// full-year bars), and `note` carries the stop-year suffix of a
/// cross-year range.
///
/// Month-level precision: a date's fraction of its year is
/// `((month-1)·31 + (day-1)) / 372`. A range whose stop lies beyond the
/// start year stretches from its start to the group's end (the next year
/// group carries the rest); a year-only range (no month) is a full-width
/// bar (rule 10). A range whose stop cannot be parsed degrades to the
/// same stretched bar.
fn range_bar_geometry(row: &EventRow) -> (String, String, bool, Option<String>) {
    let (Some(start_year), Some(start_month)) = (row.year, row.month) else {
        // No start year at all — nothing to place the bar against.
        return ("0%".to_string(), "100%".to_string(), true, None);
    };
    let start = start_fraction(start_month, row.day.unwrap_or(1));
    match (
        row.event_date_stop.as_ref().and_then(|iso| parse_iso(iso)),
        start_year,
    ) {
        (Some((stop_year, _, _)), _) if stop_year > start_year => (
            pct(start),
            pct(1.0 - start),
            true,
            Some(format!("→ {stop_year}")),
        ),
        (Some((_, stop_month, stop_day)), _) => {
            let stop = start_fraction(stop_month, stop_day);
            let mut width = stop - start;
            if width < 0.03 {
                width = 0.03; // keep sub-month ranges visible
            }
            (pct(start), pct(width), false, None)
        }
        _ => (pct(start), pct(1.0 - start), true, None),
    }
}

/// The fraction of a year a (month, day) occupies — a linear 0..1 model
/// across 372 month-slots, so month-only dates land on their month.
fn start_fraction(month: u32, day: u32) -> f64 {
    let slots = ((month - 1) * 31 + (day - 1)) as f64;
    slots / 372.0
}

/// A CSS percentage (`"8%"`) from a 0..1 fraction (integer precision —
/// a month is ~8%).
fn pct(fraction: f64) -> String {
    let percent = (fraction * 100.0) as i32;
    format!("{percent}%")
}

/// Parse an ISO date string (`"1914-07-28"`) into its components. `None`
/// for anything else (negative/empty components, malformed input) — the
/// caller degrades such ranges to stretched bars rather than failing.
fn parse_iso(value: &str) -> Option<(i32, u32, u32)> {
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    match (
        parts[0].parse::<i32>().ok(),
        parts[1].parse::<u32>().ok(),
        parts[2].parse::<u32>().ok(),
    ) {
        (Some(year), Some(month), Some(day)) => Some((year, month, day)),
        _ => None,
    }
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
        assert!(frag.is_range);
        assert_eq!(
            frag.line,
            "A B — Birth (2000-03-03) · Feb 29 · Ur · private · 26 years"
        );

        row.private = false;
        row.leap_day_folded = false;
        row.date_is_range = false;
        let frag = FragmentRow::of(&row);
        assert_eq!(frag.flags, "");
        assert!(!frag.is_range);
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
        assert_eq!(frag.line, "— — Birth () · —");
    }

    #[test]
    fn range_bar_geometry_spans_the_full_extent() {
        // Cross-year range: starts at July 1914, runs to 1918 — the bar
        // stretches from July to the group's end and notes the stop year.
        let cross = EventRow {
            person_id: None,
            person_name: "—".into(),
            event_id: None,
            event_type: "Marriage".into(),
            event_date: Some("1914-07-28".into()),
            event_date_text: "1914-07-28 - 1918-11-11".into(),
            event_date_stop: Some("1918-11-11".into()),
            date_is_range: true,
            year: Some(1914),
            month: Some(7),
            day: Some(28),
            anniversary_month: Some(7),
            anniversary_day: Some(28),
            leap_day_folded: false,
            place: None,
            role: String::new(),
            age_at_event: None,
            reference_year: 2030,
            elapsed_years: None,
            private: false,
        };
        let (left, width, spans, note) = range_bar_geometry(&cross);
        assert_eq!(left, "57%", "July 28 is ~57% of the year");
        assert_eq!(width, "42%");
        assert!(spans);
        assert_eq!(note, Some("→ 1918".to_string()));

        // Same-year range: positioned from its start, sized to its stop.
        let same = EventRow {
            year: Some(1850),
            month: Some(1),
            day: Some(1),
            event_date: Some("1850-01-01".into()),
            event_date_stop: Some("1850-06-30".into()),
            event_date_text: "1850-01-01 - 1850-06-30".into(),
            ..cross.clone()
        };
        let (left, width, spans, note) = range_bar_geometry(&same);
        assert_eq!(left, "0%");
        assert!(width.ends_with("%"));
        assert!(!spans);
        assert_eq!(note, None);

        // Year-only range: a full-year-width bar.
        let year_only = EventRow {
            year: Some(1822),
            month: None,
            day: None,
            event_date: Some("1822-00-00".into()),
            event_date_stop: Some("1824-00-00".into()),
            event_date_text: "1822-00-00 - 1824-00-00".into(),
            ..EventRow {
                person_id: None,
                person_name: "—".into(),
                event_id: None,
                event_type: "Marriage".into(),
                event_date: None,
                event_date_text: String::new(),
                event_date_stop: None,
                date_is_range: true,
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
            }
        };
        let (left, width, spans, note) = range_bar_geometry(&year_only);
        assert_eq!(left, "0%");
        assert_eq!(width, "100%");
        assert!(spans);
        assert_eq!(note, None);
    }

    #[test]
    fn non_range_rows_are_plain_entries() {
        let mut row = EventRow {
            person_id: None,
            person_name: "Ada Dates".into(),
            event_id: None,
            event_type: "Birth".into(),
            event_date: Some("2000-03-03".into()),
            event_date_text: "2000-03-03".into(),
            event_date_stop: None,
            date_is_range: false,
            year: Some(2000),
            month: Some(3),
            day: Some(3),
            anniversary_month: Some(3),
            anniversary_day: Some(3),
            leap_day_folded: false,
            place: None,
            role: "Primary".into(),
            age_at_event: None,
            reference_year: 2026,
            elapsed_years: Some(26),
            private: false,
        };
        let entry = TimelineEntry::of(&row);
        assert!(!entry.is_bar);
        assert_eq!(entry.line, "Ada Dates — Birth (2000-03-03) · 26 years");

        row.date_is_range = true;
        let entry = TimelineEntry::of(&row);
        assert!(entry.is_bar);
        assert_eq!(
            entry.bar_class, "bar bar-spans",
            "undated extent → full bar"
        );
    }

    #[test]
    fn parse_iso_accepts_plain_iso_and_rejects_junk() {
        assert_eq!(parse_iso("1914-07-28"), Some((1914, 7, 28)));
        assert_eq!(parse_iso("1918-11-11"), Some((1918, 11, 11)));
        assert_eq!(parse_iso("1914-07"), None);
        assert_eq!(parse_iso("not a date"), None);
        assert_eq!(parse_iso(""), None);
    }
}
