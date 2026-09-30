//! event-core — the engine between parsing and output.
//!
//! Resolves handles → records, links events ↔ subjects, computes derived
//! values (elapsed years, age at event, anniversary keys), applies the
//! filtering pipeline, builds the four views, and models the `EventRow` /
//! `PdfDocument` output contracts (plan §7.3).
//!
//! Milestone 6 (plan §12 step 6) shipped the *resolution* layer and
//! milestone 7 (plan §12 step 7) the *derived values + filtering* layer:
//!
//! - [`resolve::HandleIndex`] / [`resolve::build_index`] — the
//!   `handle → record` index resolution consults (plan §7.1);
//! - [`model::ResolvedEvent`] / [`model::PersonDisplay`] — the resolved
//!   event model (§7.3);
//! - [`resolve::collect_events`] — event-to-subject resolution with the
//!   plan's precedence order (§8.6), then the derived-value stamping and
//!   `ReportOptions` filtering pipeline (§7.3, §8);
//! - [`options::ReportOptions`] / [`options::LeapDayPolicy`] — the run
//!   configuration: type include/exclude precedence (rule 14 — exclusion
//!   wins), person / date-range / living-only (`probably_alive` port,
//!   §8 rule 15) / private filters, and the Feb 29 fold policy (D6);
//! - [`pipeline::derive`] — `elapsed_years` (§8.4), `age_at_event` (§8.9),
//!   anniversary keys (§8 rules 1/3) and the leap-day fold flag (D6);
//! - [`pipeline::passes`] — the filter pipeline itself.
//!
//! Milestone 8 adds the views and the flat writer contract:
//!
//! - [`row::EventRow`] — the flat serializable output contract shared by
//!   CSV / JSON / Parquet (plan §7.3), incl. `event_date_stop`,
//!   `date_is_range` and `leap_day_folded`;
//! - [`views::build_view`] / [`views::view`] / [`views::rows`] — the
//!   `ListView` and `CalendarView` builders (the milestone-8 views) and the
//!   flatten back to `EventRow`; anchor rules 1/3 (month → day 1, year-only
//!   excluded) and the Feb 29 fold (D6) land here.
//!
//! Milestone 9 adds the year-based views (§8 rules 10–13, D11/D12):
//!
//! - [`views::TimelineView`] — chronological groups under year headers,
//!   range rows keeping their full start → stop extent for the D11 bars,
//!   and the terminal "Undated" group for year-less rows (rule 13);
//! - [`views::CalendarWithYearsView`] — the year-by-year month grid
//!   covering the span of the event data; rows sit on their actual date
//!   cells and year-only rows land in their year's `full_year` list;
//! - the `Timeline` / `CalendarWithYears` variants of [`views::ViewKind`]
//!   and [`views::View`], and deterministic rule-12 output order for both.

pub mod model;
pub mod options;
pub mod pipeline;
pub mod resolve;
pub mod row;
pub mod views;

pub use model::{PersonDisplay, ResolvedEvent};
pub use options::{LeapDayPolicy, ReportOptions};
pub use pipeline::{age_at_event, elapsed_years, probably_alive};
pub use resolve::{HandleIndex, build_index, collect_events};
pub use row::EventRow;
pub use views::{
    CalendarDay, CalendarMonth, CalendarView, CalendarWithYearsDay, CalendarWithYearsMonth,
    CalendarWithYearsView, CalendarWithYearsYear, ListView, TimelineView, TimelineYear, View,
    ViewKind, build_view, rows, view,
};

/// Canonical crate name — smoke tests (and later workspace integration
/// tests) use this to assert crate linkage.
pub const CRATE_NAME: &str = "event-core";

#[cfg(test)]
mod tests {
    use crate::CRATE_NAME;

    #[test]
    fn smoke_crate_builds() {
        assert_eq!(CRATE_NAME, "event-core");
    }
}
