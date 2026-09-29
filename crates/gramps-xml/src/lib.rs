//! gramps-xml — .gramps container detection + XML → typed model.
//!
//! Detects the three container forms (plain XML / gzip / zip), decodes
//! them, and parses the Gramps XML structure into a typed model that
//! mirrors the DTD. Records carry handles, not resolved links —
//! resolution lives in `event-core`. Scaffolded in milestone 1; the
//! parser ships in later milestones.

/// Canonical crate name — smoke tests (and later workspace integration
/// tests) use this to assert crate linkage.
pub const CRATE_NAME: &str = "gramps-xml";

#[cfg(test)]
mod tests {
    use crate::CRATE_NAME;

    #[test]
    fn smoke_crate_builds() {
        assert_eq!(CRATE_NAME, "gramps-xml");
    }
}
