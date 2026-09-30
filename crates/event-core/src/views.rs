//! The view builders — plan §7.3, §8.
//!
//! [`build_view`] (and its [`view`] alias) turns a filtered, derived
//! [`ResolvedEvent`] stream into one of the plan's views; [`rows`] flattens
//! any view back to the [`EventRow`] contract the writers serialize.
//!
//! Milestone 8 ships the **list** and the **anniversary calendar**; the two
//! year-based views ([`ViewKind`]'s future `Timeline` and
//! `CalendarWithYears` variants) arrive in milestone 9.
//!
//! ```text
//!  ResolvedEvent stream ──▶ expand: one EventRow per (event, subject)
//!                                (couples expand to one row per spouse,
//!                                 plan §8.6; dedupe_same collapses
//!                                 identical (type, subject, mm-dd) rows)
//!                                ┌───────────────┬──────────────────┐
//!                                ▼               ▼                  ▼
//!                          ListView        CalendarView            (milestone 9
//!                          (rows sorted    (months → days →          views)
//!                           per rule 12)    entries, anchored
//!                                           per rules 1/3, Feb 29
//!                                           folded per D6)
//! ```
//!
//! Ordering follows the determinism contract (plan §8 rule 12): the list
//! sorts by (start date, event type, primary subject, event id) with
//! undated rows terminal (rule 13); calendar days follow
//! (month, day) with entries sorted by (type, subject, event id). Sorting
//! never depends on set or hash iteration order.

use std::collections::HashSet;

use crate::model::{PersonDisplay, ResolvedEvent};
use crate::options::ReportOptions;
use crate::row::{EventRow, format_age};

/// Which of the plan's views to build. Milestone 8 implements `List` and
/// `Calendar`; the year-based `Timeline` / `CalendarWithYears` variants are
/// added by milestone 9.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    /// Flat, sortable table of events (plan §1.6; §8 rule 12 order).
    List,
    /// Recurring anniversary calendar, no years (plan §1.6, §8 rule 1).
    Calendar,
}

/// A built view — the structure the writers and the web UI render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    /// The flat list: every row in deterministic list order.
    List(ListView),
    /// The anniversary calendar: `EventRow`s grouped by their effective
    /// anchor (month → day → entries).
    Calendar(CalendarView),
}

/// The list view — a flat row set in the plan's deterministic order
/// (plan §8 rules 12–13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListView {
    /// Every row, sorted by (start date, event type, primary subject,
    /// event id); undated rows sort after all dated rows (rule 13).
    pub rows: Vec<EventRow>,
}

/// The anniversary calendar view — months (1–12, in order, only months
/// with entries) each holding days (1–31, in order) of anchored rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarView {
    /// The months that have entries, ascending.
    pub months: Vec<CalendarMonth>,
}

/// One month of the anniversary calendar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarMonth {
    /// 1–12.
    pub month: u32,
    /// The days that have entries, ascending.
    pub days: Vec<CalendarDay>,
}

/// One day of the anniversary calendar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarDay {
    /// 1–31.
    pub day: u32,
    /// The rows anchored on this day, sorted by (event type, subject,
    /// event id) — the plan's calendar order (§8 rule 12).
    pub entries: Vec<EventRow>,
}

/// Build one of the plan's views from a resolved, filtered event stream.
///
/// Every event is expanded once per subject (one row per (event, subject),
/// plan §8.6) into [`EventRow`]s stamped with the run's `opts`; the `List`
/// kind sorts them (rule 12), the `Calendar` kind filters to anchored rows
/// and groups them by their effective (month, day) cell (rules 1/3, D6).
pub fn build_view(events: &[ResolvedEvent], kind: ViewKind, opts: &ReportOptions) -> View {
    let rows = expand_rows(events, opts);
    match kind {
        ViewKind::List => View::List(ListView {
            rows: sort_list_rows(rows),
        }),
        ViewKind::Calendar => View::Calendar(build_calendar(rows)),
    }
}

/// Alias of [`build_view`] — the shorthand name the plan's milestone table
/// uses for constructing a view.
pub fn view(events: &[ResolvedEvent], kind: ViewKind, opts: &ReportOptions) -> View {
    build_view(events, kind, opts)
}

