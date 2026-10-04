# Plan — Malformed-date tolerance, couple display, and calendar ordering

**Status:** Research / proposal — not yet implemented
**Date:** 2026-10-04
**Scope:** Four requested changes to `gramps-events`:

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

The original design lives in
[`gramps-events-architecture.md`](gramps-events-architecture.md) (the plan). This
document records the deltas and the implementation steps; it supersedes the
relevant parts of the original plan (§7.1 hard reversed ranges, §8.6 one-row-
per-subject for couples, rule 12 calendar order) and of
[`../ARCHITECTURE.md`](../ARCHITECTURE.md) where noted.

---

## 1. Confirmed decisions (2026-10-04)

| # | Decision | Choice |
| --- | --- | --- |
| D-a | Error-report file name / when | `{prefix}.errors.json` in `--out-dir`, written **only when** at least one event was skipped for a malformed date |
| D-b | Which events are reported | **All** events skipped for a bad date (reversed range/span **and** invalid value/month/day/modifier/quality/calendar/etc.), each with its message |
| D-c | Couple line format | Both names joined with the Unicode marriage symbol, `Adam Uplands ⚭ Eve Uplands` |
| D-d | Which events collapse | **All family/couple events** (any event resolved from a `<family>` couple), not only `Marriage` |
| D-e | Second person field | Add a **new column/field** `person_id_2` holding the second spouse's Gramps id; `null` for single-person events |
| D-f | Calendar scope | **Both** calendars are in scope; note that calendar-with-years already orders month-only before day-1, so it only needs regression tests |
| D-g | Anniversary calendar ordering | Category first, then year: `(specificity, year, tiebreakers)` within each `(month, day)` cell |
| D-h | Web error report | **No** `errors.json` download in the web UI: the load summary/JSON carries `date_errors` and `main.html` shows them; the file report stays CLI-only |

---

## 2. Current behaviour and root causes

### 2.1 Malformed dates abort the whole parse (requirement 1)

`crates/gramps-xml/src/parse.rs::parse_event` (around lines 565–578) treats a
reversed `daterange`/`datespan` as a hard failure:

```rust
Err(source) => {
    if matches!(source, DateError::RangeStartAfterStop(..)) {
        return Err(GrampsXmlError::InvalidDate { id, source });   // aborts parse
    }
    warnings.push(format!("skipping event {id}: malformed … date ({source})"));
    return Ok(None);
}
```

`GrampsXmlError::InvalidDate` (`crates/gramps-xml/src/error.rs`, lines 79–84)
propagates out of `parse_database`, and `main.rs` prints `error: …` and returns
`ExitCode::FAILURE` — so no CSV/JSON/Parquet/PDF is produced. Every *other*
date defect already warns and skips; only reversed ranges abort.

`Database.warnings` (`crates/gramps-xml/src/model.rs`, line 36) is a
`Vec<String>`; it is **never** read by `cli`, `event-core`, or `web` today
(only tests and `benchgen`), despite the architecture claiming it is surfaced.
So there is no structured vehicle for an error report yet.

### 2.2 Couples expand to one row per spouse (requirement 2)

Subject resolution (`crates/event-core/src/resolve.rs::resolve_subjects`,
lines 266–296) correctly resolves a family-owned event to both father and
mother **as two `PersonDisplay` subjects**. Row expansion
(`crates/event-core/src/views.rs::expand_rows`, lines 306–326) then emits one
`EventRow` per subject, which is why a marriage shows as two separate lines. The
stated plan contract was explicitly "one row per (event, subject)"
(`docs/ARCHITECTURE.md` §5/§6; `crates/event-core/src/row.rs` module docs).

`EventRow` (`crates/event-core/src/row.rs`) has a single `person_id` and a
single `person_name` — there is no field for a second person.

### 2.3 Calendar ordering (requirements 3 & 4)

`crates/event-core/src/views.rs::calendar_sort_key` (lines 464–472) sorts the
anniversary calendar by `(month, day, event_type, person_name, event_id)`. It
never considers the year, and it cannot distinguish a month-only date from a
day-1 date because both anchor to day 1 (`anniversary_key`, rule D10) — the raw
`row.day` is `None` for the first and `Some(1)` for the second.

