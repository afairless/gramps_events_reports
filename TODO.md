# Implementation Plan: Malformed-date tolerance, couple display, and calendar ordering

Source: `docs/research/report-fixes-dates-couples-calendar.md`

Four requested changes to `gramps-events`:

1. A malformed date on any event must not abort the run: the selected output is
   still produced, the event is omitted, the error text is printed to the
   terminal, an error-report JSON is written whenever file outputs are
   requested, and the web UI shows the offending event(s) and message(s).
2. A marriage / couple event must render as **one** row listing both spouses on
   one line, not two rows (one per spouse).
3. In the anniversary calendar (month + day, all years together), entries are
   sorted chronologically — earliest year first.
4. In that same calendar, month-only dates (day `00`, anchored on the 1st) are
   listed before day-specific dates, each category then sorted by year.

Each step is one commit-sized logical unit; run `cargo fmt`, `cargo clippy
--workspace --all-targets -- -D warnings`, and `cargo test --workspace` after
each. Confirmed decisions are recorded in the source plan §1 and §8.

## Steps

| # | Commit message | Logical unit | Key deliverables | Tests |
| --- | --- | --- | --- | --- |
| 1 | `feat(gramps-xml): record malformed-date issues instead of aborting` | Ingestion tolerance | `DateIssue` + `Database.date_issues`; `parse_event` defers **all** date-element errors, captures type, warns/skips; remove `GrampsXmlError::InvalidDate`; re-export `DateIssue`; update `parse.rs`/`error.rs` module docs that still call reversed ranges a hard error | Unit: rewrite `reversed_range_*` to `reversed_range_warns_and_skips_and_is_reported`; keep `malformed_date_warns_*`; add a fixture with both a reversed range and an invalid value; assert `event_type` is captured when `<type>` follows the broken date |
| 2 | `feat(writers): serialize malformed-date error reports` | JSON report writer | `writers/src/error_report.rs`, `write_date_issues` (atomic, deterministic) taking `&[event_core::DateIssue]`; `event-core` re-exports `DateIssue` so no new crate edge | Unit: array shape, `null`-free fields, empty-input guard, atomic rename, and a message containing quotes/backslashes/newlines round-trips |
| 3 | `feat(cli): emit {prefix}.errors.json and print date issues` | CLI output | `run::report` writes the report when needed and returns its path; `run::print_date_issues` (strips ASCII control chars); `main.rs` calls it for inspect/list/report | Integration: `report_writes_errors_json_only_when_needed`; `report_all_*` path-list updates; `print` helper unit test |
| 4 | `feat(web): surface malformed-date issues in the UI` | Web display | `DateIssueDto` + `LoadedSummary.date_errors`; `main.html` errors section + stylesheet rule; `summary()` mapping | Integration: load JSON carries `date_errors`; main page HTML contains the event id, type and message escaped; clean file renders no section |
| 5 | `refactor(event-core): mark family-couple events in resolution` | Resolution | `ResolvedEvent.couple` + `Resolution.couple`, set in branch (c)/(d); docs | Unit: `family_fixture_applies_role_precedence_and_couples` asserts `couple`; person-referenced events assert `!couple` |
| 6 | `feat(event-core): collapse couple events into one row with person_id_2` | Couple row + flat contract (whole workspace) | `EventRow.person_id_2`; `expand_rows` couple branch + `" ⚭ "` join; combined-handle dedupe; **update every `EventRow` struct literal / golden test in the workspace** (event-core, writers `csv.rs`/`parquet.rs`/`roundtrip.rs`, web tests); **Parquet `schema()` + `StructArray` column added here** so Parquet keeps compiling and round-tripping. CSV/JSON update automatically via serde — only the writers' test `HEADER` constant changes | Unit, Integration: `couple_event_collapses_to_one_row_with_both_ids`; single-spouse family keeps one row and `null` second id; `dedupe_same` on couples; row-shape golden; CSV header/value; Parquet field-list + round-trip preserves `person_id_2` |
| 7 | `feat(writers): stamp parquet schema_version metadata` | Output versioning | Build `WriterProperties` with `set_key_value_metadata` and call `ArrowWriter::try_new_with_options` (replacing today's `try_new(..., None)`); `schema_version = 2` file metadata | Unit: Parquet metadata read-back is `2`; no production CSV/JSON change |
| 8 | `fix(writers): add NewCM10 fallback so the marriage symbol renders` | PDF glyph | `build_markup` font list (`("Libertinus Serif", "NewCM10")`) | Unit: `marriage_symbol_renders_with_newcm_fallback` (PDF magic + page count + warning probe) |
| 9 | `feat(event-core): order the anniversary calendar by specificity then year` | Calendar order | Public `compare_calendar_rows`; `build_calendar` uses it; doc comments | Unit, Property: `calendar_orders_month_only_before_day_one_then_by_year`; total-order and shuffled-input determinism; update `golden_dates_calendar_*` and `calendar_view_is_a_month_day_tree` expectations; regenerate `list_data_calendar.snap` |
| 10 | `fix(writers): align the PDF calendar with the new row order` | PDF order | `build_pdf_document` uses the shared comparator | Unit: PDF month/day entry-order test with a month-only + day-1 collision |
| 11 | `test(event-core): lock calendar-with-years month-only-first ordering` | Regression only | No production change; explicit cell-order assertions | Unit: `calendar_with_years_orders_month_only_before_day_one` |
| 12 | `docs: document date tolerance, couple rows and calendar order` | Cross-cutting docs only | `docs/ARCHITECTURE.md` §1/§4/§5/§6/§8 (including the `DateIssue` re-export layering note and the CSV/JSON-vs-Parquet versioning wording), `README.md` report/error-report + couple behaviour, `tests/fixtures/README.md` | — |

## Notes

- **Ordering rationale**: step 1 introduces the ingestion data (`DateIssue`) that
  steps 2–4 consume (writer → CLI → web). Step 5 marks couple events before
  step 6 changes the flat row contract, and step 6 adds the Parquet column so
  the workspace keeps compiling/round-tripping before step 7 versions the file.
  Steps 8–11 are the PDF glyph and calendar-ordering fixes; 12 documents them.
- **Breaking changes are intentional and versioned**: `EventRow.person_id_2`
  moves the flat contract to v2 (Parquet `schema_version = 2` file metadata;
  CSV/JSON rely on field names). `GrampsXmlError::InvalidDate` is removed.
- **`RenderedPdf.warnings`** already exists (from the PDF-fonts fix) and is the
  probe seam for steps 8 and 10.
- **`date_issues` → `date_errors`** is an intentional presentation rename in the
  web DTO, not a data mismatch.
- **Snapshot regeneration**: run
  `UPDATE_SNAPSHOTS=1 cargo test -p cli --test snapshot` only after reviewing
  the diff.