/// Flatten any view back to the flat [`EventRow`] contract (plan §7.3).
///
/// The list returns its sorted rows; the calendar returns its entries in
/// calendar order — (month, day, event type, subject, event id), plan §8
/// rule 12.
pub fn rows(view: &View) -> Vec<EventRow> {
    match view {
        View::List(list) => list.rows.clone(),
        View::Calendar(calendar) => {
            let mut out = Vec::new();
            for month in &calendar.months {
                for day in &month.days {
                    out.extend(day.entries.iter().cloned());
                }
            }
            out
        }
    }
}

/// The `dedupe_same` identity — (event type, subject handle, effective
/// anniversary anchor) — the tuple the collapse hash keyed on.
type DedupeKey = (String, String, Option<(u32, u32)>);

/// Expand each event into one [`EventRow`] per subject, applying the
/// `dedupe_same` collapse (plan §8.6): identical (event type, subject,
/// effective anniversary month-day) rows collapse to their first
/// occurrence. Orphan placeholders share the empty handle, so two orphan
/// rows of the same type and month-day also collapse.
fn expand_rows(events: &[ResolvedEvent], opts: &ReportOptions) -> Vec<EventRow> {
    let mut out = Vec::new();
    let mut seen: HashSet<DedupeKey> = HashSet::new();
    for event in events {
        for subject in &event.subjects {
            let row = event_row(event, subject, opts);
            if opts.dedupe_same {
                let anchor = match (row.anniversary_month, row.anniversary_day) {
                    (Some(month), Some(day)) => Some((month, day)),
                    _ => None,
                };
                if !seen.insert((event.event_type.clone(), subject.handle.clone(), anchor)) {
                    continue;
                }
            }
            out.push(row);
        }
    }
    out
}

/// Build one [`EventRow`] for `subject`'s view of `event`, stamped with
/// the run's options (reference year, Feb 29 fold policy).
fn event_row(event: &ResolvedEvent, subject: &PersonDisplay, opts: &ReportOptions) -> EventRow {
    let (year, month, day) = date_components(event);
    let (anniversary_month, anniversary_day) = match event.anniversary {
        None => (None, None),
        Some(anchor) => {
            let (effective, _) = opts.leap_day.fold_anchor(anchor, opts.reference_year);
            (Some(effective.0), Some(effective.1))
        }
    };
    EventRow {
        person_id: subject.gramps_id.clone(),
        person_name: subject.name.clone(),
        event_id: event.event_id.clone(),
        event_type: event.event_type.clone(),
        event_date: event.gregorian.map(|date| date.to_string()),
        event_date_text: event
            .date
            .as_ref()
            .map(|date| date.display.clone())
            .unwrap_or_default(),
        event_date_stop: event
            .date
            .as_ref()
            .and_then(|date| date.stop())
            .map(|date| date.to_string()),
        date_is_range: event
            .date
            .as_ref()
            .map(|date| date.is_range())
            .unwrap_or(false),
        year,
        month,
        day,
        anniversary_month,
        anniversary_day,
        leap_day_folded: event.leap_day_folded,
        place: event.place_path.as_ref().map(|path| path.join(" / ")),
        role: subject.role.clone(),
        age_at_event: event.age_at_event.map(format_age),
        reference_year: opts.reference_year,
        elapsed_years: event.elapsed_years,
        private: event.private,
    }
}

/// The row's (year, month, day) — from the normalized Gregorian start when
/// one exists (so Julian dates report their Gregorian components, matching
/// `event_date`), else from the stored calendar's own components (partial
/// Gregorian dates), else all `None` (undated / text / unknown year).
fn date_components(event: &ResolvedEvent) -> (Option<i32>, Option<u32>, Option<u32>) {
    if let Some(date) = event.gregorian {
        return (
            Some(i32::from(date.year())),
            Some(u32::from(date.month() as u8)),
            Some(u32::from(date.day() as u8)),
        );
    }
    match event.date.as_ref() {
        Some(date) => {
            let (raw_year, raw_month, raw_day) = date.ymd;
            (
                (raw_year != 0).then_some(raw_year),
                (raw_month != 0).then_some(raw_month),
                (raw_day != 0).then_some(raw_day),
            )
        }
        None => (None, None, None),
    }
}

/// Sort the list rows into the plan's deterministic order (rule 12):
/// (start date ascending, event type, primary subject, event id), with
/// undated rows after every dated row (rule 13).
fn sort_list_rows(mut rows: Vec<EventRow>) -> Vec<EventRow> {
    rows.sort_by(|a, b| list_sort_key(a).cmp(&list_sort_key(b)));
    rows
}

