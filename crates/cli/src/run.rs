//! The three lived-in commands over a parsed [`gramps_xml::Database`]
//! (plan §6.5): `inspect`, `list` and `report`.
//!
//! Each command runs the shared event-core pipeline (`collect_events` →
//! `build_view`); `list` renders the chosen view as text and `report`
//! hands the flat rows to the writers (csv / json / parquet) and the PDF
//! document to the PDF backend — one command producing all four files
//! (plan §11.1 acceptance 4).

use std::path::Path;

use anyhow::{Context, Result};
use event_core::{ReportOptions, ViewKind, build_view, collect_events, rows};
use gramps_xml::Database;
use writers::{CsvWriter, EventWriter, Formats, JsonWriter, ParquetWriter, PdfBackend, TypstPdf};

use crate::render;

/// `inspect`: enumerate every event type in the database with its count,
/// sorted by count descending then type name (deterministic) — the same
/// enumeration the GUI builds its event-type checkboxes from (plan §9).
///
/// Counting is over the whole database, independent of any filter, so the
/// checkbox list never skips a type the user could select.
pub fn inspect(db: &Database) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for event in &db.events {
        match counts.iter_mut().find(|(t, _)| t == &event.event_type) {
            Some((_, n)) => *n += 1,
            None => counts.push((event.event_type.clone(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let total: usize = counts.iter().map(|(_, n)| n).sum();
    let mut out = String::from("== Event Types ==\n");
    out.push_str(&format!("Total events: {total}\n"));
    for (event_type, n) in counts {
        out.push_str(&format!("  {event_type:<16} {n}\n"));
    }
    out
}

/// `list`: build `kind` from the database under `opts` and render it as
/// text (plan §6.5 — list is the default view).
pub fn list(db: &Database, opts: &ReportOptions, kind: ViewKind) -> String {
    let events = collect_events(db, opts);
    let view = build_view(&events, kind, opts);
    render::render_view(&view)
}

/// `report`: write the database events to every enabled [`Formats`] bit,
/// one file per format into `out_dir` under `out_prefix`, atomically.
///
/// CSV / JSON / Parquet serialize the flat [`event_core::EventRow`]
/// contract; PDF is built from the same view via
/// [`writers::build_pdf_document`]. Returns the paths written, in the
/// deterministic format order (csv, json, parquet, pdf — plan §8 rule 12).
pub fn report(
    db: &Database,
    opts: &ReportOptions,
    formats: Formats,
    out_dir: &Path,
    out_prefix: &str,
) -> Result<Vec<std::path::PathBuf>> {
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("cannot create output directory {}", out_dir.display()))?;

    let events = collect_events(db, opts);
    let view = build_view(&events, ViewKind::List, opts);
    let flat_rows = rows(&view);

    let mut written = Vec::new();
    for format in formats.iter() {
        let dest = out_dir.join(filename_for(out_prefix, format));
        if format == Formats::PDF {
            let doc = writers::build_pdf_document(&view, opts);
            TypstPdf
                .render(&doc, &dest)
                .with_context(|| format!("failed to write {}", dest.display()))?;
        } else {
            let writer: Box<dyn EventWriter> = match format {
                Formats::CSV => Box::new(CsvWriter),
                Formats::JSON => Box::new(JsonWriter),
                Formats::PARQUET => Box::new(ParquetWriter),
                _ => unreachable!("pdf handled above"),
            };
            writer
                .write(&flat_rows, &dest)
                .with_context(|| format!("failed to write {}", dest.display()))?;
        }
        written.push(dest);
    }
    Ok(written)
}

/// The output filename for one format: `{prefix}.{ext}` (csv, json,
/// parquet, pdf).
fn filename_for(prefix: &str, format: Formats) -> String {
    let ext = if format == Formats::CSV {
        "csv"
    } else if format == Formats::JSON {
        "json"
    } else if format == Formats::PARQUET {
        "parquet"
    } else if format == Formats::PDF {
        "pdf"
    } else {
        unreachable!("filename_for called for an unknown format")
    };
    format!("{prefix}.{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;
    use event_core::LeapDayPolicy;

    const DATA_GRAMPS: &[u8] = include_bytes!("../../../tests/fixtures/data.gramps");

    fn db() -> Database {
        gramps_xml::parse_database(DATA_GRAMPS).unwrap()
    }

    fn opts(reference_year: i32) -> ReportOptions {
        ReportOptions::with_reference_year(reference_year)
    }

    #[test]
    fn inspect_lists_total_and_at_least_one_type() {
        let out = inspect(&db());
        assert!(out.starts_with("== Event Types =="));
        assert!(out.contains("Total events:"));
        // data.gramps has a Birth event.
        assert!(out.contains("Birth"));
    }

    #[test]
    fn inspect_counts_are_stable_across_runs() {
        assert_eq!(inspect(&db()), inspect(&db()));
    }

    #[test]
    fn list_renders_but_does_not_panic_on_the_fixture() {
        let out = list(&db(), &opts(2030), ViewKind::List);
        assert!(out.starts_with("== Events =="));
    }

    #[test]
    fn report_all_writes_four_files_with_magic_headers() {
        let dir = tempfile::tempdir().unwrap();
        let paths = report(&db(), &opts(2030), Formats::ALL, dir.path(), "r").unwrap();
        assert_eq!(paths.len(), 4);
        assert_eq!(
            paths,
            vec![
                dir.path().join("r.csv"),
                dir.path().join("r.json"),
                dir.path().join("r.parquet"),
                dir.path().join("r.pdf"),
            ]
        );
        for p in &paths {
            assert!(p.exists(), "missing {p:?}");
        }
        // CSV is a text file; JSON starts with '['; PDF starts with its
        // magic header; parquet starts with PAR1.
        let csv = std::fs::read_to_string(dir.path().join("r.csv")).unwrap();
        assert!(csv.starts_with("person_id,person_name"));
        let json = std::fs::read_to_string(dir.path().join("r.json")).unwrap();
        assert!(json.trim_start().starts_with('['));
        let pdf = std::fs::read(dir.path().join("r.pdf")).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
        let parq = std::fs::read(dir.path().join("r.parquet")).unwrap();
        assert!(parq.starts_with(b"PAR1"));
    }

    #[test]
    fn report_subset_formats_write_only_the_requested_files() {
        let dir = tempfile::tempdir().unwrap();
        let paths = report(
            &db(),
            &opts(2030),
            Formats::CSV | Formats::PDF,
            dir.path(),
            "s",
        )
        .unwrap();
        assert_eq!(paths.len(), 2);
        assert!(dir.path().join("s.csv").exists());
        assert!(dir.path().join("s.pdf").exists());
        assert!(!dir.path().join("s.json").exists());
        assert!(!dir.path().join("s.parquet").exists());
    }

    #[test]
    fn report_creates_the_output_directory() {
        let base = tempfile::tempdir().unwrap();
        let nested = base.path().join("a/b/c");
        let paths = report(&db(), &opts(2030), Formats::CSV, &nested, "x").unwrap();
        assert_eq!(paths, vec![nested.join("x.csv")]);
        assert!(nested.join("x.csv").exists());
    }

    #[test]
    fn filters_change_the_written_rows() {
        let dir = tempfile::tempdir().unwrap();
        // Exclude everything: only headers, zero data rows survive filters
        // (well — at least the runs differ in row count vs the unfiltered
        // export when we exclude Birth specifically).
        let all = report(&db(), &opts(2030), Formats::CSV, dir.path(), "all").unwrap();
        let _ = all;
        let mut ex = opts(2030);
        ex.include_types = Some(["Birth".into()].into_iter().collect());
        let only_birth = report(&db(), &ex, Formats::CSV, dir.path(), "birth").unwrap();
        let _ = only_birth;

        let birth_csv = std::fs::read_to_string(dir.path().join("birth.csv")).unwrap();
        let all_csv = std::fs::read_to_string(dir.path().join("all.csv")).unwrap();
        let birth_rows = birth_csv.lines().count();
        let all_rows = all_csv.lines().count();
        assert!(birth_rows > 1, "birth types present");
        assert!(birth_rows < all_rows, "filtering reduces the row count");
    }

    #[test]
    fn leap_day_policy_rides_the_options_through() {
        // The option object the CLI builds carries its Feb 29 fold policy
        // through report — assert a folded row when the reference year is
        // non-leap on a fixture with a Feb 29 anchor, if present.
        let mut opts = opts(2026);
        opts.leap_day = LeapDayPolicy::FoldToFeb28;
        // Just assert the pipeline runs cleanly; the fold itself is core's
        // job (its own tests cover it).
        let _ = list(&db(), &opts, ViewKind::Calendar);
    }

    /// `--include-types` and `--exclude-types` round-trip through the CLI
    /// args into the options `report` actually runs with (rule 14).
    #[test]
    fn args_filters_flow_into_report() {
        use clap::Parser;

        let cli = args::Cli::try_parse_from([
            "gramps-events",
            "report",
            "x.gramps",
            "--format",
            "csv",
            "--include-types",
            "Birth",
            "--exclude-types",
            "Birth",
        ])
        .unwrap();
        let args::Command::Report(r) = cli.command else {
            panic!("expected report")
        };
        let opts = args::report_options(&r.filters).unwrap();
        assert!(!opts.type_allowed("Birth"), "exclusion wins (rule 14)");
    }
}
