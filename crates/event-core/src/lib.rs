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
//! Milestone 8 adds the views (`ListView`, `CalendarView`,
//! `CalendarWithYearsView`, `TimelineView`) and the `EventRow` contract.

pub mod model;
pub mod options;
pub mod pipeline;
pub mod resolve;

pub use model::{PersonDisplay, ResolvedEvent};
pub use options::{LeapDayPolicy, ReportOptions};
pub use pipeline::{age_at_event, elapsed_years, probably_alive};
pub use resolve::{HandleIndex, build_index, collect_events};

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
