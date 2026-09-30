# gramps-events

Export and view **every event** of a Gramps family tree — birthdays, deaths,
weddings, and any custom event type — as CSV, JSON, Parquet and PDF, or as
one of four views: a list, an anniversary calendar, a calendar-with-years
grid, or a timeline.

`gramps-events` is a single Rust binary that ships the CLI **and** a local
web UI. It reads the XML export format (`.gramps` files — plain XML, gzipped,
or zipped "saved trees"), treats every event type uniformly, and is built to
replace the workflow of Gramps' built-in *Birthday and Anniversary Report*
with more formats and full date-range support.

```text
$ gramps-events inspect family.gramps
== Event Types ==
Total events: 6
  Birth            5
  Death            1

$ gramps-events list family.gramps --view calendar
== Anniversary Calendar ==
January:
  22: Mark Hairball — Birth (1946-01-22) · Furmany · 80 years
...
```

## Requirements

- Rust 1.91 or newer (`rust-version` is pinned in the workspace
  `Cargo.toml`). No Node, no system services — pure Rust.

## Build

```bash
cargo build --release
# binary at target/release/gramps-events
```

Run the test suite and lints:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

## Usage

```text
gramps-events inspect <file.gramps>
gramps-events list    <file.gramps> [--view list|calendar|timeline|yrcal] [filters…]
gramps-events report  <file.gramps> --format csv,json,parquet,pdf|all [filters…]
                      [--out-dir DIR] [--out-prefix NAME]
gramps-events serve   [--port 8380]
```

### `inspect`

Print every event type present in the file with its count — the same
enumeration the web UI builds its event-type checkboxes from:

```bash
gramps-events inspect family.gramps
```

### `list`

Render one of the four views as text (`--view list` is the default):

| `--view` | Shows |
| --- | --- |
| `list` | Flat table: date, type, person(s), place, age/elapsed |
| `calendar` | Anniversary calendar by month/day — no years, elapsed years per event |
| `timeline` | Chronological, grouped by year; ranges render as bars |
| `yrcal` | Calendar-with-years: a year-by-year month grid showing ranges in full |

### `report`

One command, any combination of the four formats — each writes
`{out-prefix}.{ext}` (default prefix: the input file's stem) into
`--out-dir` (created if absent):

```bash
gramps-events report family.gramps --format all --out-dir out --reference-year 2026
gramps-events report family.gramps --format csv,json --exclude-types Death
```

`--format all` expands to exactly `csv`, `json`, `parquet`, `pdf`; giving no
`--format` is an error. Writes are atomic (temp file + rename), so an
aborted run never leaves a partial file behind.

### Filters (shared by `list` and `report`)

| Flag | Meaning |
| --- | --- |
| `--include-types Birth,Death` | Only these event types (comma-separated whitelist) |
| `--exclude-types …` | Drop these types; **exclusion wins** when a type is in both sets |
| `--reference-year YYYY` | The report year every elapsed value is measured against (default: current year) |
| `--living-only` | Keep only events whose subjects are (probably) alive — a port of Gramps' `probably_alive` |
| `--include-private` | Include `priv="1"` records (default: excluded, flagged when included) |
| `--date-range 1900-01-01..2026-12-31` | Inclusive window on the normalized event date |

(Feb 29 handling is fixed per decision D6: dates anchor on Feb 29 are folded to
Feb 28 in non-leap reference years and labeled as such — `leap_day_folded` in
the output — so every anniversary appears every year.)

### `serve` — the web UI

```bash
gramps-events serve --port 8380   # then open http://127.0.0.1:8380
```

The server binds **127.0.0.1 only** — this is a local, single-user tool for
family data. Pick a `.gramps` file in the browser (uploaded with a 200 MB
cap), tick event types, toggle orphans/private/leap-day, pick a reference
year, and either browse the four tabs or export.

## Date handling (the short version)

Gramps dates are richer than ISO strings — modifiers (before/after/about),
ranges and spans, partial dates (year-only, month-only), BC, dual dates, and
seven calendars. `gramps-events` normalizes Gregorian and Julian dates
exactly (SDN conversion, the same algorithm Gramps uses); the other five
calendars degrade to their stored text with a warning. The anniversary
semantics (which dates anchor in the calendar view, how ranges behave,
Feb 29 folding, elapsed-years rules) are specified in
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and the research plan
[docs/research/gramps-events-architecture.md](docs/research/gramps-events-architecture.md).

## Output schema

All three row formats (CSV / JSON / Parquet) share one flat contract —
`EventRow` — so a file is self-describing and stable across formats:

```text
person_id, person_name, event_id, event_type, event_date, event_date_text,
event_date_stop, date_is_range, year, month, day, anniversary_month,
anniversary_day, leap_day_folded, place, role, age_at_event, reference_year,
elapsed_years, private
```

`event_date` is the normalized (Gregorian) ISO date or range start;
`event_date_stop` carries range/span endpoints; `event_date_text` is the
Gramps display string ("about 1900", "Nov 1822 – Apr 1823"); `reference_year`
is constant per run so each file states the elapsed baseline. JSON emits
`Option`s as `null`; Parquet has a fixed schema — adding or reordering fields
later is a breaking change for Parquet readers.

## Project layout

```text
crates/
  gramps-dates/   date model, parsing, Gregorian/Julian normalization
  gramps-xml/     .gramps containers + XML → typed model
  event-core/     resolution, filters, views, output contracts
  writers/        csv / json / parquet / pdf backends
  cli/            the gramps-events binary (inspect/list/report/serve)
  web/            the local web UI (axum + askama + htmx)
  benchgen/       deterministic generator for the 100k-event benchmark
tests/fixtures/   committed test data (see tests/fixtures/README.md)
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full architecture:
module map, pipeline, design decisions D1–D12, view semantics, hardening
(error contract, warnings, benchmark budgets), and limitations.

## Development notes

- **Fixtures** live in `tests/fixtures/` and are read-only inputs; the
  ~100k-event benchmark fixture is generated on demand by `benchgen` (nothing
  multi-MB is committed) and `crates/cli/tests/bench_large.rs` asserts a
  time/memory budget over the whole `report` pipeline.
- **Layering** is one-directional: `gramps-dates → gramps-xml → event-core →
  writers / cli / web`. Writers only ever see the flat `EventRow` contract —
  they never parse the XML model.
- The web UI renders the exact same view builders the CLI prints.

## Attribution

The fixture data mirrors the **Gramps XML schema** (`grampsxml.dtd`, Gramps
XML 1.7.1 / 1.7.2); `data.gramps` originates from the Gramps project's
example data. Gramps is licensed GNU GPL v2-or-later
(https://gramps-project.org) — see `tests/fixtures/README.md` for details.
This repository copies no Gramps source code — only the document structure of
the interchange format users share.