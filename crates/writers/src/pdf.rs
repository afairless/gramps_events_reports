//! PDF output: the [`PdfDocument`] model, [`build_pdf_document`], the
//! [`PdfBackend`] trait, and the Typst renderer (plan §6.3 / §7.4,
//! decision D2).
//!
//! Unlike the row writers — which serialize the flat
//! [`event_core::EventRow`] contract — the PDF report has its own document
//! model: a title, the run's reference year, and the twelve anniversary
//! calendar months (month → day → entries, plan §5 / §7.3).
//! [`build_pdf_document`] builds it from a view's anchored rows (§8 rules
//! 1/3); [`TypstPdf`] compiles generated Typst markup — a title block, the
//! reference year, and one calendar page per month, mirroring Gramps' own
//! report (§2) — into PDF bytes and writes them atomically (§7.4).

use std::path::Path;

use event_core::{EventRow, ReportOptions, View};
use typst::diag::{FileError, FileResult, Severity, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime};
use typst::layout::PagedDocument;
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_pdf::PdfOptions;

use crate::atomic::write_atomically;
use crate::error::WriterError;

/// The report's default title (plan §5: the model is
/// (title, months → days → entries)).
pub const DEFAULT_TITLE: &str = "Gramps Events — Anniversary Calendar";

/// The twelve month display names, January → December — the page headers
/// of the calendar-month pages (plan §2).
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The PDF report model (plan §5 / §7.3) — the document the
/// [`PdfBackend`] renderers consume. `event-core` builds views; this
/// model is the writers crate's own (the milestone's deliverable), so a
/// renderer swap (genpdf / printpdf) never touches the view builders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfDocument {
    /// The report title.
    pub title: String,
    /// The year every elapsed value is measured against (§8.4) — printed
    /// on the report so a page stays self-describing.
    pub reference_year: i32,
    /// The twelve calendar month pages, January → December, each holding
    /// only the days that have entries (plan §2). The renderer emits a
    /// page for a month without entries too — the same twelve-page shape
    /// as Gramps' report.
    pub months: Vec<PdfMonth>,
}

/// One calendar month page of the PDF report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfMonth {
    /// 1–12.
    pub month: u32,
    /// The days with entries, ascending.
    pub days: Vec<PdfDay>,
}

/// One day of a calendar month page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfDay {
    /// 1–31.
    pub day: u32,
    /// The entries on this day, in rule-12 calendar order (event type,
    /// subject, event id).
    pub entries: Vec<PdfEntry>,
}

/// One anniversary entry on a PDF day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfEntry {
    /// The subject's display name (`"—"` for orphan events, D7).
    pub person_name: String,
    /// The event type verbatim — "Birth", "Death", or any custom type.
    pub event_type: String,
    /// The Gramps display-date string.
    pub event_date_text: String,
    /// `reference_year − start year` (plan §8.4); negative (event after
    /// the report year) and unknown render `"—"`.
    pub elapsed_years: Option<i32>,
    /// Whether the Feb 29 fold applied for this row in this run (D6).
    pub leap_day_folded: bool,
}

/// The PDF counterpart of [`crate::EventWriter`] (plan §7.4): renders a
/// [`PdfDocument`] to a PDF file. Implementations write the destination
/// atomically; failures surface as [`WriterError`].
pub trait PdfBackend {
    /// Render `doc` into the file at `dest`, replacing any existing file
    /// only after the payload is complete.
    fn render(&self, doc: &PdfDocument, dest: &Path) -> Result<(), WriterError>;
}

