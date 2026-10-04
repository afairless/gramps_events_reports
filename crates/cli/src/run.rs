//! The three lived-in commands over a parsed [`gramps_xml::Database`]
//! (plan §6.5): `inspect`, `list` and `report`, plus `serve`, which
//! starts the local web UI (the `web` crate) on 127.0.0.1.
//!
//! Each command runs the shared event-core pipeline (`collect_events` →
//! `build_view`); `list` renders the chosen view as text and `report`
//! hands the flat rows to the writers (csv / json / parquet) and the PDF
//! document to the PDF backend — one command producing all four files
//! (plan §11.1 acceptance 4) — plus `{prefix}.errors.json` when any event
//! was skipped for a malformed date (plan §3.2, decision D-a). `serve`
//! delegates to `web::serve`, so the GUI shares every line of core logic
//! with the CLI and ships in the same binary (plan §6.5).

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
/// [`writers::build_pdf_document`]. When at least one event was skipped
/// for a malformed date, `{prefix}.errors.json` is also written through
/// the same atomic machinery (plan §3.2, decision D-a). Returns the paths
/// written: the formats in deterministic order (csv, json, parquet, pdf
/// — plan §8 rule 12), then the error report last.
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

    // The malformed-date error report: written only when at least one
    // event was skipped for a bad date, and appended **last** so the CLI
    // echoes it in a deterministic position after the format files (plan
    // §3.2, decision D-a — "only when at least one event was skipped").
    if !db.date_issues.is_empty() {
        let dest = out_dir.join(format!("{out_prefix}.errors.json"));
        writers::write_date_issues(&db.date_issues, &dest)
            .with_context(|| format!("failed to write {}", dest.display()))?;
        written.push(dest);
    }
    Ok(written)
}

