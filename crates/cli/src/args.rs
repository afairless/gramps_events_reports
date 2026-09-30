//! The clap derive surface for the `gramps-events` binary (plan §6.5) and
//! the conversions from raw CLI values to the shared core types.
//!
//! The subcommands mirror the plan's CLI sketch:
//!
//! - `inspect <file>` — enumerate every event type with its count (what the
//!   GUI's checkbox list is built from, plan §9);
//! - `list <file> [filters] [--view list|calendar|timeline|yrcal]` — render
//!   any of the four views as text (list is the default);
//! - `report <file> [filters] [--format …] [--out-dir …] [--out-prefix …]` —
//!   write the who-view as csv / json / parquet / pdf files;
//! - `serve [--port 8380]` — start the local web UI on 127.0.0.1 (the
//!   `web` crate; plan §6.5).
//!
//! `--format` is required on `report` (no formats is a clap error) and
//! `--format all` expands to exactly csv, json, parquet and pdf (plan §11).

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, anyhow, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use event_core::{ReportOptions, ViewKind};
use jiff::civil::Date;
use writers::Formats;

/// The `gramps-events` command line (plan §6.5).
#[derive(Debug, Parser)]
#[command(
    name = "gramps-events",
    version,
    about = "Export and view the events of a Gramps database",
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands of `gramps-events`.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print every event type in the database with its event count.
    Inspect {
        /// The `.gramps` file: plain XML, or a gzip/zip container.
        file: PathBuf,
    },
    /// Render one of the four views of the database as text.
    List(ListArgs),
    /// Write the database events to csv / json / parquet / pdf files.
    Report(ReportArgs),
    /// Start the local web UI on 127.0.0.1:PORT (the `web` crate).
    Serve(ServeArgs),
}

/// Filters shared by `list` and `report` — the plan's "same filters"
/// (plan §6.5): event-type selection, living-only, privacy, date range and
/// the report reference year.
#[derive(Debug, Default, Args)]
pub struct CommonFilters {
    /// Only these event types, comma-separated (whitelist).
    #[arg(long, value_delimiter = ',')]
    pub include_types: Vec<String>,
    /// Exclude these event types, comma-separated; exclusion wins over the
    /// whitelist (plan §8 rule 14).
    #[arg(long, value_delimiter = ',')]
    pub exclude_types: Vec<String>,
    /// The report year every elapsed value is measured against (plan §8.4).
    #[arg(long)]
    pub reference_year: Option<i32>,
    /// Keep only events whose subjects are all presumably alive (rule 15).
    #[arg(long)]
    pub living_only: bool,
    /// Include records marked private (plan §8.7).
    #[arg(long)]
    pub include_private: bool,
    /// Inclusive date window on the normalized start date, `START..END`
    /// with ISO dates, e.g. `1900-01-01..2026-12-31`.
    #[arg(long)]
    pub date_range: Option<String>,
}

/// The `list` subcommand arguments.
#[derive(Debug, Args)]
pub struct ListArgs {
    /// The `.gramps` file: plain XML, or a gzip/zip container.
    pub file: PathBuf,
    /// The shared filters.
    #[command(flatten)]
    pub filters: CommonFilters,
    /// Which view to render: `list`, `calendar`, `timeline`, or `yrcal`.
    #[arg(long, default_value = "list")]
    pub view: ViewArg,
}

/// The `report` subcommand arguments.
#[derive(Debug, Args)]
pub struct ReportArgs {
    /// The `.gramps` file: plain XML, or a gzip/zip container.
    pub file: PathBuf,
    /// The shared filters.
    #[command(flatten)]
    pub filters: CommonFilters,
    /// Output formats, comma-separated: `csv`, `json`, `parquet`, `pdf`,
    /// or `all` (required — no formats is an error).
    #[arg(long, value_delimiter = ',', required = true)]
    pub format: Vec<FormatArg>,
    /// Directory to write the output files into (created if absent).
    #[arg(long)]
    pub out_dir: Option<PathBuf>,
    /// Filename prefix for the output files (default: the input file stem).
    #[arg(long)]
    pub out_prefix: Option<String>,
}

/// The `serve` subcommand arguments (plan §6.5: `serve [--port 8380]`).
///
/// The server binds **127.0.0.1** only (plan §9) — a local, single-user
/// tool handling family PII; nothing listens on an external interface.
#[derive(Debug, Args)]
pub struct ServeArgs {
    /// The TCP port to listen on.
    #[arg(long, default_value_t = web::DEFAULT_PORT)]
    pub port: u16,
}

/// The four view kinds accepted by `--view`, with `yrcal` for the
/// calendar-with-years view (plan §6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ViewArg {
    /// The flat list (default).
    List,
    /// The anniversary calendar (no years).
    Calendar,
    /// The chronological timeline with range bars.
    Timeline,
    /// The calendar-with-years grid.
    #[value(name = "yrcal")]
    Yrcal,
}

