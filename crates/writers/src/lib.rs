//! writers — output backends.
//!
//! The `EventWriter` trait (csv / json / parquet) and the `PdfBackend`
//! trait (render the `PdfDocument` model), behind a `Formats` bitflag
//! factory. All writers write atomically: temp file in the destination
//! directory, renamed over the target only on success. Scaffolded in
//! milestone 1; the backends ship in later milestones.

/// Canonical crate name — smoke tests (and later workspace integration
/// tests) use this to assert crate linkage.
pub const CRATE_NAME: &str = "writers";

#[cfg(test)]
mod tests {
    use crate::CRATE_NAME;

    #[test]
    fn smoke_crate_builds() {
        assert_eq!(CRATE_NAME, "writers");
    }
}
