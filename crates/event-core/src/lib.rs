//! event-core — the engine between parsing and output.
//!
//! Resolves handles → records, links events ↔ subjects, computes derived
//! values (elapsed years, age at event, anniversary keys), applies the
//! filtering pipeline, builds the four views, and models the `EventRow` /
//! `PdfDocument` output contracts (plan §7.3).
//!
//! Milestone 6 (plan §12 step 6) ships the *resolution* layer:
//!
//! - [`resolve::HandleIndex`] / [`resolve::build_index`] — the
//!   `handle → record` index resolution consults (plan §7.1);
//! - [`model::ResolvedEvent`] / [`model::PersonDisplay`] — the resolved
//!   event model (§7.3);
//! - [`resolve::collect_events`] — event-to-subject resolution with the
//!   plan's precedence order: primary role → any role → family couple →
//!   single reference → orphan `"—"` (§8.6), place paths and `First
//!   Surname` name rendering (§8.8).
//!
//! Derived values (elapsed years, age at event, anniversary keys, leap-day
//! folding) and the `ReportOptions` filtering pipeline land in milestone 7.

pub mod model;
pub mod resolve;

pub use model::{PersonDisplay, ResolvedEvent};
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