impl From<ViewArg> for ViewKind {
    fn from(v: ViewArg) -> ViewKind {
        match v {
            ViewArg::List => ViewKind::List,
            ViewArg::Calendar => ViewKind::Calendar,
            ViewArg::Timeline => ViewKind::Timeline,
            ViewArg::Yrcal => ViewKind::CalendarWithYears,
        }
    }
}

/// The accepted `--format` values. `All` expands to the full four when
/// folded into a bitflag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    Csv,
    Json,
    Parquet,
    Pdf,
    All,
}

/// Fold the clap `--format` values into a [`Formats`] bitflag. Any `all`
/// selects every format (plan §11: `--format all ==` exactly the four);
/// repeated/overlapping values are idempotent bitwise.
pub fn formats_from(args: &[FormatArg]) -> Formats {
    let mut acc = Formats::NONE;
    for a in args {
        let bit = match a {
            FormatArg::Csv => Formats::CSV,
            FormatArg::Json => Formats::JSON,
            FormatArg::Parquet => Formats::PARQUET,
            FormatArg::Pdf => Formats::PDF,
            FormatArg::All => Formats::ALL,
        };
        acc |= bit;
    }
    acc
}

/// Build the run's [`ReportOptions`] from the CLI filters (plan §7.3).
///
/// The reference year defaults to the current year (plan §8.4) when
/// `--reference-year` is absent; every other field keeps its plan default.
pub fn report_options(f: &CommonFilters) -> anyhow::Result<ReportOptions> {
    let reference_year = match f.reference_year {
        Some(y) => y,
        None => ReportOptions::default().reference_year,
    };
    let mut opts = ReportOptions::with_reference_year(reference_year);

    if !f.include_types.is_empty() {
        let mut set = HashSet::new();
        set.extend(f.include_types.iter().cloned());
        opts.include_types = Some(set);
    }
    opts.exclude_types = f.exclude_types.iter().cloned().collect();
    opts.living_only = f.living_only;
    opts.include_private = f.include_private;
    if let Some(raw) = &f.date_range {
        opts.date_range = Some(parse_date_range(raw)?);
    }

    Ok(opts)
}

/// Parse a `--date-range START..END` window into two inclusive
/// [`jiff::civil::Date`]s (plan §7.3).
fn parse_date_range(raw: &str) -> anyhow::Result<(Date, Date)> {
    let (start_tok, end_tok) = raw
        .split_once("..")
        .ok_or_else(|| anyhow!("--date-range must be START..END with ISO dates"))?;
    let start: Date = start_tok
        .parse()
        .map_err(|_| anyhow!("invalid start date in --date-range: {start_tok:?}"))?;
    let end: Date = end_tok
        .parse()
        .map_err(|_| anyhow!("invalid end date in --date-range: {end_tok:?}"))?;
    if end < start {
        bail!("--date-range end precedes start");
    }
    Ok((start, end))
}

/// The default output filename prefix for `report`: the input file's stem
/// (e.g. `data.gramps` → `data`).
pub fn default_out_prefix(file: &std::path::Path) -> String {
    file.file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "gramps-events".to_string())
}

