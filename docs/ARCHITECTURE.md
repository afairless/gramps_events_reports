# Architecture

This document describes the **implemented** architecture of `gramps-events`,
a Rust CLI + local-web tool that reads a Gramps `.gramps` XML export, resolves
every event (birthdays, deaths, weddings, custom types) to its subjects, and
exports or displays them as a list, an anniversary calendar, a
calendar-with-years grid, or a timeline — in CSV, JSON, Parquet and PDF.

The system is specified in depth in
[`docs/research/gramps-events-architecture.md`](research/gramps-events-architecture.md)
(hereafter "the plan"). All twelve decisions D1–D12 were confirmed by the user
on 2026-09-29; this document records how the implementation realizes them and
where it deviates.

- **What**: a read-only event report generator for Gramps family trees.
- **Why this shape**: the workload is a strict *data pipeline* — ingest a
  document, transform it into resolved/filtered event rows, emit them to
  writers — so the workspace is a set of small crates with one-directional
  layering, one binary, and shared core logic between the CLI and the web UI.
- **Audience**: contributors to this codebase and reviewers of future
  changes. End-user usage lives in the root `README.md`.

---

## 1. Module map

A Cargo workspace of small crates with strict layering: each crate depends
only on "lower" crates, so ingestion never reaches into output code and the
pipeline stays one-directional.

```text
gramps-dates ──▶ gramps-xml ──▶ event-core ──▶ writers
                  ▲                │  ▲           │
                  │                │  └───────────┤ (EventRow / PdfDocument)
                  │                ▼              ▼
                  │             cli ◀─────────── web
                  └── benchgen (test-only, generates the 100k-event fixture)
```

| Crate | Responsibility | Depends on | Key types |
| --- | --- | --- | --- |
| `gramps-dates` | The Gramps date model: the four interchangeable date element forms (`dateval`/`daterange`/`datespan`/`datestr`), modifiers, quality, seven-calendar enum, display strings, Gregorian/Julian normalization via SDN, anniversary keys, leap-day fold semantics (D6). Civil-date math uses **jiff** (§6.2 of the plan). | — | `GrampsDate`, `Calendar`, `Modifier`, `Quality`, `NewYear`, `DateError` |
| `gramps-xml` | `.gramps` container detection (plain XML / gzip / zip §3.1) and the XML → typed model mapping. The DTD is the spec; records carry `handle`s, not resolved links. Recoverable defects warn-and-skip: a malformed date element records a structured [`DateIssue`] in `Database.date_issues` (plus a line in `Database::warnings`) and the event is omitted; other soft defects warn into `Database::warnings`. Hard errors (missing/duplicate handle) abort with a named record/position (§8). | `gramps-dates` | `Database`, `Event`, `Person`, `Family`, `Place`, `Header`, `Tag`, `DateIssue`, `GrampsXmlError` |
| `event-core` | The engine: `handle → record` index (plus the inverse eventref index, §5 below), subject resolution with the precedence order, derived values, the `ReportOptions` filter pipeline, the four view builders (couple events collapse to one row, §5), the flat `EventRow` writer contract and the `PdfDocument` document model. | `gramps-xml`, `gramps-dates` | `HandleIndex`, `ResolvedEvent`, `PersonDisplay`, `ReportOptions`, `LeapDayPolicy`, `View`, `ViewKind`, `ListView`, `CalendarView`, `TimelineView`, `CalendarWithYearsView`, `EventRow`, `DateIssue` (re-exported from `gramps-xml`), `compare_calendar_rows` |
| `writers` | Output backends behind traits: `EventWriter` (csv / json / parquet) with the `Formats` bitflag; `PdfBackend` (typst) rendering `PdfDocument`; `write_date_issues` serializing the malformed-date error report. Every writer is atomic: render to a temp file in the destination dir, rename over the destination only on success. The typst renderer requires the `fonts` Cargo feature on `typst-assets` — a mandatory flag that embeds ~9 MB of font data into the binary; without it `typst_assets::fonts()` returns an empty iterator and PDF export compiles **valid but blank** documents (every glyph silently dropped). See D2 for the PDF-engine decision and the [fix plan](research/fix-blank-pdf-export.md) for the root cause, hardening and verification. | `event-core` | `EventWriter`, `CsvWriter`, `JsonWriter`, `ParquetWriter`, `Formats`, `PdfBackend`, `TypstPdf`, `PdfDocument`, `WriterError`, `write_date_issues` |
| `cli` | clap 4 derive surface: `inspect`, `list`, `report`, `serve`. One binary ships CLI + GUI (§6.5); `report` runs the full workflow with any `--format` combination and, when any event was skipped for a malformed date, writes `{prefix}.errors.json` (§8). | `event-core`, `writers`, `web` | `Cli`, `Command`, `CommonFilters`, `ViewArg`, `FormatArg` |
| `web` | axum 0.8 server bound to **127.0.0.1** (D1): size-capped upload, options, view fragments, JSON dump and export routes; server-rendered UI with vendored htmx. | `event-core`, `writers` | `ServeConfig`, `AppState`, `router`, `handlers` |
| `benchgen` | Deterministic generator (fixed-seed SplitMix64) for the ~100k-event benchmark fixture — no multi-MB XML is committed. Test-only. | — (output parses through `gramps-xml`) | `BenchOptions`, `generate` |