/// The list order key — (dated-flag, start date, event type, subject,
/// event id) — as one comparable tuple (rule 12); the trailing strings
/// borrow from the sorted row.
type ListSortKey<'a> = (u8, Option<(i32, u32, u32)>, &'a str, &'a str, &'a str);

fn list_sort_key<'a>(row: &'a EventRow) -> ListSortKey<'a> {
    let date = row
        .year
        .map(|year| (year, row.month.unwrap_or(0), row.day.unwrap_or(0)));
    // Dated rows (0) sort before undated rows (1) — rule 13's terminal
    // "Undated" group.
    (
        if date.is_some() { 0 } else { 1 },
        date,
        row.event_type.as_str(),
        row.person_name.as_str(),
        row.event_id.as_deref().unwrap_or(""),
    )
}

/// Group anchored rows into the anniversary calendar's month → day tree,
/// entries sorted by (event type, subject, event id) within a day (rule 12).
/// Rows without an anchor (year-only dates, non-convertible calendars,
/// undated events) are excluded per rule 1.
fn build_calendar(rows: Vec<EventRow>) -> CalendarView {
    let mut anchored: Vec<EventRow> = rows
        .into_iter()
        .filter(|row| row.anniversary_month.is_some())
        .collect();
    anchored.sort_by(|a, b| calendar_sort_key(a).cmp(&calendar_sort_key(b)));
    let mut months: Vec<CalendarMonth> = Vec::new();
    for row in anchored {
        let month = row.anniversary_month.expect("anchored rows have a month");
        let day = row.anniversary_day.expect("anchored rows have a day");
        if months.last().map(|last| last.month) != Some(month) {
            months.push(CalendarMonth {
                month,
                days: Vec::new(),
            });
        }
        let month_entry = months.last_mut().expect("pushed above");
        if month_entry.days.last().map(|last| last.day) != Some(day) {
            month_entry.days.push(CalendarDay {
                day,
                entries: Vec::new(),
            });
        }
        month_entry
            .days
            .last_mut()
            .expect("pushed above")
            .entries
            .push(row);
    }
    CalendarView { months }
}