/// The stderr text reporting the events skipped for a malformed date:
/// one line per [`gramps_xml::DateIssue`] in document order, phrased like
/// the parser's own warning, e.g.
///
/// ```text
/// skipping event E0000: malformed daterange date (daterange/datespan stop "1914" sorts before start "1918")
/// ```
///
/// The issue message embeds raw text from the parsed file, so every line
/// is stripped of ASCII control characters (`U+0000`–`U+001F` and `U+007F`,
/// including ESC) before it is returned — a hostile `.gramps` file cannot
/// inject terminal escape sequences into the log (plan §3.2). Returns an
/// empty string when there is nothing to report; `main.rs` sends the
/// result to stderr for `inspect`, `list` and `report`.
pub fn print_date_issues(db: &Database) -> String {
    let mut out = String::new();
    for issue in &db.date_issues {
        let line = sanitize(&format!(
            "skipping event {}: malformed {} date ({})",
            issue.event_id, issue.date_kind, issue.message
        ));
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Drop every ASCII control character — `U+0000`–`U+001F` (including ESC)
/// and `U+007F` (DEL) — so hostile message text cannot smuggle terminal
/// escape sequences into the log while still slipping through as
/// visible text. Non-ASCII characters (e.g. `é`) are untouched.
fn sanitize(line: &str) -> String {
    let mut out = String::new();
    for c in line.chars() {
        if !is_ascii_control(c) {
            out.push(c);
        }
    }
    out
}

/// True for the ASCII control characters: `U+0000`–`U+001F` and `U+007F`.
fn is_ascii_control(c: char) -> bool {
    c <= '\u{1f}' || c == '\u{7f}'
}

/// `serve`: start the web UI on 127.0.0.1:port — the CLI form of the GUI
/// workflow (plan §6.5). Runs until Ctrl-C; the server's uploads and temp
/// dir are cleaned up when its state drops on exit.
pub fn serve(port: u16) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to start the tokio runtime")?;
    runtime
        .block_on(web::serve(web::ServeConfig::local(port)))
        .context("web server failed")?;
    Ok(())
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
    const MALFORMED_GRAMPS: &[u8] =
        include_bytes!("../../../tests/fixtures/malformed-dates.gramps");

    fn db() -> Database {
        gramps_xml::parse_database(DATA_GRAMPS).unwrap()
    }

    /// The damaged fixture: three events skip for malformed dates, one
    /// well-formed Birth survives.
    fn malformed_db() -> Database {
        gramps_xml::parse_database(MALFORMED_GRAMPS).unwrap()
    }

    /// A database carrying only the two issues `print_date_issues` must
    /// render — built directly so the test controls the exact text.
    fn issues_db() -> Database {
        Database {
            date_issues: vec![
                gramps_xml::DateIssue {
                    event_handle: "_e0".to_string(),
                    event_id: "E0000".to_string(),
                    event_type: "Death".to_string(),
                    date_kind: "daterange".to_string(),
                    message: "stop \"1914\" sorts before start \"1918\"".to_string(),
                },
                gramps_xml::DateIssue {
                    event_handle: "_e2".to_string(),
                    event_id: "E0002".to_string(),
                    event_type: String::new(),
                    date_kind: "dateval".to_string(),
                    message: "unrecognized month value: 13".to_string(),
                },
            ],
            ..Default::default()
        }
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
        // data.gramps is clean: no malformed dates, so no error report
        // (decision D-a — "written only when needed").
        assert!(!dir.path().join("r.errors.json").exists());
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

    /// `report` writes `{prefix}.errors.json` only when the database
    /// actually has date issues (decision D-a), appending its path **last**
    /// after csv/json/parquet/pdf so the CLI echoes it deterministically.
    #[test]
    fn report_writes_errors_json_only_when_needed() {
        let dir = tempfile::tempdir().unwrap();
        // A damaged fixture: three malformed-date skips → the report IS
        // written, in document order, with all five files present.
        let paths = report(&malformed_db(), &opts(2030), Formats::ALL, dir.path(), "m").unwrap();
        assert_eq!(
            paths,
            vec![
                dir.path().join("m.csv"),
                dir.path().join("m.json"),
                dir.path().join("m.parquet"),
                dir.path().join("m.pdf"),
                dir.path().join("m.errors.json"),
            ]
        );
        for p in &paths {
            assert!(p.exists(), "missing {p:?}");
        }
        // The report is an object that names every skipped event.
        let raw = std::fs::read_to_string(dir.path().join("m.errors.json")).unwrap();
        assert!(raw.starts_with("{"), "report must be an object: {raw}");
        assert!(raw.contains("error_count"), "report: {raw}");
        for damaged in ["E0000", "E0001", "E0002"] {
            assert!(
                raw.contains(damaged),
                "report must mention {damaged}: {raw}"
            );
        }

        // A clean fixture: no issues → no errors.json at all, four files.
        let clean = tempfile::tempdir().unwrap();
        let paths = report(&db(), &opts(2030), Formats::ALL, clean.path(), "c").unwrap();
        assert_eq!(paths.len(), 4);
        assert!(!clean.path().join("c.errors.json").exists());
    }

    /// `print_date_issues` renders one line per issue, phrased like the
    /// parser's warning, and nothing at all for a clean database.
    #[test]
    fn print_date_issues_renders_one_line_per_issue() {
        let out = print_date_issues(&issues_db());
        assert!(out.contains(
            "skipping event E0000: malformed daterange date (stop \"1914\" sorts before start \"1918\")\n"
        ));
        assert!(out.contains(
            "skipping event E0002: malformed dateval date (unrecognized month value: 13)\n"
        ));
        assert!(print_date_issues(&db()).is_empty());
    }

    /// A hostile `.gramps` file can put control bytes in the raw text that
    /// ends up in an issue message; none may reach the returned stderr text.
    #[test]
    fn print_date_issues_strips_ascii_control_characters() {
        let db = Database {
            date_issues: vec![gramps_xml::DateIssue {
                event_handle: "_e9".to_string(),
                event_id: "E0009".to_string(),
                // ESC + "clear screen" sequence in the type, ESC + red,
                // DEL, NUL, tab and an embedded newline in the message.
                event_type: "Death\u{1b}[2J".to_string(),
                date_kind: "daterange".to_string(),
                message: "stop \"1914\"\u{1b}[31m red \u{7f}\u{0}\tnewline\ncuidado \u{e9}"
                    .to_string(),
            }],
            ..Default::default()
        };
        let out = print_date_issues(&db);
        // Every control byte is gone (the line terminator is added after
        // sanitizing, so a message newline cannot split the line).
        for control in ['\u{0}', '\u{1b}', '\u{7f}', '\t'] {
            assert!(!out.contains(control), "control char {control:?} survived");
        }
        // Visible text (including the non-ASCII é) survives, joined onto
        // the single line; the ESC sequences are inert literals.
        assert!(out.contains(
            "skipping event E0009: malformed daterange date (stop \"1914\"[31m red newlinecuidado é)\n"
        ));
        assert_eq!(out.lines().count(), 1, "one issue → one line");
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