### Layering rules

- `gramps-xml` and `gramps-dates` never see `event-core` types; `event-core`
  never sees writer or HTTP types; `writers`/`web` consume only `EventRow`,
  `DateIssue` and pre-built views/documents. This is what keeps the writers'
  flat contract stable (a Parquet schema change is breaking, §7.3 of the
  plan).
- `DateIssue` is defined in `gramps-xml` and **re-exported by `event-core`**
  (`pub use gramps_xml::DateIssue;`), so the error-report writer and the web
  DTO consume it through the same single `event-core` edge as every other
  output type — no `writers → gramps-xml` (or `web → gramps-xml`)
  dependency is introduced.
- The web UI renders the *same* view builders the CLI prints — there is one
  boundary between "transform" and "present", not two.

---

## 2. Core pipeline (data flow)

All outputs and all four GUI views consume the same resolved, filtered event
stream:

```text
 .gramps file ──▶ [container detect] ──▶ [XML parse] ──▶ Database{ raw records }
                                                              │
                     build_index(): handle→record + inverse eventref index
                                                              ▼
                       [resolve]  event ↔ subjects (precedence a–e),
                                  place paths, name display
                                                              ▼
                       [derive]   elapsed_years, age_at_event,
                                  anniversary anchor, leap-day fold (D6)
                                                              ▼
                       [passes]   ReportOptions filters: privacy, orphans,
                                  types (rule 14), people, date range, living
                                                              ▼
                         Vec<ResolvedEvent>  (one per event, deduped subjects)
                        ┌───────────┬───────────┬───────────┬───────────┐
                        ▼           ▼           ▼           ▼
                   ListView   CalendarView  TimelineView  CalendarWith
                                              (bars)     YearsView
                        │           │           │           │
                        └───── rows(&view) → Vec<EventRow> ─┘
                                    │                │
                                    ▼                ▼
                        csv/json/parquet       PdfDocument → TypstPdf
                        (atomic writers)      (calendar month pages)
```

`collect_events(db, opts)` runs resolve → derive → passes for every event in
document order; `build_view`/`rows` flatten the chosen view back to the flat
`EventRow` contract; `build_pdf_document` builds the PDF's own
month→day→entry model from the same view.

### The handle index (and why it is inverted)

`HandleIndex` maps `handle → record` for events, people, families and places.
Subject resolution additionally needs "which people/families reference this
event". A naive implementation scans `db.people`/`db.families` per event —
O(events × people) — which makes a 100k-event database hang (it hung for
minutes before the hardening milestone). `build_index` therefore also builds
the **inverse** eventref index once, in `db.people`/`db.families` document
order:

- `event → [(person_handle, best_role)]` — one entry per person per event;
  the first ref, upgraded to `Primary` when a later ref holds that role;
- `event → [(family_handle, first_ref_role)]`.

Resolution then costs O(refs) per event, keeps the same output ordering, and
the whole `report` pipeline over 100k events runs in ~13 s (debug profile) —
asserted by `crates/cli/tests/bench_large.rs` against a 30 s wall / 1.2 GiB
peak-RSS budget.

---

## 3. Key design decisions

All decisions D1–D12 are confirmed in the plan's §10. The table records what
was decided and the implementation notes:

