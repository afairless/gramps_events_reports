//! The view builders — plan §7.3, §8.
//!
//! [`build_view`] (and its [`view`] alias) turns a filtered, derived
//! [`ResolvedEvent`] stream into one of the plan's views; [`rows`] flattens
//! any view back to the [`EventRow`] contract the writers serialize.
//!
//! Milestone 8 shipped the **list** and the **anniversary calendar**;
//! milestone 9 adds the two year-based views: the **timeline** (§8 rule 10,
//! with full-extent range rows for the D11 bars) and the
//! **calendar-with-years** (§8 rule 11 / D12).
//!
//! ```text
//!  ResolvedEvent stream ──▶ expand: one EventRow per (event, subject);
//!                                family/couple events collapse to one
//!                                row per event (plan §3.4, D-d);
//!                                dedupe_same collapses identical
//!                                (type, subject, mm-dd) rows
//!                                ┌──────────┬──────────┬───────────────┐
//!                                ▼          ▼          ▼               ▼
//!                          ListView  CalendarView  TimelineView  CalendarWithYearsView
//!                          (rows     (months →    (year headers,   (year-by-year
//!                           sorted     days →       ranges keep      month grid;
//!                           per rule   entries,     their full       rows sit on
//!                           12)        anchored     start→stop       actual date
//!                                      per rules    extent,          cells;
//!                                      1/3, Feb     terminal         year-only rows
//!                                      29 folded    "Undated"        land in the
//!                                      per D6)      group,           year's
//!                                                    rule 13)         full_year)
//! ```
//!
//! Ordering follows the determinism contract (plan §8 rule 12): the list
//! and the timeline sort by (start date, event type, primary subject,
//! event id) with undated rows terminal (rule 13); calendar days follow
//! (month, day) with entries sorted by (specificity, year, event type,
//! subject, event id, person id) — month-only dates (day `00`) before
//! day-specific dates, each specificity class chronological (plan §3.6,
//! requirements 3 & 4); the calendar-with-years flattens by the same
//! (start date, type, subject, id) key. Sorting never depends on set or
//! hash iteration order.

use std::cmp::Ordering;
use std::collections::HashSet;

use crate::model::{PersonDisplay, ResolvedEvent};
use crate::options::ReportOptions;
use crate::row::{EventRow, format_age};
use gramps_dates::Calendar;

/// Which of the plan's views to build. `List` and `Calendar` arrived in
/// milestone 8; the year-based `Timeline` / `CalendarWithYears` variants
/// are added by milestone 9.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    /// Flat, sortable table of events (plan §1.6; §8 rule 12 order).
    List,
    /// Recurring anniversary calendar, no years (plan §1.6, §8 rule 1).
    Calendar,
    /// Chronological view grouped under year headers (plan §8 rule 10):
    /// ranges/spans render as bars spanning start → stop (D11) and
    /// year-less events collect in a terminal "Undated" group (rule 13).
    Timeline,
    /// Year-by-year month grid covering the span of the event data (plan
    /// §8 rule 11 / D12): rows sit on their actual date cells, ranges
    /// carry their full start → stop extent, year-only rows shade whole
    /// years.
    CalendarWithYears,
}

/// A built view — the structure the writers and the web UI render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    /// The flat list: every row in deterministic list order.
    List(ListView),
    /// The anniversary calendar: `EventRow`s grouped by their effective
    /// anchor (month → day → entries).
    Calendar(CalendarView),
    /// The timeline: chronological groups under year headers (rule 10),
    /// range rows keeping their full extent, terminal "Undated" group.
    Timeline(TimelineView),
    /// The calendar-with-years: a year-by-year month grid (rule 11 / D12).
    CalendarWithYears(CalendarWithYearsView),
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

/// The timeline view (plan §8 rule 10) — chronological order grouped
/// under year headers. Single dates render as entries/markers; ranges and
/// spans keep their full start → stop extent (the row's `event_date` /
/// `event_date_stop` / `event_date_text`) so renderers draw them as bars
/// spanning the whole range (D11) — year-only ranges as full-year-width
/// bars. Year-less events — text-only dates and events with no date
/// element — collect in the terminal "Undated" group (rule 13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineView {
    /// The year groups in ascending order, each with its rows in the
    /// plan's deterministic order; the terminal "Undated" group (when any
    /// year-less row exists) is last.
    pub years: Vec<TimelineYear>,
}

/// One year group of the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineYear {
    /// The group's header year; `None` only on the terminal "Undated"
    /// group (rule 13).
    pub year: Option<i32>,
    /// The group's rows in the plan's deterministic order — (start date
    /// ascending, event type, subject, event id), rule 12; the "Undated"
    /// group sorts by (event type, subject, event id), rule 13.
    pub rows: Vec<EventRow>,
}

/// The calendar-with-years view (plan §8 rule 11 / D12): a year-by-year
/// month grid covering the span of the event data.
///
/// Rows sit on their actual date cells — the anchor from rule 1/3 applied
/// to the row's own (year, month, day): a row with a day lands on
/// (month, day), a month-only row lands on (month, 1). Rows with no month
/// — year-only dates and year-only ranges — land in the year's
/// [`CalendarWithYearsYear::full_year`] list and carry the full
/// start → stop extent (`event_date_text`, `date_is_range`): renderers
/// shade the whole month span of every year they cover across the grid
/// (rule 11). Year-less rows never appear here (rule 13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarWithYearsView {
    /// The grid's years in ascending order — every year from the earliest
    /// to the latest row covers, so the grid is a contiguous span (rule
    /// 11). Years without content hold empty months/full_year lists.
    pub years: Vec<CalendarWithYearsYear>,
}

/// One year of the calendar-with-years grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarWithYearsYear {
    /// The year.
    pub year: i32,
    /// The year's month cells that hold rows, ascending; each holds only
    /// the days with entries.
    pub months: Vec<CalendarWithYearsMonth>,
    /// The year's year-only rows — dates and ranges with no month — in
    /// rule-12 order. Each row carries the full range text and extent, so
    /// renderers shade the whole month span of the years it covers.
    pub full_year: Vec<EventRow>,
}

/// One month cell of the calendar-with-years grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarWithYearsMonth {
    /// 1–12.
    pub month: u32,
    /// The days with entries, ascending.
    pub days: Vec<CalendarWithYearsDay>,
}

/// One day cell of the calendar-with-years grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarWithYearsDay {
    /// 1–31.
    pub day: u32,
    /// The rows sitting on this date cell in rule-12 order. Ranges place
    /// here at their start cell and keep their full extent (rule 11).
    pub entries: Vec<EventRow>,
}

/// Build one of the plan's views from a resolved, filtered event stream.
///
/// Every event is expanded into [`EventRow`]s stamped with the run's
/// `opts`: one row per (event, subject), plan §8.6 — except
/// family/couple events, which collapse to one row per event (plan §3.4,
/// D-d). The `List` kind sorts them (rule 12), the `Calendar` kind filters
/// to anchored rows and groups them by their effective (month, day) cell
/// (rules 1/3, D6). The `ReportOptions` filters (privacy, types, people,
/// dates, living-only) keep operating on the full resolved subjects, so
/// `--include-people` matches either spouse before row expansion.
pub fn build_view(events: &[ResolvedEvent], kind: ViewKind, opts: &ReportOptions) -> View {
    match kind {
        ViewKind::List => {
            let rows = expand_rows(events, opts);
            View::List(ListView {
                rows: sort_list_rows(rows),
            })
        }
        ViewKind::Calendar => View::Calendar(build_calendar(expand_rows(events, opts))),
        ViewKind::Timeline => View::Timeline(build_timeline(expand_rows(events, opts))),
        // The calendar-with-years places rows on the Gregorian year grid;
        // the five non-converted calendars have no Gregorian placement
        // (D4) and year-less events are excluded (rule 13), so the stream
        // is filtered before expansion.
        ViewKind::CalendarWithYears => {
            let mut placeable: Vec<ResolvedEvent> = Vec::new();
            for event in events {
                if placeable_on_year_grid(event) {
                    placeable.push(event.clone());
                }
            }
            View::CalendarWithYears(build_calendar_with_years(expand_rows(&placeable, opts)))
        }
    }
}