The PDF backend (`crates/writers/src/pdf.rs::build_pdf_document`, lines
121–170) does not consume the calendar view's order: `report()` builds the PDF
from the **list** view and the PDF re-sorts cells with `rule_12_calendar_key`
(type, name, id) — so the PDF would diverge from the UI unless it is updated
too.

The calendar-with-years (`build_calendar_with_years`) sorts by `list_sort_key`,
whose date tuple uses `day.unwrap_or(0)` — a month-only day is `0`, so it
already sorts before day `1` inside the shared day-1 cell. That satisfies
requirement 4 today; requirement 3 is moot there because years are separate
cells. Both need locking tests (decision D-f).

---

## 3. Design

### 3.1 Structured malformed-date issues (requirement 1)

Add a structured record to the ingestion crate (no serde dependency needed
there; the output crates serialize it):

```rust
// crates/gramps-xml/src/model.rs
/// One event omitted because a date element failed to parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateIssue {
    /// The `<event>` handle.
    pub event_handle: String,
    /// The Gramps id (`id="E0000"`); falls back to the handle.
    pub event_id: String,
    /// The `<type>` text, when it had been read before the event was skipped.
    pub event_type: String,
    /// The date element tag: `dateval` | `daterange` | `datespan` | `datestr`.
    pub date_kind: String,
    /// The `DateError` display string, e.g.
    /// `daterange/datespan stop "1914" sorts before start "1918"`.
    pub message: String,
}
```