/// Build the PDF document model from a view (plan §7.3).
///
/// The calendar pages are fed by the view's *anchored* rows — the same
/// set the anniversary calendar shows (§8 rules 1/3): rows with an
/// effective anchor land on their (month, day) cell; rows without an
/// anchor (year-only dates, non-convertible calendars, undated and
/// text-only events) never appear. All twelve month pages are present —
/// each lists only the days that have entries — and the entries of a day
/// follow the rule-12 calendar order (event type, subject, event id).
pub fn build_pdf_document(view: &View, opts: &ReportOptions) -> PdfDocument {
    // Gather the anchored rows per (month, day), ordered by the rule-12
    // calendar key first so each day's entries come out already sorted —
    // total order, never hash-dependent (§8 rule 12).
    let mut cells: Vec<(u32, u32, EventRow)> = Vec::new();
    for row in event_core::rows(view) {
        if let (Some(month), Some(day)) = (row.anniversary_month, row.anniversary_day) {
            cells.push((month, day, row));
        }
    }
    cells.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| rule_12_calendar_key(&a.2).cmp(&rule_12_calendar_key(&b.2)))
    });

    let months = (1..=12)
        .map(|month| {
            let mut days: Vec<PdfDay> = Vec::new();
            for (m, day, row) in &cells {
                if *m == month {
                    match days.last_mut() {
                        Some(day_cell) if day_cell.day == *day => {
                            day_cell.entries.push(PdfEntry::from_row(row.clone()));
                        }
                        _ => days.push(PdfDay {
                            day: *day,
                            entries: vec![PdfEntry::from_row(row.clone())],
                        }),
                    }
                }
            }
            PdfMonth { month, days }
        })
        .collect();

    PdfDocument {
        title: DEFAULT_TITLE.to_string(),
        reference_year: opts.reference_year,
        months,
    }
}

/// The rule-12 calendar order key — (event type, subject, event id) —
/// applied per day cell (plan §8 rule 12).
fn rule_12_calendar_key(row: &EventRow) -> (String, String, Option<String>, Option<String>) {
    (
        row.event_type.clone(),
        row.person_name.clone(),
        row.event_id.clone(),
        row.person_id.clone(),
    )
}

impl PdfEntry {
    /// Convert a row (an event as seen by one subject) into the entry the
    /// calendar page shows.
    fn from_row(row: EventRow) -> Self {
        Self {
            person_name: row.person_name,
            event_type: row.event_type,
            event_date_text: row.event_date_text,
            elapsed_years: row.elapsed_years,
            leap_day_folded: row.leap_day_folded,
        }
    }
}

/// The rendered line for one PDF entry, e.g.
/// `Alice — Birth (2020-01-01) · 6 years`. The date parenthetical is the
/// Gramps display string; a Feb 29 fold adds a `· Feb 29` label so the
/// fold stays visible (D6); the elapsed value follows §8.4 (`"—"` for
/// future events / unknown years, `"this year"` for zero).
fn entry_line(entry: &PdfEntry) -> String {
    let mut line = format!(
        "{} — {} ({})",
        entry.person_name, entry.event_type, entry.event_date_text
    );
    if entry.leap_day_folded {
        line.push_str(" · Feb 29");
    }
    line.push_str(" · ");
    line.push_str(&format_elapsed(entry.elapsed_years));
    line
}

/// §8.4 elapsed rendering: negative and unknown render `"—"`, zero
/// renders `"this year"`, otherwise `"{n} year(s)"`.
fn format_elapsed(elapsed: Option<i32>) -> String {
    match elapsed {
        None => "—".to_string(),
        Some(n) if n < 0 => "—".to_string(),
        Some(0) => "this year".to_string(),
        Some(1) => "1 year".to_string(),
        Some(n) => format!("{n} years"),
    }
}

/// The Typst PDF backend (plan §6.3 alt. A / decision D2): compiles
/// generated Typst markup — a title block, the reference year, and one
/// calendar page per month — into a PDF and writes it atomically.
///
/// The backend is the plan's recommended one; genpdf / printpdf
/// implementations of [`PdfBackend`] can slot in later without touching
/// the model or its callers.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TypstPdf;

/// The renderer's output: the PDF bytes, the laid-out page count, and
/// any non-fatal diagnostics typst emitted while compiling. Exposed so
/// callers (and tests) can check the page structure of what gets written
/// and inspect what typst warned about (plan §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPdf {
    /// The complete PDF file bytes (`%PDF-…%%EOF`).
    pub bytes: Vec<u8>,
    /// The number of laid-out pages.
    pub pages: usize,
    /// Non-fatal typst compile warnings, one string per diagnostic —
    /// e.g. `warning: unknown font family: libertinus serif`. Warnings
    /// never fail a compile (only errors do), so callers can log or
    /// inspect them without losing the rendered output.
    pub warnings: Vec<String>,
}