/// May [`ResolvedEvent::event`]'s rows be placed on the calendar-with-years
/// Gregorian grid (plan §8 rule 11)? Every Gregorian-calendar date — full,
/// month-only or year-only (year-only rows land in the year's `full_year`
/// list) — and every Julian date that normalizes; never the five
/// non-converted calendars (decision D4, no anchor per rule 1) and never
/// year-less events (rule 13).
fn placeable_on_year_grid(event: &ResolvedEvent) -> bool {
    match event.date.as_ref() {
        None => false,
        Some(date) => {
            if event.gregorian.is_some() {
                true
            } else {
                date.calendar == Calendar::Gregorian
            }
        }
    }
}

/// Alias of [`build_view`] — the shorthand name the plan's milestone table
/// uses for constructing a view.
pub fn view(events: &[ResolvedEvent], kind: ViewKind, opts: &ReportOptions) -> View {
    build_view(events, kind, opts)
}

/// Flatten any view back to the flat [`EventRow`] contract (plan §7.3).
///
/// The list and the timeline return their rows in rule-12 order (the
/// timeline's group concatenation reproduces the ordered stream); the
/// calendar returns its entries in calendar order — (month, day,
/// specificity, year, event type, subject, event id, person id); the
/// calendar-with-years returns its rows in
/// the rule-12 (start date, type, subject, id) order — the same relative
/// order the list gives its year-bearing rows (rule 12).
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
        View::Timeline(timeline) => {
            let mut out = Vec::new();
            for year in &timeline.years {
                out.extend(year.rows.iter().cloned());
            }
            out
        }
        View::CalendarWithYears(calendar) => {
            let mut out = Vec::new();
            for year in &calendar.years {
                out.extend(year.full_year.iter().cloned());
                for month in &year.months {
                    for day in &month.days {
                        out.extend(day.entries.iter().cloned());
                    }
                }
            }
            // The grid walks (year, month, day) cell order; rule 12 wants
            // the (start date, type, subject, id) key — the same sort the
            // list applies, so year-bearing list rows keep their order.
            sort_list_rows(out)
        }
    }
}

/// The `dedupe_same` identity — (event type, subject handle, effective
/// anniversary anchor) — the tuple the collapse hash keyed on.
type DedupeKey = (String, String, Option<(u32, u32)>);

/// Expand each event into one [`EventRow`] per subject — or, for a
/// family/couple event, a single collapsed row per event (decision D-d) —
/// applying the `dedupe_same` collapse (plan §8.6): identical (event
/// type, subject, effective anniversary month-day) rows collapse to their
/// first occurrence. Couple rows key on the combined spouse handles, so
/// only identical couple events of the *same* pair collapse; orphan
/// placeholders share the empty handle, so two orphan rows of the same
/// type and month-day also collapse.
fn expand_rows(events: &[ResolvedEvent], opts: &ReportOptions) -> Vec<EventRow> {
    let mut out = Vec::new();
    let mut seen: HashSet<DedupeKey> = HashSet::new();
    for event in events {
        if event.couple {
            // A family/couple event yields exactly one row no matter how
            // many spouses resolve (plan §3.4); an unresolvable spouse set
            // (no father/mother handle in the index) yields none.
            if event.subjects.is_empty() {
                continue;
            }
            let row = couple_row(event, opts);
            if opts.dedupe_same {
                let handles = event
                    .subjects
                    .iter()
                    .map(|subject| subject.handle.clone())
                    .collect::<Vec<_>>()
                    .join("|");
                if !seen.insert((event.event_type.clone(), handles, row_anchor(&row))) {
                    continue;
                }
            }
            out.push(row);
        } else {
            for subject in &event.subjects {
                let row = event_row(event, subject, opts);
                if opts.dedupe_same
                    && !seen.insert((
                        event.event_type.clone(),
                        subject.handle.clone(),
                        row_anchor(&row),
                    ))
                {
                    continue;
                }
                out.push(row);
            }
        }
    }
    out
}

/// The dedupe anchor of an expanded row — `(month, day)` when the row
/// anchors in the anniversary calendar, else `None`.
fn row_anchor(row: &EventRow) -> Option<(u32, u32)> {
    match (row.anniversary_month, row.anniversary_day) {
        (Some(month), Some(day)) => Some((month, day)),
        _ => None,
    }
}

/// The Unicode marriage symbol used to join couple row names (decision
/// D-c): `" ⚭ "`, spaced so `A ⚭ B` reads as a single couple line.
pub(crate) const COUPLE_JOIN: &str = " ⚭ ";

/// Build one [`EventRow`] for `subject`'s view of `event`, stamped with
/// the run's options (reference year, Feb 29 fold policy).
fn event_row(event: &ResolvedEvent, subject: &PersonDisplay, opts: &ReportOptions) -> EventRow {
    row_with_subject_fields(
        event,
        opts,
        subject.gramps_id.clone(),
        None,
        subject.name.clone(),
        subject.role.clone(),
    )
}

/// Build the single [`EventRow`] of a family/couple event (decision D-d):
/// one row per event whose `person_name` joins every subject's name with
/// the marriage symbol (D-c), `person_id` the first spouse's id and
/// `person_id_2` the second's — `None` for a single-spouse family.
/// Additional subjects (an event referenced by several families) reach
/// the contract only through the joined name; the two id columns cover
/// the common two-spouse case.
fn couple_row(event: &ResolvedEvent, opts: &ReportOptions) -> EventRow {
    row_with_subject_fields(
        event,
        opts,
        event
            .subjects
            .first()
            .and_then(|subject| subject.gramps_id.clone()),
        if event.subjects.len() > 1 {
            event.subjects[1].gramps_id.clone()
        } else {
            None
        },
        event
            .subjects
            .iter()
            .map(|subject| subject.name.clone())
            .collect::<Vec<_>>()
            .join(COUPLE_JOIN),
        event.role.clone(),
    )
}