/// Read a `.gramps` file's bytes, resolving the container and producing a
/// [`gramps_xml::Database`] (plan §7.1 `parse_database`).
pub fn load_database(path: &std::path::Path) -> anyhow::Result<gramps_xml::Database> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    gramps_xml::parse_database(&bytes)
        .map_err(|e| anyhow!("failed to parse {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    const DATA_GRAMPS: &[u8] = include_bytes!("../../../tests/fixtures/data.gramps");

    fn db() -> gramps_xml::Database {
        gramps_xml::parse_database(DATA_GRAMPS).unwrap()
    }

    #[test]
    fn no_subcommand_is_an_error() {
        // With no subcommand the binary shows its help and exits nonzero —
        // a clap parse error rather than silently doing nothing.
        assert!(Cli::try_parse_from(["gramps-events"]).is_err());
    }

    #[test]
    fn report_without_format_is_a_clap_error() {
        let err = Cli::try_parse_from(["gramps-events", "report", "x.gramps"]).unwrap_err();
        assert!(err.to_string().contains("required"));
    }

    #[test]
    fn format_all_expands_to_exactly_the_four() {
        let cli = Cli::try_parse_from(["gramps-events", "report", "x.gramps", "--format", "all"])
            .unwrap();
        let Command::Report(report) = cli.command else {
            panic!("expected report")
        };
        let fmt = formats_from(&report.format);
        assert_eq!(fmt, Formats::ALL);
    }

    #[test]
    fn format_combination_is_bitwise_and_empty_is_none() {
        let cli = Cli::try_parse_from([
            "gramps-events",
            "report",
            "x.gramps",
            "--format",
            "csv,json,pdf",
        ])
        .unwrap();
        let Command::Report(report) = cli.command else {
            panic!("expected report")
        };
        assert_eq!(
            formats_from(&report.format),
            Formats::CSV | Formats::JSON | Formats::PDF
        );
        assert!(formats_from(&[]).is_empty());
    }

    #[test]
    fn view_names_map_to_all_four_view_kinds() {
        for (token, expected) in [
            ("list", ViewKind::List),
            ("calendar", ViewKind::Calendar),
            ("timeline", ViewKind::Timeline),
            ("yrcal", ViewKind::CalendarWithYears),
        ] {
            let cli = Cli::try_parse_from(["gramps-events", "list", "x.gramps", "--view", token])
                .unwrap();
            let Command::List(list) = cli.command else {
                panic!("expected list")
            };
            assert_eq!(ViewKind::from(list.view), expected);
        }
        // yrcal is the only accepted spelling of calendar-with-years.
        assert!(
            Cli::try_parse_from(["gramps-events", "list", "x.gramps", "--view", "yr-cal"]).is_err()
        );
    }

    #[test]
    fn list_defaults_to_the_list_view() {
        let cli = Cli::try_parse_from(["gramps-events", "list", "x.gramps"]).unwrap();
        let Command::List(list) = cli.command else {
            panic!("expected list")
        };
        assert_eq!(list.view, ViewArg::List);
        assert_eq!(ViewKind::from(list.view), ViewKind::List);
    }

    #[test]
    fn filters_build_report_options() {
        let cli = Cli::try_parse_from([
            "gramps-events",
            "report",
            "x.gramps",
            "--format",
            "csv",
            "--include-types",
            "Birth,Death",
            "--exclude-types",
            "Cremation",
            "--reference-year",
            "2026",
            "--living-only",
            "--include-private",
            "--date-range",
            "1900-01-01..2026-12-31",
        ])
        .unwrap();
        let Command::Report(report) = cli.command else {
            panic!("expected report")
        };
        let opts = report_options(&report.filters).unwrap();
        assert_eq!(opts.reference_year, 2026);
        assert_eq!(
            opts.include_types,
            Some(["Birth".into(), "Death".into()].into_iter().collect())
        );
        assert!(opts.exclude_types.contains("Cremation"));
        assert!(opts.living_only);
        assert!(opts.include_private);
        assert_eq!(
            opts.date_range,
            Some((
                Date::new(1900, 1, 1).unwrap(),
                Date::new(2026, 12, 31).unwrap()
            ))
        );
    }

    #[test]
    fn type_selection_exclusion_wins_through_the_options() {
        let cli = Cli::try_parse_from([
            "gramps-events",
            "list",
            "x.gramps",
            "--include-types",
            "Birth,Death",
            "--exclude-types",
            "Death",
        ])
        .unwrap();
        let Command::List(list) = cli.command else {
            panic!("expected list")
        };
        let opts = report_options(&list.filters).unwrap();
        assert!(opts.type_allowed("Birth"));
        assert!(!opts.type_allowed("Death"), "exclusion wins over whitelist");
        assert!(!opts.type_allowed("Marriage"));
    }

    #[test]
    fn bad_date_range_is_rejected() {
        let cli = Cli::try_parse_from([
            "gramps-events",
            "report",
            "x.gramps",
            "--format",
            "csv",
            "--date-range",
            "nonsense",
        ])
        .unwrap();
        let Command::Report(r) = &cli.command else {
            panic!("expected report")
        };
        assert!(report_options(&r.filters).is_err());
    }

    #[test]
    fn default_out_prefix_uses_the_input_stem() {
        assert_eq!(
            default_out_prefix(std::path::Path::new("data.gramps")),
            "data"
        );
        assert_eq!(
            default_out_prefix(std::path::Path::new("/tmp/archive.tree.gramps")),
            "archive.tree"
        );
        assert_eq!(default_out_prefix(std::path::Path::new("noext")), "noext");
    }

    #[test]
    fn inspect_enumerates_types_and_counts() {
        use crate::run;

        let out = run::inspect(&db());
        assert!(out.contains("== Event Types =="));
        // The output contains at least Birth with a nonzero count and a
        // total counter — the exact shape is locked by the snapshot test.
        assert!(out.contains("Total events:"));
    }

    #[test]
    fn serve_defaults_to_the_plan_port_8380() {
        let cli = Cli::try_parse_from(["gramps-events", "serve"]).unwrap();
        let Command::Serve(serve) = cli.command else {
            panic!("expected serve")
        };
        assert_eq!(serve.port, 8380);
    }

    #[test]
    fn serve_accepts_an_explicit_port() {
        let cli = Cli::try_parse_from(["gramps-events", "serve", "--port", "9000"]).unwrap();
        let Command::Serve(serve) = cli.command else {
            panic!("expected serve")
        };
        assert_eq!(serve.port, 9000);
    }
}