impl PdfBackend for TypstPdf {
    fn render(&self, doc: &PdfDocument, dest: &Path) -> Result<(), WriterError> {
        let rendered = self.compile(doc)?;
        write_atomically(dest, |mut file| {
            use std::io::Write as _;
            file.write_all(&rendered.bytes)
                .map_err(|e| WriterError::io(dest, e))?;
            Ok(())
        })
    }
}

impl TypstPdf {
    /// Compile `doc` into PDF bytes and report the laid-out page count
    /// plus any typst warnings.
    ///
    /// Failures surface as [`WriterError::Pdf`] carrying the typst
    /// diagnostics. The generated markup is internal (never user input),
    /// so a compile failure here means the renderer regressed. Warnings —
    /// e.g. an unresolvable font family — never fail the compile; they
    /// land on [`RenderedPdf::warnings`] for callers to surface (plan
    /// §5.2).
    pub fn compile(&self, doc: &PdfDocument) -> Result<RenderedPdf, WriterError> {
        Self::compile_markup(build_markup(doc))
    }

    /// Compile raw Typst markup into [`RenderedPdf`]: the pipeline behind
    /// [`TypstPdf::compile`] — build the world, compile, export, and
    /// collect the compile warnings. The markup seam exists so the
    /// warning path is testable: the public entry's generated markup
    /// always names a bundled font family, which produces no
    /// diagnostics.
    fn compile_markup(markup: String) -> Result<RenderedPdf, WriterError> {
        let world = PdfWorld::new(markup)?;
        let warned = typst::compile::<PagedDocument>(&world);
        let document = warned.output.map_err(|diagnostics| {
            WriterError::Pdf(format!(
                "typst compile failed: {}",
                format_diagnostics(&diagnostics)
            ))
        })?;
        let bytes = typst_pdf::pdf(&document, &PdfOptions::default()).map_err(|diagnostics| {
            WriterError::Pdf(format!(
                "typst pdf export failed: {}",
                format_diagnostics(&diagnostics)
            ))
        })?;
        Ok(RenderedPdf {
            pages: document.pages.len(),
            bytes,
            warnings: warned
                .warnings
                .as_slice()
                .iter()
                .map(format_diagnostic)
                .collect(),
        })
    }
}

/// Render the generated Typst markup for `doc`. All dynamic text — title,
/// month names, entry lines — is passed as string-literal variables and
/// interpolated with `#(...)`, so user data (names, types, dates) renders
/// literally and is never parsed as markup code.
fn build_markup(doc: &PdfDocument) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "#set document(title: {})\n",
        typst_string(&doc.title)
    ));
    out.push_str("#set page(paper: \"a4\", margin: (x: 1.75cm, y: 1.75cm), footer: context [Page #counter(page).display()])\n");
    out.push_str("#set text(size: 10pt, font: \"Libertinus Serif\")\n\n");

    out.push_str(&format!("#let title = {}\n", typst_string(&doc.title)));
    out.push_str(&format!("#let year = {}\n\n", doc.reference_year));

    // The month table: (name, days → (day number, entry lines)).
    out.push_str("#let months = (\n");
    for month in &doc.months {
        out.push_str("  (name: ");
        out.push_str(&typst_string(MONTH_NAMES[(month.month - 1) as usize]));
        out.push_str(", days: (");
        for day in &month.days {
            out.push_str("(day: ");
            out.push_str(&day.day.to_string());
            out.push_str(", entries: (");
            for entry in &day.entries {
                out.push_str(&typst_string(&entry_line(entry)));
                out.push(',');
            }
            out.push_str(")),");
        }
        out.push_str(")),\n");
    }
    out.push_str(")\n\n");

    out.push_str("#align(center)[\n");
    out.push_str("  #text(size: 16pt, weight: \"bold\")[#title]\n");
    out.push_str("  #v(6pt)\n");
    out.push_str("  #text(size: 11pt)[Anniversary calendar — reference year #year]\n");
    out.push_str("]\n\n");

    // One page per calendar month (the Gramps report shape, plan §2): the
    // title block shares January's page — the first month needs no page break.
    out.push_str("#for i in range(months.len()) [\n");
    out.push_str("  #if i > 0 [ #pagebreak() ]\n");
    out.push_str("  #let month = months.at(i)\n");
    out.push_str("  #text(size: 13pt, weight: \"bold\")[#(month.name)]\n");
    out.push_str("  #v(4pt)\n");
    out.push_str("  #for day in month.days [\n");
    out.push_str("    #strong[#(day.day) ]\n");
    out.push_str("    #v(2pt)\n");
    out.push_str("    #for entry in day.entries [\n");
    out.push_str("      - #(entry)\n");
    out.push_str("    ]\n");
    out.push_str("    #v(5pt)\n");
    out.push_str("  ]\n");
    out.push_str("]\n");
    out
}

