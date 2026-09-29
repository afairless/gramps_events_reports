//! event-core — the engine between parsing and output.
//!
//! Resolves handles → records, links events ↔ subjects, computes derived
//! values (age, elapsed years, anniversary keys), the filtering pipeline,
//! the four view builders, and the `EventRow` / `PdfDocument` output
//! contracts. Scaffolded in milestone 1; the engine ships in later
//! milestones.

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
