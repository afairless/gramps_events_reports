# Implementation Plan: Gramps Events Report

Source: `docs/research/gramps-events-architecture.md`

Reorders the plan's §12 milestones per two confirmed adjustments:
**dates-first ordering** (`gramps-dates` is a leaf crate consumed by `gramps-xml`, so it is built before the XML parser, avoiding deferred date fields) and **view builders split into two steps** (each step stays a small, independently testable unit).

> The repository is not yet a git repository; the first commit (`git init -b main`, then the plan documents) precedes step 1.

## Steps

| # | Commit message | Logical unit | Key deliverables | Tests |
| --- | --- | --- | --- | --- |
| 1 | chore: scaffold cargo workspace with crate skeletons and fixtures | Workspace scaffold | workspace `Cargo.toml`, `crates/{gramps-xml,gramps-dates,event-core,writers,cli,web}/`, `rustfmt.toml`, `.gitignore`, `tests/fixtures/` (copy of `data.gramps` + crafted edge-case XML: date forms, ranges, spans, partial/BC/non-Gregorian dates, orphans, family events, multi-role, private, unknown sections, zipped/gzipped containers) | Smoke |
| 2 | feat: add GrampsDate core model and dateval parsing | Core date model | `crates/gramps-dates/src/{lib,model,parse}.rs` — `GrampsDate`, `Calendar`, `Modifier`, `Quality`, `NewYear`, `DateError`, partial-date convention (month/day 0), `dateval` parsing | Unit |
| 3 | feat: parse range, span and text dates and normalize calendars | Date normalization | `crates/gramps-dates/` — `daterange`/`datespan`/`datestr` parsing, display strings, pin **jiff**, SDN Gregorian/Julian conversion (`to_gregorian`, `start`, `stop`, `anniversary_key`, `year`, `is_range`), non-Gregorian text fallback with warning | Unit, Property |
| 4 | feat: detect gramps containers and parse a minimal database | Container + skeleton parser | `crates/gramps-xml/` — `parse_database`, container detection (plain XML / gzip via flate2 / zip via zip), `Database` with `Event`/`Person`/`Family`/`Place` skeleton (with wired `GrampsDate`), `GrampsXmlError`, handle index; corrupt-container decode errors | Unit |
| 5 | feat: parse full gramps records with links and privacy | Full record parsing | `crates/gramps-xml/` — person names (multiple names, surname prefix/prim), eventref roles, family members, place hierarchy, privacy flags, `header`/`tags`, unknown-element/attribute tolerance | Unit |
| 6 | feat: resolve events to subjects in event-core | Subject resolution | `crates/event-core/` — index building (handle → record), `ResolvedEvent`, `collect_events`, subject resolution order (primary role → any role → family couple → single ref → orphan "—"), place paths, `PersonDisplay` | Unit |
| 7 | feat: compute derived values and apply the filtering pipeline | Derived values + filtering | `crates/event-core/` — `ReportOptions`, type include/exclude precedence (exclusion wins), person/date-range/living-only (`probably_alive` port)/private filters, `elapsed_years`, `age_at_event`, leap-day fold policy (D6), anniversary keys | Unit |
| 8 | feat: add EventRow contract with list and anniversary calendar views | List + calendar views | `crates/event-core/` — `EventRow` (flat writer contract incl. `event_date_stop`, `date_is_range`, `leap_day_folded`), `ListView`, `CalendarView` (anchor rules 1/3: month→day 1, year-only excluded, Feb 29 fold), `view`/`rows()` | Unit, Golden |
| 9 | feat: add timeline and calendar-with-years views | Year-based views | `crates/event-core/` — `TimelineView` (chronological, range bars start→stop, undated group), `CalendarWithYearsView` (year-by-year grid, full-extent ranges), `View`/`ViewKind` extension, deterministic output order | Unit, Property |
| 10 | feat: add csv, json and parquet writers | Row writers | `crates/writers/` — `EventWriter` trait, `CsvWriter`, `JsonWriter`, `ParquetWriter` (arrow-rs 60), `Formats` bitflag, `WriterError`, atomic temp-file → rename | Unit, Property |
| 11 | feat: add typst pdf writer | PDF writer | `crates/writers/` — `PdfDocument` model + `build_pdf_document`, `PdfBackend` trait, `TypstPdf` renderer (calendar month pages, title, reference year), `%PDF` magic + page-count check | Unit |
| 12 | feat: add report, list and inspect CLI commands | CLI | `crates/cli/` — clap derive: `inspect` (type/count enumeration), `list` with `--view` (all four views incl. `yrcal`), `report` with all filters + `--format` combos/`all`, `--out-dir`/`--out-prefix`, `--reference-year`; single command produces all four files | Snapshot, Unit |
| 13 | feat: serve gramps events over a local web api | Web API + skeleton UI | `crates/web/` — axum 0.8 bound to 127.0.0.1, routes (`/`, `/api/load`, `/api/options`, `/api/events`, `/api/events.json`, `/api/export`, `/api/reset`), 200 MB upload cap + generated temp names + cleanup, askama landing page, vendored htmx, escaping-regression test | Integration |
| 14 | feat: build the full four-view web ui | Web UI | `crates/web/` — four tabs (list, anniversary calendar, calendar-with-years, timeline) rendered by the shared view builders, event-type checkboxes with counts, orphan/privacy/leap-day toggles, export button group, CSS range bars, styling | Integration |
| 15 | chore: harden errors and document the project | Hardening + docs | error-message polish, empty/error fixtures, generated large-file benchmark (~100k events) with time/memory budget, `README.md`, `docs/ARCHITECTURE.md`, DTD-structure attribution, §11.1 acceptance checklist pass | Unit, Integration |