/// Quote `s` as a Typst string literal (double quotes; backslash and
/// quote escaped), so dynamic text becomes a value in the generated
/// markup, never code.
fn typst_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Render one compiler diagnostic as a human-readable string, e.g.
/// `warning: unknown font family: libertinus serif`, appended by any
/// hints. [`RenderedPdf::warnings`] carries one such string per warning;
/// the error paths join them with [`format_diagnostics`].
fn format_diagnostic(d: &SourceDiagnostic) -> String {
    let severity = match d.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("{severity}: {}", d.message));
    for hint in &d.hints {
        parts.push(format!("hint: {hint}"));
    }
    parts.join("; ")
}

/// Join a compiler/export diagnostic list into one human-readable
/// message.
fn format_diagnostics(diagnostics: &[SourceDiagnostic]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for d in diagnostics {
        parts.push(format_diagnostic(d));
    }
    parts.join("; ")
}

/// The minimal Typst [`World`]: one compiled source (the generated
/// markup) plus the bundled fonts — every other loading operation fails
/// cleanly. `today` returns `None`, so the compiled document never
/// depends on the clock or the filesystem: the same `PdfDocument` always
/// produces the same bytes.
#[derive(Debug)]
struct PdfWorld {
    /// The standard typst library.
    library: LazyHash<Library>,
    /// Metadata of the bundled fonts.
    book: LazyHash<FontBook>,
    /// The bundled fonts, indexed by the font book.
    fonts: Vec<Font>,
    /// The generated main source.
    main: Source,
}

impl PdfWorld {
    /// Build a world around the generated `markup` string, loading the
    /// bundled fonts. Fails fast if none registered — an empty font book
    /// silently drops every text glyph at layout (valid-but-blank PDFs),
    /// so the renderer refuses to compile rather than emit a blank
    /// document.
    fn new(markup: String) -> Result<Self, WriterError> {
        let fonts: Vec<Font> = typst_assets::fonts()
            .filter_map(|data| Font::new(Bytes::new(data), 0))
            .collect();
        let book = FontBook::from_fonts(&fonts);
        Self::with_fonts(markup, fonts, book)
    }

    /// Assemble a world from already-loaded `fonts`, enforcing the
    /// invariant that the font book is never empty: without fonts every
    /// text glyph is silently dropped at layout, so this is a hard error
    /// rather than a blank export. The explicit-font seam lets tests poke
    /// the guard — with the bundled-font feature on, `new`'s failure
    /// branch is unreachable.
    fn with_fonts(markup: String, fonts: Vec<Font>, book: FontBook) -> Result<Self, WriterError> {
        if fonts.is_empty() {
            return Err(WriterError::Pdf(
                "no fonts loaded — the typst-assets `fonts` feature is disabled".to_string(),
            ));
        }
        let main = Source::new(FileId::new(None, VirtualPath::new("main.typ")), markup);
        Ok(Self {
            library: LazyHash::new(Library::default()),
            book: LazyHash::new(book),
            fonts,
            main,
        })
    }
}