- `Database` gains `pub date_issues: Vec<DateIssue>`.
- `parse_event` no longer returns `GrampsXmlError::InvalidDate`. **Every**
  `Err(source)` from a date element — `RangeStartAfterStop` and every already-
  soft error (invalid value / month / day / modifier / quality / calendar /
  `datestr`) — sets a local pending issue and `continue`s the child loop
  instead of returning early, so `<type>` is captured even when it follows the
  broken date element. After the loop, if a date issue was recorded the
  function pushes the human message to `warnings` **and** the structured record
  to `date_issues`, then returns `Ok(None)` (the handle stays unregistered,
  exactly like today's soft skips). If the event also has a malformed `change`
  attribute, the date issue takes precedence: the event is skipped for the date
  with no second warning.
- `GrampsXmlError::InvalidDate` becomes unreachable and is removed (it is not
  part of any serialized contract; this is a v0.1 breaking enum change and is
  documented in `../ARCHITECTURE.md`).

`warnings` stays the human log (terminal text, existing tests), `date_issues`
is the structured subset the error report and web UI consume.

### 3.2 CLI: still produce output, print, and write the report

- `main.rs::dispatch` prints every `db.date_issues` entry to **stderr** after
  loading, for `inspect`, `list` and `report` (a shared helper, e.g.
  `run::print_date_issues(&db)`). Because the message embeds raw file text, the
  helper strips ASCII control characters (including `ESC`) before writing, so a
  hostile `.gramps` file cannot inject terminal escape sequences. The run still
  exits `SUCCESS` because the selected output is produced.
- `run::report` (`crates/cli/src/run.rs`) writes `{prefix}.errors.json` into
  `--out-dir` when `!db.date_issues.is_empty()`, after the requested format
  files, using the same atomic write path. The returned `Vec<PathBuf>` appends
  the error-report path **last** (after csv/json/parquet/pdf), so the CLI echoes
  it like the others in a deterministic position.

Report shape (deterministic; no timestamps, document order preserved):

```json
{
  "error_count": 2,
  "errors": [
    {
      "event_id": "E0005",
      "event_handle": "_abc",
      "event_type": "Death",
      "date_kind": "daterange",
      "message": "daterange/datespan stop \"1914\" sorts before start \"1918\""
    }
  ]
}
```

The serializer lives in `writers` (which already owns JSON) as a new
`writers::write_date_issues(issues: &[event_core::DateIssue], dest) ->
Result<(), WriterError>`, reusing `write_atomically`. `DateIssue` is defined in
`gramps-xml` but **re-exported by `event-core`** (`pub use
gramps_xml::DateIssue;`), so `writers` keeps its single `event-core` dependency
and the documented layering ("`writers`/`web` consume only `event-core` types
and pre-built views/documents", `../ARCHITECTURE.md` §1) holds — **no new
`writers -> gramps-xml` edge is added**.

### 3.3 Web: display the issues

- `LoadedSummary` (`crates/web/src/state.rs`) gains
  `date_errors: Vec<DateIssueDto>` where `DateIssueDto` mirrors the JSON report
  fields and derives `Serialize`. `LoadedDatabase::summary()` maps
  `db.date_issues`. (The ingestion field is `date_issues`; the web DTO name is
  `date_errors` — an intentional presentation rename, stated so it is not read
  as a mismatch.)
- `crates/web/templates/main.html` renders a `<section class="date-errors">`
  after the summary when `date_errors` is non-empty: a short heading ("N events
  were skipped because their dates are malformed"), then a table of
  `event id | type | message`. Askama escapes every field.
- The `POST /api/load` JSON summary automatically carries the same data. The
  screen-output requirement is satisfied here; per-format web exports stay
  single-file (a web `errors.json` download is a possible follow-up, not in
  scope).

### 3.4 Collapse couple events to one row (requirement 2)

**Mark couple events during resolution.** Add `pub couple: bool` to
`ResolvedEvent` (`crates/event-core/src/model.rs`) and to the internal
`Resolution` in `resolve.rs`; set it in the family branch (c)/(d). Filters
(`passes`) keep operating on the full `subjects` list, so:
- `--include-people` still matches **either** spouse (it runs before row
  expansion);
- `living_only` / `probably_alive` still sees both spouses;
- orphan / single-spouse behavior is unchanged.

**Collapse in `expand_rows`.** When `event.couple` is true, emit exactly one
`EventRow` from the event's subjects instead of one per subject:
- `person_name` = the non-empty subject names joined with `" ⚭ "`
  (D-c). One spouse → that name; both → `A ⚭ B`.
- `person_id` = first spouse's Gramps id; **`person_id_2`** = second spouse's id
  (D-e). `null` when there is no second spouse.
- `role`, `age_at_event`, `elapsed_years`, dates, etc. come from the existing
  event-level values (`age_at_event` remains the first/primary subject's age; no
  change requested).
- `dedupe_same` uses a combined handle key (`"handleA|handleB"`) for collapsed
  rows so the collapse still applies.

Non-couple events keep the current one-row-per-subject expansion, including
person-referenced events with two subjects (only family/couple events collapse,
per D-d). Family events with a single resolving spouse produce one row with
`person_id_2 = null`.

**Schema change.** `EventRow` gains `person_id_2: Option<String>` (placed
immediately after `person_id`). This is a **breaking flat-contract change**:
- CSV header gains `person_id_2` (column position changes);
- JSON gains `"person_id_2": null | "I0002"`;
- Parquet schema gains the column (nullable Utf8).

Per `../ARCHITECTURE.md` §6, evolution must be versioned. Add an
Arrow/Parquet file-level `schema_version` key-value metadata (`2`) on write and
update the golden schema tests. (No `schema_version` column — metadata only, so
the row shape stays flat.) CSV and JSON are field-name self-describing, so they
get **no** version marker; update ARCHITECTURE §6 to state explicitly that the
Parquet file metadata is the versioned surface while CSV/JSON rely on their own
field names.

### 3.5 PDF glyph for `⚭`

Verified against the bundled fonts in `typst-assets 0.14.2`:
- `LibertinusSerif-Regular.otf` (the markup's font) does **not** cover U+26AD;
- `NewCM10-Regular.otf` **does** cover U+26AD;
- `DejaVuSansMono.ttf` does not.

Relying on typst's implicit fallback is fragile, so set an explicit fallback in
`build_markup`:

```typst
#set text(size: 10pt, font: ("Libertinus Serif", "NewCM10"))
```

Latin text still shapes with Libertinus; only the marriage symbol falls through
to NewCM10. Add a test that the compiled PDF contains no missing-glyph warning
and that headless text/layout still yields the expected page count (the
`RenderedPdf.warnings` seam already exists). The deterministic probe is fixed:
assert `RenderedPdf.warnings` contains no diagnostic naming a missing glyph /
`.notdef`; if the pinned typst 0.14.2 emits no such warning, fall back to a
byte-level probe that renders the one-glyph document with and without the
explicit fallback and asserts the with-fallback content stream is non-empty
(page count unchanged). No other probe is in scope.

### 3.6 Anniversary-calendar ordering (requirements 3 & 4)

Change the sort applied inside each `(month, day)` cell from
`(type, subject, id)` to:

```text
(specificity, year, type, person_name, event_id, person_id)
```

where `specificity = 0` when `row.day.is_none()` (month-only, day `00`) and `1`
when the day is known. `(month, day)` remain the primary grouping key, so the
day-cell grouping loop is untouched. Result: all month-only entries of a day-1
cell come first (earliest year first), then all day-1 entries (earliest year
first).

Expose the order once from `event-core` as a public comparator (e.g.
`pub fn compare_calendar_rows(a: &EventRow, b: &EventRow) -> Ordering`) and use
it from `build_calendar`. The comparator starts with `(month, anniversary_day,
specificity, year, …)` so it is also usable by the PDF.

**PDF alignment.** `build_pdf_document` gathers anchored rows and sorts cells
with `rule_12_calendar_key`; replace that with `event_core::compare_calendar_rows`
so the PDF matches the UI/CLI calendar exactly (the PDF is built from the list
view, so it must sort itself).

**Calendar-with-years.** No behavioural change: `list_sort_key` already orders a
month-only row (`day = 0`) before a day-1 row in the same year/month cell and
years are separate cells. Add regression tests that lock month-only-before-day-1
within a cell, satisfying D-f.

---

## 4. Step-by-step implementation plan

Each step is one commit-sized logical unit; run `cargo fmt`, `cargo clippy
--workspace --all-targets -- -D warnings`, and `cargo test --workspace` after
each.

| # | Commit message | Logical unit | Key deliverables | Tests |
| --- | --- | --- | --- | --- |
| 1 | `feat(gramps-xml): record malformed-date issues instead of aborting` | Ingestion tolerance | `DateIssue` + `Database.date_issues`; `parse_event` defers **all** date-element errors, captures type, warns/skips; remove `GrampsXmlError::InvalidDate`; re-export `DateIssue`; update the `parse.rs`/`error.rs` module docs that still call reversed ranges a hard error | Rewrite `reversed_range_*` to `reversed_range_warns_and_skips_and_is_reported`; keep `malformed_date_warns_*`; add a fixture with both a reversed range and an invalid value; assert `event_type` is captured when `<type>` follows the broken date |
| 2 | `feat(writers): serialize malformed-date error reports` | JSON report writer | `writers/src/error_report.rs`, `write_date_issues` (atomic, deterministic) taking `&[event_core::DateIssue]`; `event-core` re-exports `DateIssue` so no new crate edge | Unit: array shape, `null`-free fields, empty-input guard, atomic rename, and a message containing quotes/backslashes/newlines round-trips |
| 3 | `feat(cli): emit {prefix}.errors.json and print date issues` | CLI output | `run::report` writes the report when needed and returns its path; `run::print_date_issues`; `main.rs` calls it for inspect/list/report | `report_writes_errors_json_only_when_needed`; `report_all_*` path-list updates; `print` helper unit test |
| 4 | `feat(web): surface malformed-date issues in the UI` | Web display | `DateIssueDto` + `LoadedSummary.date_errors`; `main.html` errors section + stylesheet rule; `summary()` mapping | API: load JSON carries `date_errors`; main page HTML contains the event id, type and message escaped; clean file renders no section |
| 5 | `refactor(event-core): mark family-couple events in resolution` | Resolution | `ResolvedEvent.couple` + `Resolution.couple`, set in branch (c)/(d); docs | `family_fixture_applies_role_precedence_and_couples` asserts `couple`; person-referenced events assert `!couple` |
| 6 | `feat(event-core): collapse couple events into one row with person_id_2` | Couple row + flat contract (whole workspace) | `EventRow.person_id_2`; `expand_rows` couple branch + `" ⚭ "` join; combined-handle dedupe; **update every `EventRow` struct literal / golden test in the workspace** (event-core, writers `csv.rs`/`parquet.rs`/`roundtrip.rs`, web tests); **Parquet `schema()` + `StructArray` column added here** so Parquet keeps compiling and round-tripping. CSV/JSON update automatically via serde — only the writers' test `HEADER` constant changes | `couple_event_collapses_to_one_row_with_both_ids`; single-spouse family keeps one row and `null` second id; `dedupe_same` on couples; row-shape golden; CSV header/value; Parquet field-list + round-trip preserves `person_id_2` |
| 7 | `feat(writers): stamp parquet schema_version metadata` | Output versioning | Build `WriterProperties` with `set_key_value_metadata` and call `ArrowWriter::try_new_with_options` (replacing today's `try_new(..., None)`); `schema_version = 2` file metadata | Parquet metadata read-back is `2`; no production CSV/JSON change |
| 8 | `fix(writers): add NewCM10 fallback so the marriage symbol renders` | PDF glyph | `build_markup` font list; test that `⚭` entries compile without a missing-glyph warning | `marriage_symbol_renders_with_newcm_fallback` (PDF magic + page count + warning probe) |
| 9 | `feat(event-core): order the anniversary calendar by specificity then year` | Calendar order | Public `compare_calendar_rows`; `build_calendar` uses it; doc comments | `calendar_orders_month_only_before_day_one_then_by_year`; update `golden_dates_calendar_*` and `calendar_view_is_a_month_day_tree` expectations; regenerate `list_data_calendar.snap` |
| 10 | `fix(writers): align the PDF calendar with the new row order` | PDF order | `build_pdf_document` uses the shared comparator | PDF month/day entry-order test with a month-only + day-1 collision |
| 11 | `test(event-core): lock calendar-with-years month-only-first ordering` | Regression only | No production change; explicit cell-order assertions | `calendar_with_years_orders_month_only_before_day_one` |
| 12 | `docs: document date tolerance, couple rows and calendar order` | Cross-cutting docs only | `../ARCHITECTURE.md` §1/§4/§5/§6/§8 (including the `DateIssue` re-export layering note and the CSV/JSON-vs-Parquet versioning wording), `README.md` report/error-report + couple behaviour, fixture README | — |

### Suggested file touch list

- `crates/gramps-xml/src/{model.rs,parse.rs,error.rs,lib.rs}`
- `crates/event-core/src/{model.rs,resolve.rs,views.rs,row.rs,lib.rs}`
- `crates/writers/src/{error_report.rs (new),lib.rs,csv.rs,parquet.rs,pdf.rs}`
- `crates/cli/src/{run.rs,main.rs}`
- `crates/web/src/{state.rs,handlers.rs,templates.rs}`, `crates/web/templates/main.html`,
  `crates/web/static/style.css`
- `crates/cli/tests/snapshot.rs` + `crates/cli/tests/snapshots/list_data_calendar.snap`
- `crates/web/tests/api.rs`, `crates/cli/tests/*`, `crates/event-core/src/views.rs` tests
- `docs/ARCHITECTURE.md`, `README.md`, `tests/fixtures/README.md`
- `tests/fixtures/malformed-dates.gramps` (new)

---

## 5. Test plan (detail)

#### Malformed dates
- Reversed `daterange` and `datespan` (start after stop) each warn, are absent
  from `db.events`, appear in `date_issues`, and do not fail `parse_database`.
- Invalid value / month / day / modifier / quality / calendar also appear in
  `date_issues` (D-b).
- `<type>` that follows a broken date element is still captured in
  `DateIssue.event_type` (the reason for the deferred-skip design).
- A hostile message containing quotes, backslashes, newlines and control
  characters serializes to valid JSON and, when printed, has control bytes
  stripped.
- `report --format all` on a damaged fixture writes csv/json/parquet/pdf **and**
  `{prefix}.errors.json`; exit status is success; stderr mentions the event.
- A clean fixture writes no `errors.json`.
- Web `POST /api/load` JSON includes `date_errors`; the rendered `main.html`
  shows them HTML-escaped; a clean file shows no error section.

#### Couples
- family fixture: the two marriage rows collapse to one row
  `person_name == "Adam Uplands ⚭ Eve Uplands"`, `person_id == "I00.."`,
  `person_id_2 == "I.."`.
- Divorce / other family events also collapse (D-d).
- Single-spouse family event: one row, `person_id_2 == null`.
- Person-referenced two-subject event: unchanged two rows (not a couple).
- A couple event resolved from more than two subjects still emits one row and
  documents the extra ids (only the first two reach `person_id`/`person_id_2`).
- `--include-people` selects the event from **either** spouse's id (pre-existing
  filter semantics preserved).
- CSV/JSON/Parquet golden shape tests include `person_id_2`; Parquet round-trip
  preserves it and the `schema_version` metadata is `2`.

#### Calendar ordering
- In one `(month, day=1)` cell: `1980-02-00` before `1980-02-01`; among
  month-only entries years ascend; among day-1 entries years ascend.
- Regression: month-only is not merged with a *different* day cell.
- PDF and CLI calendar order agree.
- Property test: `compare_calendar_rows` is a total order (antisymmetric and
  transitive), and `build_calendar` output agrees with sorting its rows by it
  under shuffled input (determinism).
- calendar-with-years: month-only before day-1 in the same year/month/day cell.
- Regenerate snapshots with `UPDATE_SNAPSHOTS=1 cargo test -p cli --test snapshot`
  only after reviewing the diff.

#### PDF glyph
- A document containing `⚭` compiles, keeps its page count, and produces no
  missing-glyph diagnostic on `RenderedPdf.warnings`.

---

## 6. Risks, edge cases, and notes

- **Parquet schema break (D-e).** Adding `person_id_2` invalidates existing
  Parquet readers that assert the old column set. Mitigation: file-level
  `schema_version = 2` metadata + documentation; the project is v0.1 and the
  contract was already declared "adding a field is breaking".
- **More than two couple subjects.** Rare (an event referenced by several
  families). The collapse joins all names with `" ⚭ "`, but `person_id_2` can
  only hold one id — extra ids are only present in the combined name. Documented
  limitation; `person_id`/`person_id_2` cover the common two-spouse case.
- **`age_at_event` for a couple row** stays the first spouse's age (pre-existing
  behaviour for the first-per-spouse row); the requirement did not ask to change
  it.
- **Type capture on skip.** Because every date error is deferred to after the
  child loop, `<type>` is captured whether it precedes or follows the broken
  date element; only a file that omits `<type>` entirely yields an empty
  `event_type` in the report (the `event_id` is the reliable key).
- **`InvalidDate` removal** is a public enum change; update the doc comment in
  `../ARCHITECTURE.md` §1/§8 that calls reversed ranges a hard error.
- **`⚭` in non-PDF outputs** is plain UTF-8; terminals, HTML, CSV/JSON are fine.
  Only PDF needed the font fallback.
- **Untrusted text sinks.** `DateIssue.message` and `event_type` come from the
  file. Stderr printing strips ASCII control characters; the web route is
  Askama-escaped and JSON is serde-escaped. CSV formula injection (fields
  beginning `= + - @`) is a pre-existing, CSV-wide concern; this plan does not
  change the CSV encoding but records the risk here.
- **Issue precedence.** One event with both a broken date and a bad `change`
  attribute records the date issue once and skips; no second warning.
- **Determinism.** The error report contains no clock values; issue order is
  document order; the report writer sorts nothing else.

---

## 7. Deviations from the original plan / ARCHITECTURE.md

| Original text | New behaviour |
| --- | --- |
| plan §7.1 / ARCHITECTURE §3/§8: reversed `daterange`/`datespan` is a hard error; `GrampsXmlError::InvalidDate` | warns and skips; structured `DateIssue`; `InvalidDate` removed |
| plan §8.6 / ARCHITECTURE §5: "one row per (event, subject)"; couples expand per spouse | family/couple events collapse to one row; `EventRow.person_id_2` added |
| plan rule 12 calendar order `(type, subject, id)` | `(specificity, year, type, subject, id)` within a `(month, day)` cell |
| ARCHITECTURE §6: `EventRow` v1 flat contract | v2, with `person_id_2` + Parquet `schema_version` metadata |
| ARCHITECTURE §8: "Soft defects warn-and-skip: `Database::warnings` … the CLI and web surface it" | now actually true for malformed-date issues (structured + terminal + web), via `date_issues` |

---

## 8. Assumptions to confirm during review

1. `person_id_2` is placed immediately after `person_id` in the flat contract
   (readability) rather than appended at the end; both are breaking either way.
2. ~~The error report is only produced by the CLI `report` command~~ —
   **confirmed as D-h**: the web route shows the issues and carries them in the
   load summary/JSON, but adds no `errors.json` download.
3. `age_at_event` on a collapsed couple row remains the first spouse's age.
4. `⚭` (U+26AD) is the intended separator with spaces on both sides exactly as
   written: `" ⚭ "`.
