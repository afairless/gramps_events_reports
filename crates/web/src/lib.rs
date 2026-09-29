//! web — local HTTP server and server-rendered UI.
//!
//! An axum server bound to 127.0.0.1: upload/load/options/events/export
//! routes with a size cap and generated temp file names, plus the
//! Askama + HTMX landing page and views. Scaffolded in milestone 1; the
//! server ships in later milestones.

/// Canonical crate name — smoke tests (and later workspace integration
/// tests) use this to assert crate linkage.
pub const CRATE_NAME: &str = "web";

#[cfg(test)]
mod tests {
    use crate::CRATE_NAME;

    #[test]
    fn smoke_crate_builds() {
        assert_eq!(CRATE_NAME, "web");
    }
}