impl World for PdfWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(FileError::NotFound(
                id.vpath().as_rooted_path().to_path_buf(),
            ))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        Err(FileError::NotFound(
            id.vpath().as_rooted_path().to_path_buf(),
        ))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<i64>) -> Option<Datetime> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_core::{CalendarDay, CalendarMonth, CalendarView};
    use std::fs;
    use tempfile::TempDir;
    use typst::layout::{Frame, FrameItem};
    use typst::text::{FontStretch, FontStyle, FontVariant, FontWeight};

    /// A row with a fixed anniversary anchor (calendar-view rows carry the
    /// effective anchor after the fold, D6).
    fn calendar_row(
        event_type: &str,
        person_name: &str,
        event_id: &str,
        anniversary: (u32, u32),
        elapsed_years: Option<i32>,
        leap_day_folded: bool,
        reference_year: i32,
    ) -> EventRow {
        EventRow {
            person_id: Some("I0001".to_string()),
            person_id_2: None,
            person_name: person_name.to_string(),
            event_id: Some(event_id.to_string()),
            event_type: event_type.to_string(),
            event_date: Some("2020-01-14".to_string()),
            event_date_text: "2020-01-14".to_string(),
            event_date_stop: None,
            date_is_range: false,
            year: Some(2020),
            month: Some(1),
            day: Some(14),
            anniversary_month: Some(anniversary.0),
            anniversary_day: Some(anniversary.1),
            leap_day_folded,
            place: None,
            role: "Primary".to_string(),
            age_at_event: None,
            reference_year,
            elapsed_years,
            private: false,
        }
    }

    /// A small calendar view: January 14 holds a birth and a year-only
    /// row (no anchor) that must not reach the PDF; December 3 a death.
    fn calendar_view() -> View {
        let mut no_anchor = calendar_row("Birth", "Carol", "E0003", (1, 14), None, false, 2026);
        no_anchor.anniversary_month = None;
        no_anchor.anniversary_day = None;
        no_anchor.year = Some(1990);
        no_anchor.month = None;
        no_anchor.day = None;

        let day = CalendarDay {
            day: 14,
            entries: vec![
                calendar_row("Birth", "Alice", "E0001", (1, 14), Some(6), false, 2026),
                no_anchor,
            ],
        };
        let month = CalendarMonth {
            month: 1,
            days: vec![day],
        };
        let day2 = CalendarDay {
            day: 3,
            entries: vec![calendar_row(
                "Death",
                "Bob",
                "E0002",
                (12, 3),
                Some(5),
                false,
                2026,
            )],
        };
        let month2 = CalendarMonth {
            month: 12,
            days: vec![day2],
        };
        View::Calendar(CalendarView {
            months: vec![month, month2],
        })
    }

    #[test]
    fn build_pdf_document_groups_anchored_rows_into_month_days() {
        let opts = ReportOptions::with_reference_year(2026);
        let doc = build_pdf_document(&calendar_view(), &opts);

        // Title and reference year (§8.4) ride on the model.
        assert_eq!(doc.title, DEFAULT_TITLE);
        assert_eq!(doc.reference_year, 2026);

        // All twelve months are present, January → December.
        assert_eq!(doc.months.len(), 12);
        for (i, month) in doc.months.iter().enumerate() {
            assert_eq!(month.month, (i + 1) as u32);
        }

        // January holds the day-14 birth; December the day-3 death;
        // months without entries hold empty day lists.
        let january = &doc.months[0];
        assert_eq!(january.days.len(), 1);
        assert_eq!(january.days[0].day, 14);
        let january_entry = &january.days[0].entries[0];
        assert_eq!(january_entry.person_name, "Alice");
        assert_eq!(january_entry.event_type, "Birth");
        assert_eq!(january_entry.event_date_text, "2020-01-14");
        assert_eq!(january_entry.elapsed_years, Some(6));

        let december = &doc.months[11];
        assert_eq!(december.days[0].day, 3);
        assert_eq!(december.days[0].entries[0].person_name, "Bob");

        for (i, month) in doc.months.iter().enumerate() {
            if i != 0 && i != 11 {
                assert!(
                    month.days.is_empty(),
                    "month {} should be empty",
                    month.month
                );
            }
        }

        // The year-only row has no anchor (rules 1/3) and never reaches
        // the PDF model.
        let all_entries: usize = doc
            .months
            .iter()
            .map(|m| m.days.iter().map(|d| d.entries.len()).sum::<usize>())
            .sum();
        assert_eq!(all_entries, 2);
    }

    #[test]
    fn entries_within_a_day_follow_rule_12_order() {
        let opts = ReportOptions::with_reference_year(2026);
        // Same day keeps (event type, subject, event id) order regardless
        // of arrival order.
        let mut rows = vec![
            calendar_row("Birth", "Zed", "E0001", (1, 14), Some(6), false, 2026),
            calendar_row("Birth", "Zed", "E0002", (1, 14), Some(6), false, 2026),
            calendar_row("Birth", "Ada", "E0003", (1, 14), Some(6), false, 2026),
        ];
        // Deliberately unordered input cells: the builder sorts.
        rows.swap(0, 1);
        let day = CalendarDay {
            day: 14,
            entries: rows,
        };
        let view = View::Calendar(CalendarView {
            months: vec![CalendarMonth {
                month: 1,
                days: vec![day],
            }],
        });

        let doc = build_pdf_document(&view, &opts);
        let entries = &doc.months[0].days[0].entries;
        let order: Vec<&str> = entries.iter().map(|e| e.person_name.as_str()).collect();
        assert_eq!(order, vec!["Ada", "Zed", "Zed"]);
        assert_eq!(entries[1].event_type, "Birth");
        assert_eq!(entries[1].person_name, "Zed");
    }

    #[test]
    fn leap_day_fold_is_labeled_and_elapsed_follows_rule_4() {
        let folded = calendar_row("Birth", "Jane", "E0004", (2, 28), Some(1), true, 2021);
        let line = entry_line(&PdfEntry::from_row(folded));
        assert!(line.contains("Feb 29"), "fold label in: {line}");
        assert!(line.ends_with("1 year"), "elapsed in: {line}");

        let now = calendar_row("Birth", "Jane", "E0004", (2, 28), Some(0), false, 2021);
        let line = entry_line(&PdfEntry::from_row(now));
        assert!(line.ends_with("this year"), "elapsed in: {line}");

        let future = calendar_row("Birth", "Jane", "E0004", (2, 28), Some(-3), false, 2021);
        let line = entry_line(&PdfEntry::from_row(future));
        assert!(line.ends_with('—'), "future elapsed in: {line}");

        assert_eq!(format_elapsed(None), "—");
        assert_eq!(format_elapsed(Some(0)), "this year");
        assert_eq!(format_elapsed(Some(1)), "1 year");
        assert_eq!(format_elapsed(Some(6)), "6 years");
        assert_eq!(format_elapsed(Some(-1)), "—");
    }

    #[test]
    fn entry_line_formats_person_type_date_and_elapsed() {
        let entry = PdfEntry {
            person_name: "Abraham Meowser".to_string(),
            event_type: "Death".to_string(),
            event_date_text: "2020-12-03".to_string(),
            elapsed_years: Some(6),
            leap_day_folded: false,
        };
        assert_eq!(
            entry_line(&entry),
            "Abraham Meowser — Death (2020-12-03) · 6 years"
        );
    }

    #[test]
    fn typst_string_escapes_quotes_and_backslashes() {
        assert_eq!(typst_string("plain"), "\"plain\"");
        assert_eq!(typst_string("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(typst_string("a\\b"), "\"a\\\\b\"");
    }

    #[test]
    fn bundled_fonts_fill_the_worlds_font_book() {
        // The workspace enables typst-assets' `fonts` feature; without it
        // `typst_assets::fonts()` yields an empty iterator and every text
        // glyph is silently dropped at layout (blank-PDF regression).
        let world = PdfWorld::new("".to_string()).expect("bundled fonts must load");
        assert!(
            !world.fonts.is_empty(),
            "the typst-assets `fonts` feature must load bundled fonts"
        );

        // The generated markup sets `font: "Libertinus Serif"`, so the
        // family must resolve from the book exactly as `TypstPdf` will
        // look it up at layout.
        let book = FontBook::from_fonts(&world.fonts);
        assert!(
            book.contains_family("libertinus serif"),
            "Libertinus Serif must be registered in the font book"
        );
        assert!(
            book.select(
                "libertinus serif",
                FontVariant::new(
                    FontStyle::default(),
                    FontWeight::default(),
                    FontStretch::default(),
                ),
            )
            .is_some(),
            "Libertinus Serif must resolve from the font book"
        );
    }

    #[test]
    fn empty_font_book_fails_fast_instead_of_rendering_blank_pages() {
        // The guard branch in `new` is unreachable once fonts are bundled
        // (`bundled_fonts_fill_the_worlds_font_book` proves they load),
        // so poke the invariant through the explicit-font seam with an
        // empty vector — the blank-PDF regression's precise precondition.
        let err = PdfWorld::with_fonts("".to_string(), Vec::new(), FontBook::new())
            .expect_err("an empty font book must be rejected, not rendered blank");
        let WriterError::Pdf(msg) = &err else {
            panic!("expected WriterError::Pdf, got {err}");
        };
        assert_eq!(
            msg,
            "no fonts loaded — the typst-assets `fonts` feature is disabled"
        );
    }

    /// Depth-first collect the plain text of every laid-out text run in
    /// `frame` (plan §5.3 test 2). Content nests arbitrarily, so recurse
    /// through [`FrameItem::Group`] children and collect the string of
    /// every [`FrameItem::Text`] run.
    fn frame_texts(frame: &Frame) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (_, item) in frame.items() {
            match item {
                FrameItem::Text(text) => out.push(text.text.as_str().to_string()),
                FrameItem::Group(group) => {
                    for nested in frame_texts(&group.frame) {
                        out.push(nested);
                    }
                }
                _ => {}
            }
        }
        out
    }

    #[test]
    fn render_pdf_has_magic_and_the_twelve_calendar_pages() {
        let opts = ReportOptions::with_reference_year(2026);
        let doc = build_pdf_document(&calendar_view(), &opts);
        let rendered = TypstPdf
            .compile(&doc)
            .expect("typst must compile the report");

        assert!(
            rendered.bytes.starts_with(b"%PDF-"),
            "PDF magic header missing"
        );
        assert!(
            rendered.bytes.windows(5).any(|w| w == b"%%EOF"),
            "PDF must end with the EOF marker"
        );
        // One page per month — the Gramps report shape.
        assert_eq!(rendered.pages, 12, "one page per calendar month");
    }

    #[test]
    fn compiled_frames_contain_the_title_and_entry_text() {
        // The blank-PDF regression's telltale: the 12 pages exist but the
        // laid-out frames hold no text at all. Compile the document and
        // walk page 1's frame — the title block shares January's page —
        // requiring the title, the month header *and* at least one entry
        // text run. With the empty font book that produced blank pages,
        // every glyph is dropped and no `Text` item ever appears.
        let opts = ReportOptions::with_reference_year(2026);
        let doc = build_pdf_document(&calendar_view(), &opts);

        let world = PdfWorld::new(build_markup(&doc)).expect("bundled fonts must load");
        let warned = typst::compile::<PagedDocument>(&world);
        let document = warned.output.expect("typst must compile the report");
        assert_eq!(document.pages.len(), 12, "one page per calendar month");

        let page_1_texts = frame_texts(&document.pages[0].frame);
        let page_1_preview = page_1_texts.join(" · ");
        assert!(
            page_1_texts
                .iter()
                .any(|t| t.contains("Gramps Events — Anniversary Calendar")),
            "page 1 must contain the report title: {page_1_preview}"
        );
        assert!(
            page_1_texts
                .iter()
                .any(|t| t.contains("Alice — Birth (2020-01-14)")),
            "page 1 must contain at least one entry text run: {page_1_preview}"
        );
        assert!(
            page_1_texts.iter().any(|t| t.contains("January")),
            "page 1 must contain its month header: {page_1_preview}"
        );
    }

    #[test]
    fn compiled_pdf_bytes_embed_font_resources() {
        // End-to-end content probe (plan §5.3 test 3): font objects and
        // page resource dictionaries are written uncompressed, so a raw
        // `/Font` / `FontFile` byte search discriminates the fixed export
        // (26 `/Font` hits) from the blank-PDF regression (0 hits — an
        // empty font book embeds no fonts). Content streams are
        // FlateDecode-compressed by typst-pdf 0.14.2, so text-showing
        // operators (`Tj`/`TJ`) are deliberately not scanned for.
        let opts = ReportOptions::with_reference_year(2026);
        let doc = build_pdf_document(&calendar_view(), &opts);
        let rendered = TypstPdf
            .compile(&doc)
            .expect("typst must compile the report");

        assert!(
            rendered.bytes.windows(5).any(|w| w == b"/Font"),
            "the PDF must embed font resources (/Font)"
        );
        assert!(
            rendered.bytes.windows(8).any(|w| w == b"FontFile"),
            "the PDF must embed font program data (FontFile)"
        );
    }

    #[test]
    fn bundled_default_surfaces_no_unknown_font_family_warning() {
        // The blank-PDF regression's telltale diagnostic was `unknown
        // font family: libertinus serif` — emitted at warning severity
        // and previously discarded by `compile`. With the `fonts` feature
        // on, the golden report must compile with no such warning
        // surfacing on the render result (plan §5.3 test 4).
        let opts = ReportOptions::with_reference_year(2026);
        let doc = build_pdf_document(&calendar_view(), &opts);
        let rendered = TypstPdf
            .compile(&doc)
            .expect("typst must compile the report");

        assert!(
            rendered
                .warnings
                .iter()
                .all(|w| !w.contains("unknown font family")),
            "the bundled default must not warn about missing fonts: {}",
            rendered.warnings.join(" · ")
        );
    }

    #[test]
    fn unknown_font_family_warning_surfaces_on_rendered_pdf() {
        // Asking for a font family the bundled book does not carry is a
        // *warning* in typst — the compile succeeds with a fallback
        // instead of failing — so it must land on `RenderedPdf.warnings`,
        // visible to callers, rather than being discarded. The markup
        // seam runs the exact pipeline `compile` runs.
        let markup = "#set text(font: \"Nope Chocolate Regular\")\nHello".to_string();
        let rendered = TypstPdf::compile_markup(markup)
            .expect("an unknown font family must fall back, not fail the compile");

        assert!(
            rendered.bytes.starts_with(b"%PDF-"),
            "the compile must still produce pdf bytes"
        );
        assert!(
            rendered
                .warnings
                .iter()
                .any(|w| w.contains("unknown font family")),
            "warnings must expose the unknown font family diagnostic: {}",
            rendered.warnings.join(" · ")
        );
    }

    #[test]
    fn user_data_is_rendered_literally_not_parsed_as_markup() {
        let opts = ReportOptions::with_reference_year(2026);
        // Hostile-looking names/dates must compile and render as text,
        // never as markup or code.
        let row = EventRow {
            person_id: Some("I0001".to_string()),
            person_id_2: None,
            person_name: "#[Alice \"Quoted\" *so*]".to_string(),
            event_id: Some("E0001".to_string()),
            event_type: "Birth $x$".to_string(),
            event_date: Some("2020-01-14".to_string()),
            event_date_text: "2020-01-14".to_string(),
            event_date_stop: None,
            date_is_range: false,
            year: Some(2020),
            month: Some(1),
            day: Some(14),
            anniversary_month: Some(1),
            anniversary_day: Some(14),
            leap_day_folded: false,
            place: None,
            role: "Primary".to_string(),
            age_at_event: None,
            reference_year: 2026,
            elapsed_years: Some(6),
            private: false,
        };
        let day = CalendarDay {
            day: 14,
            entries: vec![row],
        };
        let view = View::Calendar(CalendarView {
            months: vec![CalendarMonth {
                month: 1,
                days: vec![day],
            }],
        });
        let doc = build_pdf_document(&view, &opts);
        let rendered = TypstPdf
            .compile(&doc)
            .expect("hostile text must still compile");
        assert!(rendered.bytes.starts_with(b"%PDF-"));
    }

    #[test]
    fn render_writes_the_destination_atomically() {
        let opts = ReportOptions::with_reference_year(2026);
        let doc = build_pdf_document(&calendar_view(), &opts);
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.pdf");

        TypstPdf.render(&doc, &dest).unwrap();

        let bytes = fs::read(&dest).unwrap();
        assert!(bytes.starts_with(b"%PDF-"), "file holds the rendered PDF");
        assert!(
            fs::read_dir(td.path())
                .unwrap()
                .all(|e| { !e.unwrap().file_name().to_string_lossy().ends_with(".tmp") }),
            "no temp files left behind"
        );
    }
}