fn calendar_sort_key(row: &EventRow) -> (u32, u32, &str, &str, &str) {
    (
        row.anniversary_month.unwrap_or(0),
        row.anniversary_day.unwrap_or(0),
        row.event_type.as_str(),
        row.person_name.as_str(),
        row.event_id.as_deref().unwrap_or(""),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect_events;
    use crate::options::LeapDayPolicy;
    use gramps_xml::parse_database;

    const DATA_GRAMPS: &[u8] = include_bytes!("../../../tests/fixtures/data.gramps");
    const DATES_GRAMPS: &[u8] = include_bytes!("../../../tests/fixtures/dates.gramps");
    const FAMILIES_GRAMPS: &[u8] = include_bytes!("../../../tests/fixtures/families.gramps");

    /// A hand-written database exercising the view rules the committed
    /// fixtures do not: a Feb 29 birth (D6 fold), a month-only date (rule
    /// 3), a year-only date (rule 1 exclusion), a year-only range, a
    /// month-only range, a text and an undated event, a private event, a
    /// couple with a role, and two identical rows for dedupe_same.
    const VIEWS_XML: &[u8] = br#"<database>
  <events>
    <event handle="_v0" id="V0" change="1">
      <type>Birth</type>
      <dateval val="2000-02-29"/>
    </event>
    <event handle="_v1" id="V1" change="1">
      <type>Birth</type>
      <dateval val="1995-04"/>
    </event>
    <event handle="_v2" id="V2" change="1">
      <type>Birth</type>
      <dateval val="1990"/>
    </event>
    <event handle="_v3" id="V3" change="1">
      <type>Marriage</type>
      <daterange start="1988" stop="1989"/>
    </event>
    <event handle="_v4" id="V4" change="1">
      <type>Immigration</type>
      <daterange start="1998-03" stop="1998-05"/>
    </event>
    <event handle="_v5" id="V5" change="1">
      <type>Graduation</type>
      <datestr val="sometime"/>
    </event>
    <event handle="_v6" id="V6" change="1">
      <type>Party</type>
    </event>
    <event handle="_v7" id="V7" change="1" priv="1">
      <type>Secret</type>
      <dateval val="2001-01-01"/>
    </event>
    <event handle="_v8" id="V8" change="1">
      <type>Birth</type>
      <dateval val="1970-05-05"/>
    </event>
    <event handle="_v9" id="V9" change="1">
      <type>Birth</type>
      <dateval val="1970-05-05"/>
    </event>
  </events>
  <people>
    <person handle="_vp0" id="P0">
      <name type="Birth Name">
        <first>Zed</first>
        <surname>Fold</surname>
      </name>
      <eventref hlink="_v0" role="Primary"/>
      <eventref hlink="_v1" role="Primary"/>
      <eventref hlink="_v2" role="Primary"/>
      <eventref hlink="_v4" role="Witness"/>
    </person>
    <person handle="_vp1" id="P1">
      <name type="Birth Name">
        <first>Ada</first>
        <surname>Dates</surname>
      </name>
      <eventref hlink="_v8" role="Primary"/>
      <eventref hlink="_v9" role="Primary"/>
    </person>
  </people>
  <families>
    <family handle="_vf0" id="F0">
      <father hlink="_vp0"/>
      <mother hlink="_vp1"/>
      <eventref hlink="_v3" role="Family"/>
    </family>
  </families>
</database>"#;

    fn run_opts() -> ReportOptions {
        ReportOptions::with_reference_year(2026)
    }

    /// Build the rows for `xml` under `opts`, in list order.
    fn list_rows<'a>(xml: &'a [u8], opts: &'a ReportOptions) -> Vec<EventRow> {
        let db = parse_database(xml).unwrap();
        let events = collect_events(&db, opts);
        rows(&view(&events, ViewKind::List, opts))
    }

    /// Build the calendar entries for `xml` under `opts`, in calendar
    /// order.
    fn calendar_rows<'a>(xml: &'a [u8], opts: &'a ReportOptions) -> Vec<EventRow> {
        let db = parse_database(xml).unwrap();
        let events = collect_events(&db, opts);
        rows(&view(&events, ViewKind::Calendar, opts))
    }

    /// The compact row fingerprint for golden tables — every contract
    /// field except the non-nullable display constants covered by the
    /// full-row goldens below.
    type Fp = (
        Option<String>,
        String,
        String,
        Option<String>,
        Option<String>,
        bool,
        Option<i32>,
        Option<u32>,
        Option<u32>,
        Option<u32>,
        Option<u32>,
        bool,
        Option<i32>,
        String,
    );

    fn fp(row: &EventRow) -> Fp {
        (
            row.event_id.clone(),
            row.person_name.clone(),
            row.event_type.clone(),
            row.event_date.clone(),
            row.event_date_stop.clone(),
            row.date_is_range,
            row.year,
            row.month,
            row.day,
            row.anniversary_month,
            row.anniversary_day,
            row.leap_day_folded,
            row.elapsed_years,
            row.event_date_text.clone(),
        )
    }

    // --- golden: data.gramps (plan §11 fixture; mirrors the Gramps report)
    // ----------------------------------------------------------------------

    #[test]
    fn golden_list_matches_the_full_flat_contract() {
        let opts = run_opts();
        let rows = list_rows(DATA_GRAMPS, &opts);
        assert_eq!(rows.len(), 6);

        // Abraham's death — the full 19-field contract, all fields set.
        let death = &rows[5];
        assert_eq!(
            death,
            &EventRow {
                person_id: Some("I0004".to_string()),
                person_name: "Abraham Meowser".to_string(),
                event_id: Some("E0005".to_string()),
                event_type: "Death".to_string(),
                event_date: Some("2020-12-03".to_string()),
                event_date_text: "2020-12-03".to_string(),
                event_date_stop: None,
                date_is_range: false,
                year: Some(2020),
                month: Some(12),
                day: Some(3),
                anniversary_month: Some(12),
                anniversary_day: Some(3),
                leap_day_folded: false,
                place: Some("Ur".to_string()),
                role: "Primary".to_string(),
                age_at_event: Some("82y 0m".to_string()),
                reference_year: 2026,
                elapsed_years: Some(6),
                private: false,
            }
        );

        // The five births in deterministic list order (rule 12).
        assert_eq!(fp(&rows[0]).0, Some("E0004".to_string())); // 1938
        assert_eq!(fp(&rows[1]).0, Some("E0003".to_string())); // 1946
        assert_eq!(fp(&rows[2]).0, Some("E0001".to_string())); // 1970-08
        assert_eq!(fp(&rows[3]).0, Some("E0002".to_string())); // 1970-12
        assert_eq!(fp(&rows[4]).0, Some("E0000".to_string())); // 2000-03
        assert_eq!(fp(&rows[4]).1, "Harry Meowser");
        assert_eq!(fp(&rows[4]).6, Some(2000));
        assert_eq!(fp(&rows[4]).9, Some(3));
        assert_eq!(fp(&rows[4]).10, Some(3));
        assert_eq!(fp(&rows[4]).12, Some(26));
    }

    #[test]
    fn golden_calendar_places_every_anchor_in_month_day_order() {
        let opts = run_opts();
        let rows = calendar_rows(DATA_GRAMPS, &opts);
        // (month, day) → event ids, in calendar order (rule 12):
        // Jan 22 Mark, Mar 3 Harry, Aug 14 George, Nov 11 + Dec 3 Abraham.
        assert_eq!(
            rows.iter()
                .map(|row| (
                    row.anniversary_month,
                    row.anniversary_day,
                    row.event_id.clone()
                ))
                .collect::<Vec<_>>(),
            vec![
                (Some(1), Some(22), Some("E0003".to_string())),
                (Some(3), Some(3), Some("E0000".to_string())),
                (Some(8), Some(14), Some("E0001".to_string())),
                (Some(11), Some(11), Some("E0004".to_string())),
                (Some(12), Some(3), Some("E0005".to_string())),
                (Some(12), Some(5), Some("E0002".to_string())),
            ],
        );
    }

    #[test]
    fn calendar_view_is_a_month_day_tree() {
        let opts = run_opts();
        let db = parse_database(DATA_GRAMPS).unwrap();
        let events = collect_events(&db, &opts);
        let View::Calendar(calendar) = view(&events, ViewKind::Calendar, &opts) else {
            unreachable!("Calendar kind")
        };
        assert_eq!(
            calendar
                .months
                .iter()
                .map(|month| (month.month, month.days.len()))
                .collect::<Vec<_>>(),
            vec![(1, 1), (3, 1), (8, 1), (11, 1), (12, 2)],
        );
        let december = &calendar.months[4];
        assert_eq!(december.days[0].day, 3);
        assert_eq!(december.days[0].entries[0].person_name, "Abraham Meowser");
        assert_eq!(december.days[1].day, 5);
        assert_eq!(december.days[1].entries[0].person_name, "Sally Furball");
        // rows() flattens the tree back in the same order.
        assert_eq!(rows(&view(&events, ViewKind::Calendar, &opts)).len(), 6);
    }

    // --- golden: dates.gramps (anchor + range rules)
    // ----------------------------------------------------------------------

    #[test]
    fn golden_dates_fixture_list_rows() {
        let opts = run_opts();
        let rows = list_rows(DATES_GRAMPS, &opts);
        assert_eq!(rows.len(), 25);
        let seen = |id: &str| {
            fp(rows
                .iter()
                .find(|r| fp(r).0.as_deref() == Some(id))
                .unwrap())
        };

        // about / before / after / quality modifiers keep their display
        // strings and anchor normally.
        let about = seen("D0003");
        assert_eq!(about.2, "Birth");
        assert_eq!(about.3, Some("1900-01-01".to_string()));
        assert_eq!(about.13, "abt 1900-01-01");
        assert_eq!(about.9, Some(1));
        assert_eq!(about.10, Some(1));
        assert_eq!(about.12, Some(126));
        let before = seen("D0001");
        assert_eq!(before.1, "Second Example");
        assert_eq!(before.13, "bef 1914-06-28");
        assert_eq!(before.12, Some(112));

        // partial: year-only — year known, no month/day, never anchored.
        let year_only = seen("D0006");
        assert_eq!(year_only.3, None);
        assert_eq!(year_only.6, Some(1822));
        assert_eq!(year_only.7, None);
        assert_eq!(year_only.8, None);
        assert_eq!(year_only.9, None);
        assert_eq!(year_only.10, None);

        // partial: year-month — anchors at day 1 (plan rule 3).
        let month_only = seen("D0007");
        assert_eq!(month_only.6, Some(1822));
        assert_eq!(month_only.7, Some(11));
        assert_eq!(month_only.8, None);
        assert_eq!(month_only.9, Some(11));
        assert_eq!(month_only.10, Some(1));
        assert_eq!(month_only.12, Some(204));

        // full range: both endpoints ISO, anchored at the start (§8 rule 2).
        let range = seen("D0017");
        assert_eq!(range.3, Some("1914-07-28".to_string()));
        assert_eq!(range.4, Some("1918-11-11".to_string()));
        assert!(range.5);
        assert_eq!(range.9, Some(7));
        assert_eq!(range.10, Some(28));
        assert_eq!(range.12, Some(112));

        // month-only range endpoints: the start anchors at day 1, the stop
        // has no ISO (no day).
        let month_range = seen("D0018");
        assert_eq!(month_range.3, None);
        assert_eq!(month_range.4, None);
        assert!(month_range.5);
        assert_eq!(month_range.6, Some(1822));
        assert_eq!(month_range.7, Some(11));
        assert_eq!(month_range.8, None);
        assert_eq!(month_range.9, Some(11));
        assert_eq!(month_range.10, Some(1));
        assert_eq!(month_range.12, Some(204));

        // year-only range: flagged a range but never anchored (rule 1).
        let year_range = seen("D0019");
        assert_eq!(year_range.3, None);
        assert!(year_range.5);
        assert_eq!(year_range.9, None);
        assert_eq!(year_range.10, None);

        // BC dates normalize and anchor on their (negative) year.
        let bc = seen("D0008");
        assert_eq!(bc.3, Some("-000550-04-22".to_string()));
        assert_eq!(bc.6, Some(-550));
        assert_eq!(bc.9, Some(4));
        assert_eq!(bc.10, Some(22));
        assert_eq!(bc.12, Some(2576));

        // text-only dates carry the verbatim text, no date, no anchor
        // (rule 13) and sort into the terminal undated group.
        assert_eq!(fp(&rows[24]).0, Some("D0024".to_string()));
        let text = fp(&rows[24]);
        assert_eq!(text.13, "circa the harvest festival");
        assert_eq!(text.3, None);
        assert_eq!(text.9, None);
        assert_eq!(text.12, None);
    }

    #[test]
    fn golden_dates_calendar_excludes_unanchorable_dates() {
        let opts = run_opts();
        let rows = calendar_rows(DATES_GRAMPS, &opts);
        // 25 events − (year-only, yr-only range, five non-converted
        // calendars, text date) = 17 anchored entries.
        assert_eq!(rows.len(), 17);
        let ids = rows
            .iter()
            .map(|row| row.event_id.as_deref().unwrap())
            .collect::<Vec<_>>();
        for present in [
            "D0000", "D0001", "D0002", "D0003", "D0004", "D0005", "D0007", "D0008", "D0009",
            "D0010", "D0016", "D0017", "D0018", "D0020", "D0021", "D0022", "D0023",
        ] {
            assert!(ids.contains(&present), "{present} must anchor");
        }
        for excluded in [
            "D0006", // year-only
            "D0011", "D0012", "D0013", "D0014", "D0015", // non-Gregorian (D4)
            "D0019", // year-only range
            "D0024", // text-only
        ] {
            assert!(!ids.contains(&excluded), "{excluded} must not anchor");
        }
        // Order: the anchor cells ascend (month, day), entries within a day
        // follow (type, subject, id) — so on Nov 1 the Birth (D0007) sorts
        // before the Marriage (D0018), and Nov 11 closes the calendar.
        assert_eq!(ids[0], "D0003"); // Jan 1, before the same-day Marriage
        assert_eq!(ids[14], "D0007");
        assert_eq!(ids[15], "D0018");
        assert_eq!(ids[16], "D0004");
    }

    // --- golden: families.gramps (couples, roles, orphans)
    // ----------------------------------------------------------------------

    #[test]
    fn golden_families_couple_rows_expand_per_subject() {
        let opts = run_opts();
        let rows = list_rows(FAMILIES_GRAMPS, &opts);
        assert_eq!(rows.len(), 12);
        let fp_all = rows.iter().map(fp).collect::<Vec<_>>();
        let ids = fp_all
            .iter()
            .map(|r| r.0.as_deref().unwrap())
            .collect::<Vec<_>>();
        // Deterministic list order (start date, type, subject, id); the
        // undated custom-type orphan closes the list (rule 13).
        assert_eq!(
            ids,
            vec![
                "E0005", "E0006", "E0004", "E0000", "E0000", "E0001", "E0001", "E0002", "E0002",
                "E0003", "E0003", "E0007",
            ]
        );

        // Couples expand to one row per spouse with the couple's role
        // (marriages stay visible despite the later divorce — D5).
        let marriage = &fp_all[3];
        assert_eq!(marriage.0, Some("E0000".to_string()));
        assert_eq!(marriage.1, "Adam Uplands");
        assert_eq!(marriage.2, "Marriage");
        let eve_row = &fp_all[4];
        assert_eq!(eve_row.0, Some("E0000".to_string()));
        assert_eq!(eve_row.1, "Eve Uplands");
        assert_eq!(eve_row.2, "Marriage");

        // The orphan immigration carries the placeholder subject; the
        // text-only custom type is undated and terminal.
        let orphan = &fp_all[1];
        assert_eq!(orphan.0, Some("E0006".to_string()));
        assert_eq!(orphan.1, "—");
        assert!(orphan.5);
        assert_eq!(orphan.9, Some(1));
        assert_eq!(orphan.10, Some(1));

        // Rows carry the same reference year on every row.
        assert!(rows.iter().all(|row| row.reference_year == 2026));
    }

    // --- unit: the milestone deliverable rules (anchors + fold)
    // ----------------------------------------------------------------------

    #[test]
    fn feb_29_anchor_folds_in_the_calendar_and_stays_in_the_row_date() {
        let opts = run_opts();
        let rows = list_rows(VIEWS_XML, &opts);
        let zed_birth = rows
            .iter()
            .find(|row| fp(row).0 == Some("V0".to_string()))
            .unwrap();
        // The true Gregorian date stays in event_date; the anchor lands on
        // Feb 28 and the fold is labeled (D6).
        assert_eq!(zed_birth.event_date, Some("2000-02-29".to_string()));
        assert_eq!(zed_birth.anniversary_month, Some(2));
        assert_eq!(zed_birth.anniversary_day, Some(28));
        assert!(zed_birth.leap_day_folded);
        // It appears under Feb 28 in the calendar.
        assert_eq!(calendar_rows(VIEWS_XML, &opts)[0].anniversary_day, Some(28));

        // A leap reference year keeps Feb 29, unfolded.
        let mut leap = run_opts();
        leap.reference_year = 2024;
        let rows_leap = list_rows(VIEWS_XML, &leap);
        let birth_leap = rows_leap
            .iter()
            .find(|row| fp(row).0 == Some("V0".to_string()))
            .unwrap();
        assert_eq!(birth_leap.anniversary_month, Some(2));
        assert_eq!(birth_leap.anniversary_day, Some(29));
        assert!(!birth_leap.leap_day_folded);

        // The Keep policy never folds.
        let mut keep = run_opts();
        keep.leap_day = LeapDayPolicy::Keep;
        let rows_keep = list_rows(VIEWS_XML, &keep);
        let birth_keep = rows_keep
            .iter()
            .find(|row| fp(row).0 == Some("V0".to_string()))
            .unwrap();
        assert_eq!(birth_keep.anniversary_day, Some(29));
        assert!(!birth_keep.leap_day_folded);
        // ... and both land on Feb 29 under Keep.
        assert_eq!(calendar_rows(VIEWS_XML, &keep)[0].anniversary_day, Some(29));
    }

    #[test]
    fn calendar_excludes_year_only_text_and_undated_rows() {
        let opts = run_opts();
        let rows = list_rows(VIEWS_XML, &opts);
        let pos = |id: &str| {
            rows.iter()
                .position(|row| fp(row).0.as_deref() == Some(id))
                .unwrap()
        };
        // 10 events − 1 private = 9 resolved events, expanded to 10 rows
        // (the couple V3 yields one row per spouse).
        assert_eq!(rows.len(), 10);

        // Year-only, year-only range, text and undated events are in the
        // list but carry no anchor.
        assert_eq!(rows[pos("V2")].anniversary_month, None);
        assert_eq!(rows[pos("V3")].anniversary_month, None);
        assert_eq!(rows[pos("V5")].anniversary_month, None);
        assert_eq!(rows[pos("V6")].anniversary_month, None);

        // The calendar keeps only anchored rows, in (month, day) order:
        // Feb 28 (folded V0), Mar 1 (V4 range start), Apr 1 (V1 month-only),
        // May 5 (V8), May 5 (V9).
        let cal_rows = calendar_rows(VIEWS_XML, &opts);
        let cal = cal_rows
            .iter()
            .map(|row| {
                (
                    row.anniversary_day.unwrap(),
                    row.event_id.as_deref().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            cal,
            vec![(28, "V0"), (1, "V4"), (1, "V1"), (5, "V8"), (5, "V9"),]
        );
    }

    #[test]
    fn month_only_and_month_range_anchor_at_day_one() {
        let opts = run_opts();
        let rows = list_rows(VIEWS_XML, &opts);
        let v1 = rows
            .iter()
            .find(|row| fp(row).0 == Some("V1".to_string()))
            .unwrap();
        assert_eq!((v1.month, v1.day), (Some(4), None));
        assert_eq!(
            (v1.anniversary_month, v1.anniversary_day),
            (Some(4), Some(1))
        );
        let v4 = rows
            .iter()
            .find(|row| fp(row).0 == Some("V4".to_string()))
            .unwrap();
        assert_eq!(
            (v4.anniversary_month, v4.anniversary_day),
            (Some(3), Some(1))
        );
        assert!(v4.date_is_range);
        assert_eq!(v4.elapsed_years, Some(28)); // from the range start year
    }

    #[test]
    fn couple_rows_start_from_family_role_and_split_by_subject() {
        let opts = run_opts();
        let rows = list_rows(VIEWS_XML, &opts);
        let couple: Vec<&EventRow> = rows
            .iter()
            .filter(|row| fp(row).0 == Some("V3".to_string()))
            .collect();
        assert_eq!(couple.len(), 2);
        assert_eq!(couple[0].person_name, "Ada Dates");
        assert_eq!(couple[1].person_name, "Zed Fold");
        assert!(couple.iter().all(|row| row.role == "Family"));
        assert!(couple.iter().all(|row| row.date_is_range));
        assert_eq!(couple[0].elapsed_years, Some(38)); // 2026 − 1988
    }

    #[test]
    fn dedupe_same_collapses_identical_type_subject_anchor_rows() {
        let mut opts = run_opts();
        let plain = list_rows(VIEWS_XML, &opts);
        assert_eq!(plain.len(), 10);
        // Ada's two identical Birth rows (V8, V9 — same type, subject,
        // month-day) both remain by default...
        assert_eq!(fp(&plain[0]).0, Some("V8".to_string()));
        assert_eq!(fp(&plain[1]).0, Some("V9".to_string()));

        // ... and collapse to one under dedupe_same.
        opts.dedupe_same = true;
        let deduped = list_rows(VIEWS_XML, &opts);
        assert_eq!(deduped.len(), 9);
        assert_eq!(
            deduped
                .iter()
                .filter(|row| fp(row).0 == Some("V8".to_string()))
                .count(),
            1
        );
        assert_eq!(
            deduped
                .iter()
                .filter(|row| fp(row).0 == Some("V9".to_string()))
                .count(),
            0
        );
        // Applying it twice is idempotent.
        assert!(list_rows(VIEWS_XML, &opts).len() == 9);
    }

    #[test]
    fn private_rows_hide_by_default_and_show_flagged_when_opted_in() {
        let opts = run_opts();
        assert_eq!(
            list_rows(VIEWS_XML, &opts)
                .iter()
                .find(|row| fp(row).0 == Some("V7".to_string())),
            None
        );
        let mut include = run_opts();
        include.include_private = true;
        let rows = list_rows(VIEWS_XML, &include);
        let secret = rows
            .iter()
            .find(|row| fp(row).0 == Some("V7".to_string()))
            .unwrap();
        assert!(secret.private);
        assert_eq!(secret.anniversary_day, Some(1));
        assert_eq!(secret.elapsed_years, Some(25));
    }

    #[test]
    fn deterministic_order_never_depends_on_hash_order() {
        let opts = run_opts();
        let db = parse_database(VIEWS_XML).unwrap();
        let events = collect_events(&db, &opts);
        let a = rows(&view(&events, ViewKind::List, &opts));
        let b = rows(&view(&events, ViewKind::List, &opts));
        assert_eq!(a, b);
        // The list sorts equal-date rows by (type, subject, id).
        let ada_v8 = a
            .iter()
            .find(|row| fp(row).0 == Some("V8".to_string()))
            .unwrap();
        let ada_v9 = a
            .iter()
            .find(|row| fp(row).0 == Some("V9".to_string()))
            .unwrap();
        assert_eq!(ada_v8.person_name, "Ada Dates");
        assert_eq!(ada_v9.person_name, "Ada Dates");
        // V8 precedes V9 by id on the same (1970, 5, 5) key.
        let idx_v8 = a.iter().position(|row| row == ada_v8).unwrap();
        let idx_v9 = a.iter().position(|row| row == ada_v9).unwrap();
        assert!(idx_v8 < idx_v9);
    }
}