/// Build an [`EventRow`] from the event's shared values plus one subject
/// view (its ids, name and role), stamped with the run's options.
fn row_with_subject_fields(
    event: &ResolvedEvent,
    opts: &ReportOptions,
    person_id: Option<String>,
    person_id_2: Option<String>,
    person_name: String,
    role: String,
) -> EventRow {
    let (year, month, day) = date_components(event);
    let (anniversary_month, anniversary_day) = match event.anniversary {
        None => (None, None),
        Some(anchor) => {
            let (effective, _) = opts.leap_day.fold_anchor(anchor, opts.reference_year);
            (Some(effective.0), Some(effective.1))
        }
    };
    EventRow {
        person_id,
        person_id_2,
        person_name,
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
        role,
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
/// entries sorted by (specificity, year, event type, subject, event id,
/// person id) within a day (plan §3.6, requirements 3 & 4): a month-only
/// date (day `00`, anchored on the 1st) comes before every day-specific
/// date in the shared day-1 cell, and each specificity class sorts
/// chronologically (earliest year first). Rows without an anchor
/// (year-only dates, non-convertible calendars, undated events) are
/// excluded per rule 1.
fn build_calendar(rows: Vec<EventRow>) -> CalendarView {
    let mut anchored: Vec<EventRow> = rows
        .into_iter()
        .filter(|row| row.anniversary_month.is_some())
        .collect();
    anchored.sort_by(compare_calendar_rows);
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

/// The anniversary-calendar order key — (anchor month, anchor day,
/// specificity, year, event type, subject name, event id, person id) —
/// as one comparable tuple (plan §3.6 / decision D-g); the trailing
/// strings borrow from the compared row.
type CalendarSortKey<'a> = (u32, u32, u8, i32, &'a str, &'a str, &'a str, &'a str);

/// The key of one calendar row: the (month, day) anchor cell, then the
/// specificity — `0` for a month-only date (`row.day` unknown) and `1`
/// for a day-specific date — then the year (requirement 3: earliest year
/// first within each class), then the rule-12 tie-breakers (event type,
/// subject name, event id, person id) for equal dates.
fn calendar_sort_key<'a>(row: &'a EventRow) -> CalendarSortKey<'a> {
    (
        row.anniversary_month.unwrap_or(0),
        row.anniversary_day.unwrap_or(0),
        // Month-only dates (day 00, anchored on the 1st) sort before
        // day-specific dates inside the shared (month, day=1) cell
        // (requirement 4).
        if row.day.is_none() { 0 } else { 1 },
        row.year.unwrap_or(0),
        row.event_type.as_str(),
        row.person_name.as_str(),
        row.event_id.as_deref().unwrap_or(""),
        row.person_id.as_deref().unwrap_or(""),
    )
}

/// Compare two rows by the anniversary-calendar order (plan §3.6,
/// requirements 3 & 4): calendar cells ascend by (anchor month, anchor
/// day); within a cell a month-only date (day `00`, anchored on the 1st)
/// sorts before a day-specific date, and each specificity class sorts
/// chronologically — the earliest year first — with the rule-12
/// tie-breakers (event type, subject name, event id, person id) for
/// equal dates. A public total order over `EventRow`s: [`build_calendar`]
/// sorts by it, and the PDF backend consumes the same comparator so the
/// CLI/UI anniversary calendar and the PDF entry order always agree.
pub fn compare_calendar_rows(a: &EventRow, b: &EventRow) -> Ordering {
    calendar_sort_key(a).cmp(&calendar_sort_key(b))
}

/// Build the timeline (plan §8 rules 10 and 13).
///
/// The rows keep the plan's deterministic order — (start date ascending,
/// event type, primary subject, event id) with year-less rows terminal
/// (rule 12) — and are chunked into ascending year groups on that order.
/// The year-less tail becomes the terminal "Undated" group (rule 13);
/// ranges/spans stay whole in their row (extent fields unchanged), so
/// renderers draw them as bars from their start to their stop (D11).
fn build_timeline(rows: Vec<EventRow>) -> TimelineView {
    let sorted = sort_list_rows(rows);
    let mut years: Vec<TimelineYear> = Vec::new();
    for row in sorted {
        let is_new_group = match years.last() {
            None => true,
            Some(last) => last.year != row.year,
        };
        if is_new_group {
            years.push(TimelineYear {
                year: row.year,
                rows: Vec::new(),
            });
        }
        years
            .last_mut()
            .expect("a group was pushed above")
            .rows
            .push(row);
    }
    TimelineView { years }
}

/// Build the calendar-with-years (plan §8 rule 11 / D12).
///
/// Year-bearing rows keep the plan's rule-12 order and are placed on the
/// grid: a row with a month sits on its actual date cell — (month, day),
/// or (month, 1) when the day is unknown (rule 3) — and a row without a
/// month (year-only dates and year-only ranges) lands in its start year's
/// `full_year` list, carrying the full start → stop extent for the grid
/// shading. The grid spans every year from its earliest to its latest row.
/// Year-less rows are never placed (rule 13).
fn build_calendar_with_years(rows: Vec<EventRow>) -> CalendarWithYearsView {
    let mut with_year: Vec<EventRow> = rows.into_iter().filter(|row| row.year.is_some()).collect();
    if with_year.is_empty() {
        return CalendarWithYearsView { years: Vec::new() };
    }
    with_year.sort_by(|a, b| list_sort_key(a).cmp(&list_sort_key(b)));
    let first_year = with_year
        .first()
        .expect("non-empty")
        .year
        .expect("rows have a year");
    let last_year = with_year
        .last()
        .expect("non-empty")
        .year
        .expect("rows have a year");

    let mut years: Vec<CalendarWithYearsYear> = Vec::new();
    let mut cursor = 0;
    for offset in 0..(last_year - first_year + 1) {
        let year = first_year + offset;
        let mut months: Vec<CalendarWithYearsMonth> = Vec::new();
        let mut full_year: Vec<EventRow> = Vec::new();
        while cursor < with_year.len() && with_year[cursor].year == Some(year) {
            let row = with_year[cursor].clone();
            cursor += 1;
            match row.month {
                None => full_year.push(row),
                Some(month) => {
                    // Anchor rule 3: a month without a day lands on day 1.
                    let day = row.day.unwrap_or(1);
                    if months.last().map(|last| last.month) != Some(month) {
                        months.push(CalendarWithYearsMonth {
                            month,
                            days: Vec::new(),
                        });
                    }
                    let month_cell = months.last_mut().expect("pushed above");
                    if month_cell.days.last().map(|last| last.day) != Some(day) {
                        month_cell.days.push(CalendarWithYearsDay {
                            day,
                            entries: Vec::new(),
                        });
                    }
                    month_cell
                        .days
                        .last_mut()
                        .expect("pushed above")
                        .entries
                        .push(row);
                }
            }
        }
        years.push(CalendarWithYearsYear {
            year,
            months,
            full_year,
        });
    }
    CalendarWithYearsView { years }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect_events;
    use crate::options::LeapDayPolicy;
    use gramps_dates::{Calendar, GrampsDate, Modifier, NewYear, Quality};
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

    /// Build the timeline rows for `xml` under `opts` — rule-12 order
    /// (identical to the list's row stream).
    fn timeline_rows<'a>(xml: &'a [u8], opts: &'a ReportOptions) -> Vec<EventRow> {
        let db = parse_database(xml).unwrap();
        let events = collect_events(&db, opts);
        rows(&view(&events, ViewKind::Timeline, opts))
    }

    /// Build the calendar-with-years rows for `xml` under `opts` — rule-12
    /// (start date, type, subject, id) order over the year-bearing rows.
    fn yrcal_rows<'a>(xml: &'a [u8], opts: &'a ReportOptions) -> Vec<EventRow> {
        let db = parse_database(xml).unwrap();
        let events = collect_events(&db, opts);
        rows(&view(&events, ViewKind::CalendarWithYears, opts))
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

        // Abraham's death — the full 21-field contract, all fields set.
        let death = &rows[5];
        assert_eq!(
            death,
            &EventRow {
                person_id: Some("I0004".to_string()),
                person_id_2: None,
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

    /// A database whose February (month, day=1) cell holds both
    /// month-only and day-specific entries across two years, plus a
    /// day-2 cell and an April cell, to lock the requirement-4 order
    /// (specificity first, then year). Each event carries a distinct type
    /// so `dedupe_same` — keyed on (type, subject, effective month-day) —
    /// never collapses a month-only and a day-1 entry of the same cell
    /// into one row.
    const CALENDAR_ORDER_XML: &[u8] = br#"<database>
  <events>
    <event handle="_o0" id="C0" change="1">
      <type>Alpine</type>
      <dateval val="1980-02"/>
    </event>
    <event handle="_o1" id="C1" change="1">
      <type>Atoll</type>
      <dateval val="1980-02-01"/>
    </event>
    <event handle="_o2" id="C2" change="1">
      <type>Birth</type>
      <dateval val="1970-02"/>
    </event>
    <event handle="_o3" id="C3" change="1">
      <type>Boreal</type>
      <dateval val="1970-02-01"/>
    </event>
    <event handle="_o4" id="C4" change="1">
      <type>Coast</type>
      <dateval val="1980-02-02"/>
    </event>
    <event handle="_o5" id="C5" change="1">
      <type>Delta</type>
      <dateval val="1970-04"/>
    </event>
    <event handle="_o6" id="C6" change="1">
      <type>Epoch</type>
      <dateval val="1970-04-01"/>
    </event>
  </events>
  <people>
    <person handle="_op0" id="P0">
      <name type="Birth Name">
        <first>Ada</first>
        <surname>Example</surname>
      </name>
      <eventref hlink="_o0" role="Primary"/>
      <eventref hlink="_o1" role="Primary"/>
      <eventref hlink="_o2" role="Primary"/>
      <eventref hlink="_o3" role="Primary"/>
      <eventref hlink="_o4" role="Primary"/>
      <eventref hlink="_o5" role="Primary"/>
      <eventref hlink="_o6" role="Primary"/>
    </person>
  </people>
</database>"#;

    #[test]
    fn calendar_orders_month_only_before_day_one_then_by_year() {
        let opts = run_opts();
        let db = parse_database(CALENDAR_ORDER_XML).unwrap();
        let events = collect_events(&db, &opts);
        let View::Calendar(calendar) = view(&events, ViewKind::Calendar, &opts) else {
            unreachable!("Calendar kind")
        };
        // Month tree: February has the day-1 and day-2 cells, April the
        // day-1 cell — month-only Feb rows land on day 1, never merged
        // into a different day cell.
        assert_eq!(
            calendar
                .months
                .iter()
                .map(|month| (month.month, month.days.len()))
                .collect::<Vec<_>>(),
            vec![(2, 2), (4, 1)],
        );
        // The (2, 1) cell: month-only entries (day 00) first — 1970-02
        // (C2) before 1980-02 (C0) — then the day-1 entries — 1970-02-01
        // (C3) before 1980-02-01 (C1): specificity, then year.
        let feb = &calendar.months[0];
        assert_eq!(feb.days[0].day, 1);
        assert_eq!(
            feb.days[0]
                .entries
                .iter()
                .map(|row| row.event_id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["C2", "C0", "C3", "C1"],
        );
        assert_eq!(feb.days[1].day, 2);
        assert_eq!(
            feb.days[1]
                .entries
                .iter()
                .map(|row| row.event_id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["C4"],
        );
        // A non-February month shows the same class split.
        assert_eq!(calendar.months[1].days[0].day, 1);
        assert_eq!(
            calendar.months[1].days[0]
                .entries
                .iter()
                .map(|row| row.event_id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["C5", "C6"],
        );
        // rows() flattens the tree back in the same deterministic order.
        let order_rows = calendar_rows(CALENDAR_ORDER_XML, &opts);
        let ids = order_rows
            .iter()
            .map(|row| row.event_id.as_deref().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 7);
        assert_eq!(ids[0], "C2");
        assert_eq!(ids[1], "C0");
        assert_eq!(ids[2], "C3");
        assert_eq!(ids[3], "C1");
        assert_eq!(ids[4], "C4");
        assert_eq!(ids[5], "C5");
        assert_eq!(ids[6], "C6");
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
        // Order: the anchor cells ascend (month, day); entries within a
        // day follow (specificity, year, type, subject, id) — the Jan 1
        // cell opens with the earliest year (the 1801 Marriage D0020),
        // then the 1900 Birth (D0003); the dual-dated Immigration (D0023)
        // anchors on Jan 13 (Julian → Gregorian) right after; on Nov 1
        // the two month-only 1822 entries keep their type tie-break —
        // Birth (D0007) before the Marriage (D0018) — and Nov 11 closes
        // the calendar.
        assert_eq!(ids[0], "D0020"); // Jan 1: earliest year first
        assert_eq!(ids[1], "D0003");
        assert_eq!(ids[2], "D0023"); // Jan 13: the dual-dated span
        assert_eq!(ids[14], "D0007");
        assert_eq!(ids[15], "D0018");
        assert_eq!(ids[16], "D0004");
    }

    // --- golden: families.gramps (couples, roles, orphans)
    // ----------------------------------------------------------------------

    #[test]
    fn couple_event_collapses_to_one_row_with_both_ids() {
        let opts = run_opts();
        let rows = list_rows(FAMILIES_GRAMPS, &opts);
        // The Marriage (E0000), Marriage-Alternative (E0001) and Divorce
        // (E0002) all resolve through family (couple) eventrefs (D-d) and
        // each collapses to exactly one row carrying both spouses (D-c).
        for id in ["E0000", "E0001", "E0002"] {
            assert_eq!(
                rows.iter()
                    .filter(|row| fp(row).0.as_deref() == Some(id))
                    .count(),
                1,
                "{id} yields one collapsed row, not one per spouse"
            );
        }
        let marriage = rows
            .iter()
            .find(|row| fp(row).0.as_deref() == Some("E0000"))
            .unwrap();
        // Father-first subject order: Adam ⚭ Eve, ids in the same order.
        assert_eq!(marriage.person_name, "Adam Uplands ⚭ Eve Uplands");
        assert_eq!(marriage.person_id, Some("I0000".to_string()));
        assert_eq!(marriage.person_id_2, Some("I0001".to_string()));
        assert_eq!(marriage.role, "Family");
        assert_eq!(marriage.elapsed_years, Some(71)); // 2026 − 1955
    }

    #[test]
    fn golden_families_couple_rows_collapse_to_one_row_per_event() {
        let opts = run_opts();
        let rows = list_rows(FAMILIES_GRAMPS, &opts);
        assert_eq!(rows.len(), 9);
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
                "E0005", "E0006", "E0004", "E0000", "E0001", "E0002", "E0003", "E0003", "E0007",
            ]
        );

        // Couples collapse to one row per event with the couple's role
        // (marriages stay visible despite the later divorce — D5); a
        // single-person row carries no second id.
        let marriage = &fp_all[3];
        assert_eq!(marriage.0, Some("E0000".to_string()));
        assert_eq!(marriage.1, "Adam Uplands ⚭ Eve Uplands");
        assert_eq!(marriage.2, "Marriage");
        assert_eq!(fp_all[4].0, Some("E0001".to_string()));
        assert_eq!(fp_all[4].1, "Adam Uplands ⚭ Eve Uplands");
        assert_eq!(fp_all[5].0, Some("E0002".to_string()));
        assert_eq!(fp_all[5].1, "Adam Uplands ⚭ Eve Uplands");

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

    /// A hand-written database exercising the couple-row edge cases the
    /// committed fixtures do not: a single-spouse family (branch (d)) and
    /// two identical couple events for the same pair (dedupe_same).
    const COUPLE_EDGES_XML: &[u8] = br#"<database>
  <events>
    <event handle="_s0" id="S0" change="1">
      <type>Marriage</type>
      <dateval val="1985-06-01"/>
    </event>
    <event handle="_k0" id="K0" change="1">
      <type>Marriage</type>
      <dateval val="1990-05-05"/>
    </event>
    <event handle="_k1" id="K1" change="1">
      <type>Marriage</type>
      <dateval val="1990-05-05"/>
    </event>
  </events>
  <people>
    <person handle="_sp0" id="I0">
      <name type="Birth Name">
        <first>Solo</first>
        <surname>Spouse</surname>
      </name>
    </person>
    <person handle="_kp0" id="J0">
      <name type="Birth Name">
        <first>Jun</first>
        <surname>One</surname>
      </name>
    </person>
    <person handle="_kp1" id="J1">
      <name type="Birth Name">
        <first>No</first>
        <surname>Two</surname>
      </name>
    </person>
  </people>
  <families>
    <family handle="_sf0" id="F0">
      <father hlink="_sp0"/>
      <eventref hlink="_s0" role="Family"/>
    </family>
    <family handle="_kf0" id="F1">
      <father hlink="_kp0"/>
      <mother hlink="_kp1"/>
      <eventref hlink="_k0" role="Family"/>
      <eventref hlink="_k1" role="Family"/>
    </family>
  </families>
</database>"#;

    #[test]
    fn single_spouse_family_keeps_one_row_with_null_second_id() {
        let opts = run_opts();
        let rows = list_rows(COUPLE_EDGES_XML, &opts);
        // S0 is a family event whose mother handle is absent (branch d):
        // still a couple event (one row), the single spouse's name and id,
        // and a null second id.
        assert_eq!(rows.len(), 3);
        let solo = rows
            .iter()
            .find(|row| fp(row).0.as_deref() == Some("S0"))
            .unwrap();
        assert_eq!(solo.person_name, "Solo Spouse");
        assert_eq!(solo.person_id, Some("I0".to_string()));
        assert_eq!(solo.person_id_2, None);
        assert_eq!(solo.role, "Family");
    }

    #[test]
    fn dedupe_same_collapses_identical_couple_rows() {
        let opts = run_opts();
        // Two identical Marriage events of the same couple (same type,
        // subjects, anchor) both remain by default...
        assert_eq!(list_rows(COUPLE_EDGES_XML, &opts).len(), 3);

        // ... and collapse to one under dedupe_same, keyed on the combined
        // spouse handles (the first occurrence K0 survives).
        let mut dedupe = run_opts();
        dedupe.dedupe_same = true;
        let rows = list_rows(COUPLE_EDGES_XML, &dedupe);
        assert_eq!(
            rows.iter()
                .filter(|row| fp(row).0 == Some("K0".to_string()))
                .count(),
            1
        );
        assert_eq!(
            rows.iter()
                .filter(|row| fp(row).0 == Some("K1".to_string()))
                .count(),
            0
        );
        assert!(list_rows(COUPLE_EDGES_XML, &opts).len() == 3);
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
        // 10 events − 1 private = 9 resolved events, expanded to 9 rows
        // (the couple V3 collapses to one row per event).
        assert_eq!(rows.len(), 9);

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
    fn couple_rows_collapse_to_one_row_with_the_joined_names() {
        let opts = run_opts();
        let rows = list_rows(VIEWS_XML, &opts);
        let couple: Vec<&EventRow> = rows
            .iter()
            .filter(|row| fp(row).0 == Some("V3".to_string()))
            .collect();
        assert_eq!(couple.len(), 1);
        assert_eq!(couple[0].person_name, "Zed Fold ⚭ Ada Dates");
        assert_eq!(couple[0].person_id, Some("P0".to_string()));
        assert_eq!(couple[0].person_id_2, Some("P1".to_string()));
        assert_eq!(couple[0].role, "Family");
        assert!(couple[0].date_is_range);
        assert_eq!(couple[0].elapsed_years, Some(38)); // 2026 − 1988
    }

    #[test]
    fn dedupe_same_collapses_identical_type_subject_anchor_rows() {
        let mut opts = run_opts();
        let plain = list_rows(VIEWS_XML, &opts);
        assert_eq!(plain.len(), 9);
        // Ada's two identical Birth rows (V8, V9 — same type, subject,
        // month-day) both remain by default...
        assert_eq!(fp(&plain[0]).0, Some("V8".to_string()));
        assert_eq!(fp(&plain[1]).0, Some("V9".to_string()));

        // ... and collapse to one under dedupe_same.
        opts.dedupe_same = true;
        let deduped = list_rows(VIEWS_XML, &opts);
        assert_eq!(deduped.len(), 8);
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
        assert!(list_rows(VIEWS_XML, &opts).len() == 8);
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

    // --- milestone 9: the year-based views (timeline, calendar-with-years)
    // ----------------------------------------------------------------------

    #[test]
    fn timeline_groups_under_year_headers_with_a_terminal_undated_group() {
        let opts = run_opts();
        let db = parse_database(VIEWS_XML).unwrap();
        let events = collect_events(&db, &opts);
        let View::Timeline(timeline) = view(&events, ViewKind::Timeline, &opts) else {
            unreachable!("Timeline kind");
        };
        // Ascending year headers, the year-less events (V5 text, V6 undated)
        // closing the timeline in the terminal "Undated" group (rule 13).
        assert_eq!(
            timeline
                .years
                .iter()
                .map(|year| year.year)
                .collect::<Vec<_>>(),
            vec![
                Some(1970),
                Some(1988),
                Some(1990),
                Some(1995),
                Some(1998),
                Some(2000),
                None,
            ],
        );
        let ids = |year: &TimelineYear| {
            year.rows
                .iter()
                .map(|row| fp(row).0.expect("fixture rows carry event ids"))
                .collect::<Vec<_>>()
        };
        // Equal start dates tie-break by (type, subject, id): V8 before V9.
        assert_eq!(ids(&timeline.years[0]), vec!["V8", "V9"]);
        // The 1988–1989 couple range collapses to one row with its full
        // extent intact.
        assert_eq!(ids(&timeline.years[1]), vec!["V3"]);
        assert!(timeline.years[1].rows.iter().all(|row| row.date_is_range));
        assert!(timeline.years[1].rows.iter().all(|row| row.month.is_none()));
        // The orphan text and undated events close the timeline, sorted by
        // (type, subject, id).
        assert_eq!(ids(&timeline.years[6]), vec!["V5", "V6"]);
        assert!(timeline.years[6].rows.iter().all(|row| row.year.is_none()));
        assert!(
            timeline.years[6]
                .rows
                .iter()
                .all(|row| row.person_name == "—")
        );
    }

    #[test]
    fn timeline_range_rows_keep_their_full_extent_for_bars() {
        let opts = run_opts();
        let db = parse_database(DATES_GRAMPS).unwrap();
        let events = collect_events(&db, &opts);
        let View::Timeline(timeline) = view(&events, ViewKind::Timeline, &opts) else {
            unreachable!("Timeline kind");
        };
        let in_group = |row: &EventRow, id: &str| fp(row).0.as_deref() == Some(id);
        // D0017 (1914-07-28 → 1918-11-11) sits in the 1914 group carrying
        // both ISO endpoints — the bar geometry (rule 10, D11).
        let group_1914 = timeline
            .years
            .iter()
            .find(|year| year.year == Some(1914))
            .unwrap();
        let ww1 = group_1914
            .rows
            .iter()
            .find(|row| in_group(row, "D0017"))
            .unwrap();
        assert_eq!(ww1.year, Some(1914));
        assert_eq!(ww1.event_date, Some("1914-07-28".to_string()));
        assert_eq!(ww1.event_date_stop, Some("1918-11-11".to_string()));
        assert!(ww1.date_is_range);
        // D0019 (1822–1824, year-only) is a full-year-width bar: the start
        // year only, no month, and the full range text carries the stop.
        let group_1822 = timeline
            .years
            .iter()
            .find(|year| year.year == Some(1822))
            .unwrap();
        let year_range = group_1822
            .rows
            .iter()
            .find(|row| in_group(row, "D0019"))
            .unwrap();
        assert_eq!(year_range.year, Some(1822));
        assert_eq!(year_range.month, None);
        assert!(year_range.date_is_range);
        assert!(year_range.event_date_text.contains("1824"));
        // The text-only date closes the timeline in the Undated group.
        let undated = timeline.years.last().expect("terminal undated group");
        assert_eq!(undated.year, None);
        assert_eq!(undated.rows.len(), 1);
        assert_eq!(fp(&undated.rows[0]).0, Some("D0024".to_string()));
    }

    #[test]
    fn calendar_with_years_is_a_contiguous_year_grid_of_actual_date_cells() {
        let opts = run_opts();
        let db = parse_database(VIEWS_XML).unwrap();
        let events = collect_events(&db, &opts);
        let View::CalendarWithYears(calendar) = view(&events, ViewKind::CalendarWithYears, &opts)
        else {
            unreachable!("CalendarWithYears kind");
        };
        // The grid covers every year from the earliest to the latest row
        // (rule 11) — 1970 through 2000 is 31 years (the private 2001
        // event is filtered before the view is built).
        assert_eq!(calendar.years.len(), 31);
        assert_eq!(calendar.years.first().expect("grid").year, 1970);
        assert_eq!(calendar.years.last().expect("grid").year, 2000);
        // Feb 29 sits on its actual date cell — the D6 fold only shifts the
        // anniversary anchor, never the grid cell — and stays labeled.
        let year_2000 = calendar
            .years
            .iter()
            .find(|year| year.year == 2000)
            .unwrap();
        assert_eq!(
            year_2000
                .months
                .iter()
                .map(|month| (month.month, month.days[0].day, month.days[0].entries.len()))
                .collect::<Vec<_>>(),
            vec![(2, 29, 1)],
        );
        assert!(year_2000.months[0].days[0].entries[0].leap_day_folded);
        // Month-only V1 lands on (4, 1) per anchor rule 3.
        let year_1995 = calendar
            .years
            .iter()
            .find(|year| year.year == 1995)
            .unwrap();
        assert_eq!(year_1995.months[0].month, 4);
        assert_eq!(year_1995.months[0].days[0].day, 1);
        assert_eq!(
            fp(&year_1995.months[0].days[0].entries[0]).0,
            Some("V1".to_string())
        );
        // The Immigration range V4 (1998-03 → 1998-05) lands at its start
        // cell and keeps the full extent text (rule 11).
        let year_1998 = calendar
            .years
            .iter()
            .find(|year| year.year == 1998)
            .unwrap();
        assert_eq!(year_1998.months[0].month, 3);
        assert_eq!(year_1998.months[0].days[0].day, 1);
        let v4 = &year_1998.months[0].days[0].entries[0];
        assert_eq!(fp(v4).0, Some("V4".to_string()));
        assert!(v4.date_is_range);
        assert!(v4.event_date_text.contains("1998-05"));
        // Year-only rows land in full_year: the 1988–1989 couple range
        // (one collapsed row) and the 1990 birth.
        let year_1988 = calendar
            .years
            .iter()
            .find(|year| year.year == 1988)
            .unwrap();
        assert_eq!(year_1988.full_year.len(), 1);
        assert!(year_1988.full_year.iter().all(|row| row.month.is_none()));
        assert!(year_1988.full_year.iter().all(|row| row.date_is_range));
        let year_1990 = calendar
            .years
            .iter()
            .find(|year| year.year == 1990)
            .unwrap();
        assert_eq!(year_1990.full_year.len(), 1);
        assert!(year_1990.full_year[0].month.is_none());
        // An empty middle year is still a grid year.
        let year_1971 = calendar
            .years
            .iter()
            .find(|year| year.year == 1971)
            .unwrap();
        assert_eq!(year_1971.months.len(), 0);
        assert_eq!(year_1971.full_year.len(), 0);
    }

    #[test]
    fn calendar_with_years_places_full_extent_ranges_and_excludes_the_unplaceable() {
        let opts = run_opts();
        let rows = yrcal_rows(DATES_GRAMPS, &opts);
        let ids = rows
            .iter()
            .map(|row| fp(row).0.expect("fixture rows carry event ids"))
            .collect::<Vec<_>>();
        // Only Gregorian-placeable rows appear: the five non-converted
        // calendars (D4), the text-only date and any year-less row (rule
        // 13) are never placed.
        for id in [
            "D0000", "D0001", "D0002", "D0003", "D0004", "D0005", "D0006", "D0007", "D0008",
            "D0009", "D0010", "D0016", "D0017", "D0018", "D0019", "D0020", "D0021", "D0022",
            "D0023",
        ] {
            assert!(ids.iter().any(|x| x.as_str() == id), "{id} must be placed");
        }
        for id in ["D0011", "D0012", "D0013", "D0014", "D0015", "D0024"] {
            assert!(!ids.iter().any(|x| x.as_str() == id), "{id} must be absent");
        }

        let db = parse_database(DATES_GRAMPS).unwrap();
        let events = collect_events(&db, &opts);
        let View::CalendarWithYears(calendar) = view(&events, ViewKind::CalendarWithYears, &opts)
        else {
            unreachable!("CalendarWithYears kind");
        };
        // Grid endpoints: -550 (D0008) through 2000 (D0000).
        assert_eq!(calendar.years.len(), 2551);
        assert_eq!(calendar.years.first().expect("grid").year, -550);
        assert_eq!(calendar.years.last().expect("grid").year, 2000);
        // The full-extent range D0017 sits at its start cell (1914, 7, 28).
        let year_1914 = calendar
            .years
            .iter()
            .find(|year| year.year == 1914)
            .unwrap();
        let july = year_1914
            .months
            .iter()
            .find(|month| month.month == 7)
            .unwrap();
        assert_eq!(july.days[0].day, 28);
        assert_eq!(fp(&july.days[0].entries[0]).0, Some("D0017".to_string()));
        assert_eq!(
            july.days[0].entries[0].event_date_stop,
            Some("1918-11-11".to_string()),
        );
        // The month-only Birth and month-only range share cell
        // (1822, 11, 1), the Birth first by type (rule 12).
        let year_1822 = calendar
            .years
            .iter()
            .find(|year| year.year == 1822)
            .unwrap();
        let november = year_1822
            .months
            .iter()
            .find(|month| month.month == 11)
            .unwrap();
        assert_eq!(november.days[0].day, 1);
        assert_eq!(
            fp(&november.days[0].entries[0]).0,
            Some("D0007".to_string())
        );
        assert_eq!(
            fp(&november.days[0].entries[1]).0,
            Some("D0018".to_string())
        );
        assert!(november.days[0].entries[1].date_is_range);
        // The year-only date and the year-only range land in full_year,
        // Birth before Marriage (rule 12).
        assert_eq!(year_1822.full_year.len(), 2);
        assert_eq!(fp(&year_1822.full_year[0]).0, Some("D0006".to_string()));
        let d0019 = &year_1822.full_year[1];
        assert_eq!(fp(d0019).0, Some("D0019".to_string()));
        assert_eq!(d0019.month, None);
        assert!(d0019.date_is_range);
        assert!(d0019.event_date_text.contains("1824"));
    }

    /// Locks decision D-f for the calendar-with-years grid: a month-only
    /// date (day `00`) sorts before a day-specific date inside the same
    /// (year, month, day=1) cell. The grid sorts its year-bearing rows by
    /// the list key, whose date tuple reads the partial day as
    /// `day.unwrap_or(0)`, so a month-only row lands on day 0 and thus
    /// precedes the day-1 row in the shared cell, and years ascend across
    /// the grid. Regression only — no production change, the assertions
    /// pin the explicit cell order so a future sort change cannot
    /// silently reorder the grid.
    #[test]
    fn calendar_with_years_orders_month_only_before_day_one() {
        let opts = run_opts();
        let db = parse_database(CALENDAR_ORDER_XML).unwrap();
        let events = collect_events(&db, &opts);
        let View::CalendarWithYears(calendar) = view(&events, ViewKind::CalendarWithYears, &opts)
        else {
            unreachable!("CalendarWithYears kind");
        };
        // The grid spans every year the fixture covers (rule 11) — 1970
        // through 1980 inclusive.
        assert_eq!(calendar.years.len(), 11);
        assert_eq!(calendar.years.first().expect("grid").year, 1970);
        assert_eq!(calendar.years.last().expect("grid").year, 1980);

        // The (1970, 4, 1) cell: the month-only 1970-04 (C5) comes before
        // the day-specific 1970-04-01 (C6) — and only those two rows.
        let year_1970 = calendar.years.iter().find(|y| y.year == 1970).unwrap();
        let april = year_1970.months.iter().find(|m| m.month == 4).unwrap();
        assert_eq!(april.days.len(), 1);
        assert_eq!(april.days[0].day, 1);
        assert_eq!(row_ids(&april.days[0].entries), vec!["C5", "C6"]);
        assert_eq!(april.days[0].entries[0].day, None);
        assert_eq!(april.days[0].entries[1].day, Some(1));
        // The (1970, 2, 1) cell: month-only 1970-02 (C2) before day-1
        // 1970-02-01 (C3).
        let feb_1970 = year_1970.months.iter().find(|m| m.month == 2).unwrap();
        assert_eq!(feb_1970.days.len(), 1);
        assert_eq!(feb_1970.days[0].day, 1);
        assert_eq!(row_ids(&feb_1970.days[0].entries), vec!["C2", "C3"]);
        assert_eq!(feb_1970.days[0].entries[0].day, None);
        assert_eq!(feb_1970.days[0].entries[1].day, Some(1));
        // The (1980, 2, 1) cell: month-only 1980-02 (C0) before day-1
        // 1980-02-01 (C1), and the same day-0/day-1 split holds in a later
        // grid year.
        let year_1980 = calendar.years.iter().find(|y| y.year == 1980).unwrap();
        let feb_1980 = year_1980.months.iter().find(|m| m.month == 2).unwrap();
        assert_eq!(row_ids(&feb_1980.days[0].entries), vec!["C0", "C1"]);
        assert_eq!(feb_1980.days[0].entries[0].day, None);
        assert_eq!(feb_1980.days[0].entries[1].day, Some(1));
        // Month-only never merges into a different day cell: the day-2
        // cell holds only the 1980-02-02 row (C4).
        assert_eq!(feb_1980.days.len(), 2);
        assert_eq!(feb_1980.days[1].day, 2);
        assert_eq!(row_ids(&feb_1980.days[1].entries), vec!["C4"]);
        assert_eq!(feb_1980.days[1].entries[0].day, Some(2));
    }

    /// The event ids of a row slice, in order — test helper for
    /// cell-order assertions.
    fn row_ids(entries: &[EventRow]) -> Vec<&str> {
        entries
            .iter()
            .map(|row| {
                row.event_id
                    .as_deref()
                    .expect("fixture rows carry event ids")
            })
            .collect()
    }

    #[test]
    fn view_kind_and_view_cover_all_four_views() {
        let opts = run_opts();
        let db = parse_database(VIEWS_XML).unwrap();
        let events = collect_events(&db, &opts);
        let v_list = view(&events, ViewKind::List, &opts);
        let v_cal = view(&events, ViewKind::Calendar, &opts);
        let v_tl = view(&events, ViewKind::Timeline, &opts);
        let v_yrcal = view(&events, ViewKind::CalendarWithYears, &opts);
        match v_list {
            View::List(_) => (),
            _ => unreachable!("List kind"),
        }
        match v_cal {
            View::Calendar(_) => (),
            _ => unreachable!("Calendar kind"),
        }
        match v_tl {
            View::Timeline(_) => (),
            _ => unreachable!("Timeline kind"),
        }
        match v_yrcal {
            View::CalendarWithYears(_) => (),
            _ => unreachable!("CalendarWithYears kind"),
        }
        // Timeline rows are the list rows; yrcal keeps only the
        // year-bearing ones; the anniversary calendar its anchored ones.
        let list = rows(&v_list);
        assert_eq!(list.len(), 9);
        assert_eq!(rows(&v_tl), list);
        assert_eq!(rows(&v_yrcal).len(), 7);
        assert_eq!(rows(&v_cal).len(), 5);
    }

    #[test]
    fn timeline_and_yrcal_flatten_in_rule_12_order() {
        let opts = run_opts();
        // The timeline row stream is exactly the list's (rules 10 + 12).
        assert_eq!(timeline_rows(VIEWS_XML, &opts), list_rows(VIEWS_XML, &opts));
        assert_eq!(
            timeline_rows(DATES_GRAMPS, &opts),
            list_rows(DATES_GRAMPS, &opts)
        );
        // The yrcal keeps the list's year-bearing rows in the same order.
        let list = list_rows(VIEWS_XML, &opts);
        let placeable = list
            .iter()
            .filter(|row| row.year.is_some())
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(yrcal_rows(VIEWS_XML, &opts), placeable);
    }

    #[test]
    fn year_based_views_are_deterministic() {
        let opts = run_opts();
        let db = parse_database(DATES_GRAMPS).unwrap();
        let events = collect_events(&db, &opts);
        assert_eq!(
            rows(&view(&events, ViewKind::Timeline, &opts)),
            rows(&view(&events, ViewKind::Timeline, &opts)),
        );
        assert_eq!(
            rows(&view(&events, ViewKind::CalendarWithYears, &opts)),
            rows(&view(&events, ViewKind::CalendarWithYears, &opts)),
        );
    }

    // --- property tests: ordering and grid invariants under random input
    // -----------------------------------------------------------------------

    // The `proptest::prelude` root shadows nothing of the crate CLI; it
    // provides the `prop` module and the `proptest!` macro.
    use proptest::prelude::*;

    proptest! {
        /// The timeline flattens back to exactly the list's row stream:
        /// grouping under year headers and the terminal "Undated" group
        /// reorder nothing (plan §8 rules 10, 12, 13).
        #[test]
        fn timeline_rows_equal_list_rows(
            events in prop::collection::vec(gen_event(), 0..20)
        ) {
            let opts = run_opts();
            prop_assert_eq!(
                rows(&build_view(&events, ViewKind::Timeline, &opts)),
                rows(&build_view(&events, ViewKind::List, &opts)),
            );
        }

        /// The calendar-with-years keeps exactly the year-bearing rows (all
        /// generated events are Gregorian, so every year-bearing row is
        /// placeable), in the same rule-12 order as the list (plan §8 rules
        /// 11–12).
        #[test]
        fn calendar_with_years_rows_are_year_bearing_list_rows(
            events in prop::collection::vec(gen_event(), 0..20)
        ) {
            let opts = run_opts();
            let list = rows(&build_view(&events, ViewKind::List, &opts));
            let with_years = list
                .iter()
                .filter(|row| row.year.is_some())
                .cloned()
                .collect::<Vec<_>>();
            prop_assert_eq!(
                rows(&build_view(&events, ViewKind::CalendarWithYears, &opts)),
                with_years,
            );
        }

        /// Timeline year groups are sound: every row's year matches its
        /// group header, dated groups ascend, and the year-less "Undated"
        /// group is last.
        #[test]
        fn timeline_year_groups_match_their_rows(
            events in prop::collection::vec(gen_event(), 0..20)
        ) {
            let opts = run_opts();
            let View::Timeline(timeline) = build_view(&events, ViewKind::Timeline, &opts) else {
                unreachable!("Timeline kind");
            };
            for (index, group) in timeline.years.iter().enumerate() {
                for row in &group.rows {
                    prop_assert_eq!(row.year, group.year);
                }
                if index > 0 {
                    let prev = &timeline.years[index - 1];
                    prop_assert!(year_rank(prev.year) <= year_rank(group.year));
                }
            }
        }

        /// The year grid is contiguous — every year between its first and
        /// its last appears exactly once (rule 11) — and every row lands in
        /// a cell or `full_year` list of its own year: year-only rows in
        /// `full_year`, month-bearing rows on cells that ascend by
        /// (month, day).
        #[test]
        fn calendar_with_years_grid_is_contiguous_and_sorted(
            events in prop::collection::vec(gen_event(), 0..20)
        ) {
            let opts = run_opts();
            let View::CalendarWithYears(calendar) =
                build_view(&events, ViewKind::CalendarWithYears, &opts) else {
                unreachable!("CalendarWithYears kind");
            };
            let first = calendar.years.first().map(|year| year.year);
            for (index, year) in calendar.years.iter().enumerate() {
                prop_assert_eq!(
                    year.year,
                    first.expect("non-empty grid") + (index as i32),
                );
                for row in &year.full_year {
                    prop_assert_eq!(row.year, Some(year.year));
                    prop_assert!(row.month.is_none());
                }
                let mut last_month = 0;
                for month in &year.months {
                    prop_assert!(month.month > last_month);
                    last_month = month.month;
                    let mut last_day = 0;
                    for day in &month.days {
                        prop_assert!(day.day > last_day);
                        last_day = day.day;
                        for row in &day.entries {
                            prop_assert_eq!(row.year, Some(year.year));
                        }
                    }
                }
            }
        }

        /// The calendar comparator is a total order over the whole row
        /// domain (anchored or not): reflexive (`a` ≤ `a`), antisymmetric
        /// (equal exactly when the reversed comparison is equal; less-than
        /// exactly the opposite of greater-than) and transitive (`a ≤ b ≤ c`
        /// implies `a ≤ c`). A comparator with any contradiction or gap
        /// would make a sort non-deterministic (plan §5 calendar ordering).
        #[test]
        fn calendar_comparator_is_a_total_order(
            events in prop::collection::vec(gen_event(), 0..20)
        ) {
            let opts = run_opts();
            let rows = rows(&build_view(&events, ViewKind::List, &opts));
            for a in &rows {
                prop_assert!(compare_calendar_rows(a, a).is_eq());
                for b in &rows {
                    let ab = compare_calendar_rows(a, b);
                    let ba = compare_calendar_rows(b, a);
                    prop_assert_eq!(ab.is_eq(), ba.is_eq());
                    prop_assert_eq!(ab.is_lt(), ba.is_gt());
                    prop_assert_eq!(ab.is_gt(), ba.is_lt());
                }
            }
            for a in &rows {
                for b in &rows {
                    for c in &rows {
                        if !compare_calendar_rows(a, b).is_gt()
                            && !compare_calendar_rows(b, c).is_gt()
                        {
                            prop_assert!(!compare_calendar_rows(a, c).is_gt());
                        }
                    }
                }
            }
        }

        /// `build_calendar` agrees with sorting its rows by
        /// [`compare_calendar_rows`]: the flattened calendar equals the
        /// anchored rows in comparator order, and rebuilding it from a
        /// reversed (shuffled) row stream yields the identical tree — the
        /// view is deterministic under any arrival order (plan §5).
        #[test]
        fn calendar_matches_comparator_sort_under_shuffled_input(
            events in prop::collection::vec(gen_event(), 0..20)
        ) {
            let opts = run_opts();
            let expanded = expand_rows(&events, &opts);
            let mut anchored = expanded
                .iter()
                .filter(|row| row.anniversary_month.is_some())
                .cloned()
                .collect::<Vec<_>>();
            anchored.sort_by(compare_calendar_rows);

            let forward = build_calendar(expanded.clone());
            prop_assert_eq!(rows(&View::Calendar(forward)), anchored.clone());

            let mut shuffled = expanded;
            shuffled.reverse();
            let backward = build_calendar(shuffled);
            prop_assert_eq!(rows(&View::Calendar(backward)), anchored);
        }
    }

    /// Strategy args for a generated event: (kind, year, month, day,
    /// offset) — `kind` picks the date shape, `offset` the range length.
    /// Years stay in the 20th/21st century so the calendar-with-years grid
    /// span stays small; months/days use the partial-date convention
    /// (0 = unknown) via the stored components only.
    fn gen_event_args() -> impl Strategy<Value = (u32, i32, u32, u32, i32)> {
        (0u32..=5, 1900i32..=2025, 1u32..=12, 1u32..=28, 0i32..=5)
    }

    /// A strategy of random resolved events for the year-based view
    /// properties.
    fn gen_event() -> impl Strategy<Value = ResolvedEvent> {
        gen_event_args().prop_map(build_event)
    }

    /// Build one generated resolved event — Gregorian only, so every
    /// event is placeable on the year grid — with the date kind varying
    /// over undated / year-only / month-only / full / range(full) /
    /// range(year-only). `gregorian` stays `None` (the view derives the
    /// year/month/day from the stored components, exactly the partial-date
    /// fallback real parsing produces for non-full dates).
    fn build_event(args: (u32, i32, u32, u32, i32)) -> ResolvedEvent {
        let (kind, year, month, day, offset) = args;
        let stop_year = year + offset;
        let (date, anniversary) = match kind {
            0u32 => (None, None),
            1 => (Some(synthetic_date((year, 0, 0), None)), None),
            2 => (
                Some(synthetic_date((year, month, 0), None)),
                Some((month, 1)),
            ),
            3 => (
                Some(synthetic_date((year, month, day), None)),
                Some((month, day)),
            ),
            4 => {
                let stop = (stop_year, month, day);
                (
                    Some(synthetic_date((year, month, day), Some(stop))),
                    Some((month, day)),
                )
            }
            _ => {
                let stop = (stop_year, 0, 0);
                (Some(synthetic_date((year, 0, 0), Some(stop))), None)
            }
        };
        ResolvedEvent {
            event_type: match kind % 3 {
                0 => "Birth".to_string(),
                1 => "Marriage".to_string(),
                _ => "Immigration".to_string(),
            },
            event_id: Some(format!("E{}-{}-{}", kind, year, month)),
            date,
            gregorian: None,
            subjects: vec![PersonDisplay {
                gramps_id: None,
                handle: "prop-1".to_string(),
                name: "P".to_string(),
                role: "Primary".to_string(),
            }],
            role: "Primary".to_string(),
            place_path: None,
            private: false,
            orphan: false,
            couple: false,
            elapsed_years: None,
            anniversary,
            leap_day_folded: false,
            age_at_event: None,
        }
    }

    /// A synthetic Gregorian date for the property generator — plain
    /// fields, no parsing involved. Ranges get `Modifier::Range` so
    /// `is_range()` reports them as such (rule 2).
    fn synthetic_date(ymd: (i32, u32, u32), stop: Option<(i32, u32, u32)>) -> GrampsDate {
        GrampsDate {
            calendar: Calendar::Gregorian,
            modifier: if stop.is_some() {
                Modifier::Range
            } else {
                Modifier::None
            },
            quality: Quality::None,
            ymd,
            stop,
            dual_dated: false,
            new_year: NewYear::Jan1,
            display: "date".to_string(),
        }
    }

    /// Rank an optional year for ordering checks — year-less (the
    /// "Undated" group) sorts after every real year.
    fn year_rank(year: Option<i32>) -> i64 {
        match year {
            None => 200000,
            Some(y) => y as i64,
        }
    }
}
