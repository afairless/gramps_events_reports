//! cli — the `gramps-events` command-line front-end.
//!
//! clap-based subcommands `report`, `list`, `inspect`, and `serve` share
//! the event-core pipeline and writers. Scaffolded in milestone 1; the
//! commands ship in later milestones.

/// Canonical crate name — smoke tests (and later workspace integration
/// tests) use this to assert crate linkage.
pub const CRATE_NAME: &str = "cli";

#[cfg(test)]
mod tests {
    use crate::CRATE_NAME;

    #[test]
    fn smoke_crate_builds() {
        assert_eq!(CRATE_NAME, "cli");
    }
}