| # | Decision | Implemented as |
| --- | --- | --- |
| D1 | Front-end framework | **Axum 0.8 + Askama + vendored htmx**, server-rendered; the only non-Rust artifact in the repo is the static htmx file. |
| D2 | PDF engine | **Typst** behind the `PdfBackend` trait. Pinned to `=0.14.2` (not the plan's 0.15) because the installed toolchain is rustc 1.91 and typst ≥ 0.15 needs 1.92 — the exact verified patch per §6.3's "pin the verified patch" note. Fonts come from `typst-assets` (bundled Libertinus), so PDF output is deterministic with no system font lookup. |
| D3 | Parquet writer | **arrow-rs** (`arrow`/`parquet` 60.x): `StructArray` → `RecordBatch` → `ArrowWriter`; round-trip tested via `ParquetRecordBatchReaderBuilder`. |
| D4 | Non-Gregorian calendars | **Gregorian + Julian only**; Hebrew/French-Republican/Persian/Islamic/Swedish degrade to text display with warnings and are excluded from anniversary math. |
| D5 | Marriages after divorce | **Show all events** — no divorce-based suppression. |
| D6 | Feb 29 | **Fold to Feb 28 in non-leap reference years, labeled as Feb 29**; rows record `leap_day_folded = true` while `event_date` keeps the true date. |
| D7 | Orphan events | **Included** by default with subject `"—"`, toggleable via `show_orphans`. |
| D8 | Zip/gzip containers | All three forms supported, detected by byte magic. |
| D9 | Range rule scope | `daterange` and `datespan` behave identically, any quality; anchor at start when the start has a month. |
| D10 | Month-only dates | Anchor at day 1 (single dates and range/span starts). |
| D11 | Timeline range rendering | Rows keep start → stop (`event_date` / `event_date_stop`, `date_is_range = true`); the web UI renders them as CSS bars spanning their full extent. |
| D12 | Calendar-with-years | Fourth view, year-by-year grid; ranges span their full extent. |

Additional implementation decisions beyond the twelve:

- **Typst version pin** (see D2) and **arrow-rs 60 pin** — both are recorded
  in the workspace `Cargo.toml` with the reasoning in comments.
- **Inverse eventref index** (§2) — a performance-enabling index added during
  hardening, keeping resolution linear.
- **Benchmark fixture is generated, not committed** (D-bench): `benchgen`
  produces ~100k events deterministically; the committed fixtures stay small.
- **`Database::warnings`** — soft parser defects (bad attribute value,
  malformed soft field, bad gender/priority, …) skip the record and are
  listed here instead of aborting the parse (plan §7.1 boundary contract).

---

## 4. The date model (`gramps-dates`)

The trickiest part of the format. `GrampsDate` is an enum-flavored struct:

- `calendar` (7 values), `modifier` (none/before/after/about/range/span/text),
  `quality` (none/estimated/calculated), `dual_dated`, `new_year`;
- `ymd: (i32, u32, u32)` with **partial-date convention** month/day `0`
  (`YYYY` / `YYYY-MM`);
- `stop` for range/span endpoints, `display` carrying what Gramps would
  print ("about 1900", "Nov 1822 – Apr 1823", …);
- `to_gregorian()` / `start()` / `stop()` → jiff civil dates; SDN
  (public-domain) conversion for Gregorian/Julian — the same algorithm
  Gramps' `gcalendar.py` uses. `anniversary_key() → Option<(month, day)>`
  implements anchor rule (§8 rules 1/3): full date → (m, d); month without
  day → (m, 1); year-only → `None`.

Ranges and spans keep both endpoints end-to-end: `event_date_stop` and
`date_is_range` travel all the way to the flat rows, so the year-based views
can render the full extent.

A month-only date normalizes with day `0`, so a flat row keeps `day = None`
(the anniversary anchor is still `(m, 1)` when the month is known, rule D10)
while a real day-1 date has `day = Some(1)` — the anniversary calendar uses
this distinction to list month-only entries before day-specific ones in the
shared day-1 cell (§5). A date element that fails to parse never aborts
the run: the event is omitted and a structured [`DateIssue`] records the
reason (§8).

---

## 5. The anniversary contract (view semantics)

The report contract is plan §8 rules 1–15. The essentials, as implemented:

- **Anchor**: a recurring (month, day) from the *normalized Gregorian* date,
  only when the date (or range/span start) has a month. Year-only events and
  year-only ranges never anchor — they appear in list, timeline and
  calendar-with-years (flagged), never in the anniversary calendar.
- **Elapsed** = `reference_year − anchor_year`, rendered `"—"` when negative
  and `"this year"` when zero.
- **Ranges/spans** (identical, D9): full range always shown; anniversary
  calendar anchors at range start; elapsed measured from start year.
- **Feb 29** folds with the D6 label.
- **Weddings / couple events**: family-owned events resolve to the couple —
  both spouses are the subjects (no divorce suppression, D5) — and
  `expand_rows` collapses them to **one** row per event (not one per
  spouse): `person_name` joins the names with `" ⚭ "` (D-c), `person_id`
  holds the first spouse and `person_id_2` the second (D-e), `null` for a
  single-spouse family. Only family/couple events collapse (D-d);
  person-referenced events keep one row per subject.
- **Subject precedence** (a→e): Primary eventrefs on people → any eventref'd
  person → family couple/single spouse → orphan `"—"` (D7). One row per
  (event, subject) — except family/couple events, which collapse to one row
  per event (above); `dedupe_same` additionally collapses identical
  (type, subject, month-day) rows, keying couple rows on the combined
  couple-handle pair.
- **Privacy**: `priv="1"` excluded by default (§8.7), included+flagged with
  `--include-private`.
- **Living-only** (§8 rule 15): a port of Gramps
  `gramps.gen.utils.alive.probably_alive` — death-type events (Death /
  Cremation / Burial) mark dead; else birth known and
  `reference_year − birth_year ≥ 110` presumes death.
- **Deterministic order** (rule 12): list/timeline by (start date, type,
  subject, event id); calendar-with-years by (start date, type, subject,
  id). The anniversary calendar sorts by the shared
  `event_core::compare_calendar_rows` comparator — (anchor month, anchor
  day, specificity, year, type, subject, event id, person id) — so inside
  the shared day-1 cell a month-only date (day `00`, anchored on the 1st)
  comes before every day-specific date, and each specificity class lists
  the earliest year first (requirements 3 & 4). The PDF backend sorts its
  calendar cells with the same comparator, so the PDF entry order always
  matches the CLI/UI calendar. Undated events sort last, in a terminal
  "Undated" group in the timeline.

---

## 6. Output contracts

`EventRow` is the immutable **v2** flat-file contract shared by CSV / JSON /
Parquet (plan §7.3): person id/name — `person_id` and `person_id_2` (the
second spouse of a collapsed couple row, D-e) — event id/type, date ISO +
display text, `event_date_stop`, `date_is_range`, year/month/day,
anniversary month/day, `leap_day_folded`, place, role, age-at-event,
reference year, elapsed, private. Serde-driven; `Option`s serialize as
`null`.

> Adding or reordering `EventRow` fields is a breaking schema change for
> Parquet readers — evolution must bump a `schema_version` or ship a new
> format. The v2 contract (adding `person_id_2`) is stamped as file-level
> `schema_version = 2` key-value metadata on every Parquet file (metadata
> only — the row shape stays flat). CSV and JSON are field-name
> self-describing, so they carry **no** version marker: the Parquet file
> metadata is the versioned surface, while CSV/JSON readers rely on the
> field names themselves.

`PdfDocument` is a separate document model (title, reference year, calendar
months → days → entries) built by event-core and rendered exclusively by the
PDF backend; the row writers never see the XML model, and the PDF path never
touches `EventRow`.

---

## 7. CLI and web surface

### CLI (`gramps-events`)

```text
gramps-events inspect <file.gramps>                  # types + counts (what the GUI enumerates)
gramps-events list    <file.gramps> [--view list|calendar|timeline|yrcal] [filters…]
gramps-events report  <file.gramps> --format csv,json,parquet,pdf|all [filters…]
                      [--out-dir DIR] [--out-prefix NAME]
gramps-events serve   [--port 8380]
```

`CommonFilters`: `--include-types`, `--exclude-types` (rule 14: exclusion
wins), `--include-people`, `--date-range`, `--reference-year`,
`--living-only`, `--include-private`, `--leap-day`.
`--format all` expands to exactly csv, json, parquet, pdf; no `--format` is a
clap error. `report` writes `{prefix}.{ext}` into `--out-dir` (created if
absent), atomically. When any event was skipped for a malformed date it also
writes `{prefix}.errors.json` (D-a); `inspect`/`list`/`report` print one line
per issue to stderr with ASCII control characters stripped, so a hostile
file cannot inject terminal escape sequences — and the run still exits
successfully because the selected output is produced (§8).

### Web (`gramps-events serve`)

Binds **127.0.0.1** only (single-user, family PII — no route listens on an
external interface without an explicit opt-in flag). Routes:

| Route | Purpose |
| --- | --- |
| `GET /` | Landing page (file picker / resume) |
| `POST /api/load` | Multipart upload, **200 MB cap** checked before parsing; stored under a **generated** temp name (the browser-supplied filename is never used as a path); parse → state |
| `GET /api/options` / `PUT /api/options` | Current / updated report options |
| `GET /api/events?view=list\|calendar\|yrcal\|timeline` | Rendered view as an HTML fragment (htmx swap target) |
| `GET /api/events.json` | The same data as `EventRow` JSON |
| `GET /api/export?format=csv,parquet,pdf` | Streamed downloads with `Content-Disposition`; the payload is built by `event-core` + `writers` |
| `POST /api/reset` | Unload the current file; uploads are deleted on reset and at process exit |

State is `Arc<AppState>` holding the loaded `Database`, current
`ReportOptions` and the temp-dir path. The four tabs (list, anniversary
calendar, calendar-with-years, timeline) render the shared view builders'
data; event-type checkboxes with counts, orphan/privacy/leap-day toggles and
the export button group all drive the same options object.

---

## 8. Error handling and hardening

- **Container decode errors are diagnosable**: `EmptyInput`,
  `UnknownContainer { magic }` (a self-quoting, single-line byte `preview` of
  the input head, hex-escaped past 16 bytes), `GzipDecode`, `ZipDecode`,
  `ZipMissingMember { found }` (names the actual members), `InvalidUtf8`.
- **XML structure errors are hard**: missing/duplicate handles abort with
  the record type/position in the message. Reversed
  `daterange`/`datespan` endpoints are **not** hard errors: the date model
  still rejects them (`DateError::RangeStartAfterStop`), but the XML layer
  demotes the failure to a warn-and-skip issue (`GrampsXmlError::InvalidDate`
  was removed with this change).
- **Soft defects warn-and-skip**: `Database::warnings` lists each
  occurrence. Malformed date elements extend this to a structured
  `Database.date_issues: Vec<DateIssue>` (event handle/id/type, the failing
  date element tag and the `DateError` message, in document order): the
  event is omitted from outputs, the message is printed to stderr (control
  bytes stripped), `report` writes `{prefix}.errors.json` when file outputs
  are requested, and the web load summary carries the same records as
  `date_errors` and renders them in an error section (D-h). `warnings` stays
  the human log; `date_issues` is the structured subset downstream
  consumers read. Empty input and garbage files each raise the named error;
  a well-formed `<database>` with an empty body parses fine (the UI renders
  "0 events").
- **Writers are atomic** (temp file + rename) so an aborted export never
  leaves a partial file at the final path.
- **Performance proportionality**: `benchgen` + `bench_large.rs` assert the
  100k-event `report` (csv+json) inside 30 s wall and a 1.2 GiB peak-RSS
  delta (debug profile, Linux `/proc/self/status` `VmHWM`); the memory half
  is skipped on non-Linux.

---

## 9. Testing strategy

- **Unit + property tests per crate** (gramps-dates 54, gramps-xml 68,
  event-core 64, writers 49 + roundtrip, cli 39, web 17 + 36 API
  integration, benchgen 3) — golden parse tables, anchor rules, role
  precedence, dedup, couple-row collapse, calendar ordering, filter
  combinations, parquet round-trip, PDF magic/page-count.
- **Committed fixtures** in `tests/fixtures/` (`data.gramps` example copy,
  `dates`, `edge-cases`, `families`, plus the hardening empty/error sweep) —
  see `tests/fixtures/README.md` for the table and DTD attribution.
- **Snapshot tests** for `inspect`/`list`/`report` output on the fixtures,
  including `--view yrcal`.
- **Web integration tests** drive the router with Tower `oneshot`; an
  escaping regression test asserts hostile names/dates render escaped.
- **Benchmark** (`bench_large.rs`): time/memory budget over the generated
  100k-event fixture.
- **Cross-check**: the plan calls for diffing birth/death/marriage entries
  against Gramps' own Birthday & Anniversary report on the fixture set.

### 9.1 §11.1 acceptance checklist (status: passing)

| §1 requirement | Verifiable acceptance | Covering test / evidence |
| --- | --- | --- |
| 1. Read any `.gramps` file | All three containers parse; corrupt containers raise a decode error naming the file/position | `all_three_containers_produce_identical_databases`; `truncated_gzip_raises_a_decode_error`; `truncated_zip_raises_a_decode_error`; `zip_without_data_gramps_member_is_rejected` + the `committed_*` fixture sweep (incl. `EmptyInput` / `UnknownContainer` with the self-quoting `preview`) |
| 2. All event types, incl. custom | `inspect` enumerates a user-defined type; `list`/`report` emit rows; uniform anniversary treatment | `inspect_lists_total_and_at_least_one_type` (cli); benchgen embeds a `Custom` type and asserts it parses (`small_fixture_has_no_warnings_and_some_private_events`); the golden date/view tests treat every type uniformly |
| 3. Event-type selection | include/exclude tests incl. rule 14 precedence; GUI checkbox list == `inspect` | `type_filters_implement_whitelist_exclusion_and_precedence` (rule 14: exclusion wins); the web UI builds checkboxes from the same count enumeration |
| 4. Any combination of the four formats | One run with all four writes four files with correct magic headers; each non-empty subset tested; `--format all` == exactly four | `report_all_writes_four_files_with_magic_headers`; `report_subset_formats_write_only_the_requested_files`; `report_all.snap` snapshot; `format_all_expands_to_exactly_the_four` (cli args) |
| 5. CLI first, GUI second | One `report` command with no server/Node produces all outputs; `serve` starts standalone | `end_to_end_report_all_via_dispatch` (main.rs); `serve` documented + web API integration tests (`crates/web/tests/api.rs`) |
| 6. Four display modes | Each view builder has golden fixtures; the GUI renders the same builders' data in four tabs | snapshot tests `list_data_{list,calendar,timeline,yrcal}.snap`; view-builder unit tests; web UI four-tab layout |
| 7. Date-range handling | Rules 2/3/9/11/13 golden-tested: anchor-at-start, no-month exclusion, range bars, full-extent spans, undated handling | `compound_stop_before_start_is_a_hard_error`; anchor-rule tests (`*must anchor` / `*must not anchor` in views); `event_date_stop`/`date_is_range` flow through rows and the timeline/yrcal builders; undated-group tests |
| 8. All Rust | Build/lint/CI are Rust-only; htmx is the only vendored non-Rust artifact; a single Rust binary ships | Workspace is pure Rust (`cargo build/clippy/test`); one `[[bin]] gramps-events`; htmx served as a vendored static file only |
| — living-only | Cross-checked against Gramps `probably_alive` | `probably_alive_matches_the_gramps_port`; `living_only_keeps_events_with_no_dead_subject` |
| — privacy | Default runs exclude private records; `--include-private` shows them flagged | `defaults_hide_private_but_keep_everything_else`; `include_private_keeps_private_events`; `event_cause_description_and_private_parse` |
| — Feb 29 fold | `leap_day_folded` column + label in every output where the fold applies | `feb_29_anchor_folds_in_a_non_leap_reference_year`; `feb_29_folds_only_in_non_leap_years_under_fold_policy`; `leap_day_folded` carried on every `EventRow` and rendered as a label in views |

---

## 10. Limitations and future work

- **Non-Gregorian calendars**: only Gregorian + Julian convert (D4); the
  other five degrade to text display. SDN converters for all seven are the
  planned later release.
- **Name formats**: primary-name rendering is `First Surname` (with surname
  prefix); Gramps' name-format options and married-woman surnames are
  documented v1.1 gaps (§8.8 of the plan).
- **Read-only**: no editing/round-tripping back to Gramps; the native
  (BerkeleyDB/BSDD) database format is not read — only XML exports.
- **GEDCOM import**: not in v1; `event-core` is the natural adapter point.
- **Web**: single-user loopback only; no TLS, auth, or multi-user support by
  design.

---

## 11. Security and operational notes

- The web server binds 127.0.0.1 by default; uploads are size-capped
  (200 MB) before reading into memory, stored under generated temp names in
  a temp dir, and deleted on reset/exit. HTML output is auto-escaped by
  Askama (locked by the escaping regression test).
- No secrets are handled by this tool. Embedded non-code data is the askama
  templates, the vendored static assets, and the bundled typst fonts — the
  font data (~9 MB, binary) is pulled in by the `fonts` feature on
  `typst-assets` (see the writers crate in §1 and the [fix plan](research/fix-blank-pdf-export.md)).
- Writers use atomic rename so concurrent or aborted exports do not corrupt
  final files; outputs land only in `--out-dir` on the machine running the
  command.