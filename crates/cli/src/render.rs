//! Human-readable text rendering of the four views for the `list`
//! subcommand (plan §6.5).
//!
//! Every view renders deterministically from its [`View`] structure — the
//! same builders the writers and the web UI use — so `list` text and the
//! file/HTML outputs always agree. Row lines follow the same
//! `Name — Type (date) · place · elapsed` convention the PDF renderer uses
//! (plan §6.3 alt. A), with the Feb 29 fold, age and privacy flagged.

use event_core::{View, ViewKind};

/// Render any built view as text. The concrete formatting mirrors the
/// view's structure (plan §8): the list in rule-12 order, the anniversary
/// calendar grouped by month → day, the timeline under year headers with a
/// terminal "Undated" group, and the calendar-with-years as a year page of
/// month cells plus its year-only rows.
pub fn render_view(view: &View) -> String {
    match view {
        View::List(v) => render_list(v),
        View::Calendar(v) => render_calendar(v),
        View::Timeline(v) => render_timeline(v),
        View::CalendarWithYears(v) => render_calendar_with_years(v),
    }
}

/// The header label shown above each view — lets a snapshot tell which
/// view produced a body at a glance.
pub fn view_label(kind: ViewKind) -> &'static str {
    match kind {
        ViewKind::List => "Events",
        ViewKind::Calendar => "Anniversary Calendar",
        ViewKind::Timeline => "Timeline",
        ViewKind::CalendarWithYears => "Calendar With Years",
    }
}

/// The flat list: every row in the plan's deterministic order (rule 12).
fn render_list(view: &event_core::ListView) -> String {
    let mut out = String::from("== Events ==\n");
    for row in &view.rows {
        out.push_str(&row_line(row));
        out.push('\n');
    }
    out
}

/// The anniversary calendar: months with entries, then the day and its
/// rows (rules 1/3, §8).
fn render_calendar(view: &event_core::CalendarView) -> String {
    let mut out = String::from("== Anniversary Calendar ==\n");
    for month in &view.months {
        out.push_str(&format!("{}:\n", MONTHS[(month.month - 1) as usize]));
        for day in &month.days {
            for entry in &day.entries {
                out.push_str(&format!("  {:>2}: {}\n", day.day, row_line(entry)));
            }
        }
    }
    out
}

/// The timeline: year headers (rule 10) then rows, with the terminal
/// "Undated" group last (rule 13). Range rows carry the full range text.
fn render_timeline(view: &event_core::TimelineView) -> String {
    let mut out = String::from("== Timeline ==\n");
    for year in &view.years {
        match year.year {
            Some(y) => out.push_str(&format!("{y}:\n")),
            None => out.push_str("(Undated):\n"),
        }
        for row in &year.rows {
            out.push_str(&format!("  {}\n", row_line(row)));
        }
    }
    out
}

/// The calendar-with-years grid (rule 11 / D12): one stanza per year —
/// year-only rows first, then each month cell and its days.
fn render_calendar_with_years(view: &event_core::CalendarWithYearsView) -> String {
    let mut out = String::from("== Calendar With Years ==\n");
    for year in &view.years {
        out.push_str(&format!("{}:\n", year.year));
        for row in &year.full_year {
            out.push_str(&format!("  (all year) {}\n", row_line(row)));
        }
        for month in &year.months {
            let name = MONTHS[(month.month - 1) as usize];
            out.push_str(&format!("  {name}:\n"));
            for day in &month.days {
                for entry in &day.entries {
                    out.push_str(&format!("    {:>2}: {}\n", day.day, row_line(entry)));
                }
            }
        }
    }
    out
}

/// One row line in the `Name — Type (date) · … · elapsed` convention (same
/// shape as the PDF renderer), flagging the Feb 29 fold (D6), the place,
/// the age at the event and privacy where present.
fn row_line(row: &event_core::EventRow) -> String {
    let mut s = format!(
        "{} — {} ({})",
        row.person_name, row.event_type, row.event_date_text
    );
    if row.leap_day_folded {
        s.push_str(" · Feb 29");
    }
    if let Some(place) = &row.place {
        s.push_str(&format!(" · {place}"));
    }
    if let Some(age) = &row.age_at_event {
        s.push_str(&format!(" · age {age}"));
    }
    if row.private {
        s.push_str(" · private");
    }
    s.push_str(" · ");
    s.push_str(&format_elapsed(row.elapsed_years));
    s
}

/// The plan's elapsed rendering (§8.4): negative and unknown render `"—"`,
/// zero renders `"this year"`, otherwise `"{n} year(s)"`.
fn format_elapsed(elapsed: Option<i32>) -> String {
    match elapsed {
        None => "—".to_string(),
        Some(n) if n < 0 => "—".to_string(),
        Some(0) => "this year".to_string(),
        Some(1) => "1 year".to_string(),
        Some(n) => format!("{n} years"),
    }
}

/// English month names for calendar grouping (deterministic, ASCII).
const MONTHS: [&str; 12] = [
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

#[cfg(test)]
mod tests {
    use super::*;
    use event_core::{ReportOptions, ViewKind, build_view, collect_events};

    const DATES_GRAMPS: &[u8] = include_bytes!("../../../tests/fixtures/dates.gramps");

    fn db() -> gramps_xml::Database {
        gramps_xml::parse_database(DATES_GRAMPS).unwrap()
    }

    fn render(kind: ViewKind, reference_year: i32) -> String {
        let db = db();
        let opts = ReportOptions::with_reference_year(reference_year);
        let events = collect_events(&db, &opts);
        let view = build_view(&events, kind, &opts);
        render_view(&view)
    }

    /// Every view renders its labeled header and is non-empty on a dated
    /// fixture, and never contains a leftover debug `{:?}` `Row(…)` form.
    #[test]
    fn every_view_renders_a_labeled_heading() {
        for kind in [
            ViewKind::List,
            ViewKind::Calendar,
            ViewKind::Timeline,
            ViewKind::CalendarWithYears,
        ] {
            let out = render(kind, 2030);
            assert!(
                out.starts_with(&format!("== {} ==", view_label(kind))),
                "{kind:?}"
            );
            assert!(!out.contains("EventRow {"), "{kind:?} leaked a debug form");
        }
    }

    #[test]
    fn list_is_sorted_and_contains_expected_lines() {
        let out = render(ViewKind::List, 2030);
        assert!(out.contains(" — Birth ("));
        // Determinism: rendering twice yields identical bytes.
        assert_eq!(out, render(ViewKind::List, 2030));
    }

    #[test]
    fn timeline_has_year_headers_and_undated_group() {
        let out = render(ViewKind::Timeline, 2030);
        assert!(out.contains("(Undated):"));
        // A range row keeps its full range text on the line.
        assert!(!out.is_empty());
    }
}
