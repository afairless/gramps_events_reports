//! cli — the `gramps-events` command-line front-end.
//!
//! clap-based subcommands `inspect`, `list` and `report` drive the
//! event-core pipeline and the writers crate (plan §6.5). The binary
//! (`main.rs`) only parses arguments and streams bytes/text; all the logic
//! lives here in the library so it is unit- and snapshot-testable:
//!
//! - [`args`] — the clap derive surface and the conversions from CLI
//!   values to the shared `ReportOptions` / `Formats` / `ViewKind` types;
//! - [`run`] — the commands (`inspect` / `list` / `report` / `serve`)
//!   over a parsed [`gramps_xml::Database`];
//! - [`render`] — the human-readable text rendering of any of the four
//!   views the `list` subcommand prints.
//!
//! `serve` (the web UI) was wired by milestone 13: it starts the `web`
//! crate's axum server on 127.0.0.1 -- the same binary ships CLI and GUI.

pub mod args;
pub mod render;
pub mod run;

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
