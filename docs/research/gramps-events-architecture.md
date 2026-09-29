# Gramps Events Report — Architecture Plan

**Status:** Research / proposal — no code written yet
**Date:** 2026-09-29
**Decisions:** all 12 architecture decisions confirmed 2026-09-29 (see §10)
**Scope:** Architecture options with pros/cons, recommended design, and a step-by-step build plan for a Rust CLI + web application that exports all events from a `.gramps` file (birthdays, deaths, weddings, and every other event type) to CSV / Parquet / JSON / PDF and displays them as a list, an anniversary calendar, a calendar-with-years, or a timeline.

---

## 1. Goals and Requirements

The tool (working name: `gramps-events`) is a Rust re-implementation and generalization of Gramps' built-in **"Birthday and Anniversary Report"** (`plugins/textreport/birthdayreport.py`). Requirements distilled from the brief:

1. **Read any `.gramps` file** (Gramps XML export format).
2. **Include all events** — not just Birth / Death / Marriage, but every event type present in the file (custom types included, e.g. "Immigration", "Graduation", user-defined types).
3. **Event selection:** the user chooses which event types to include or exclude in the output. The GUI must enumerate the types actually present in the loaded file.
4. **Output formats:** CSV, Parquet, JSON, PDF — **any combination** of the four in a single run.
5. **CLI first, GUI second:** the CLI must work standalone; the GUI is a web-based front-end that supports the full workflow (pick file → choose options → export → view results).
6. **Four display modes in the GUI:**
   - **List** — flat, sortable table of events.
   - **Calendar (anniversary, no years)** — events organized by month and day; shows how many years have elapsed since each event.
   - **Timeline (with years)** — events organized by year, month, and day (chronological); range events render as bars spanning their full duration.
   - **Calendar-with-years** — a year-by-year month grid placing events on their actual dates, with ranges spanning their full extent.
7. **Date-range handling.** Date ranges and spans must be visible with their full range in the year-based views (list, timeline, calendar-with-years). In the anniversary calendar, a range whose start includes a month anchors at the start of the range (day 1 if the day is missing); a range without months never appears in the anniversary calendar (full rules in §8).
8. **All Rust**, both backend and (ideally) front-end.

### Non-goals (v1)

- Reading Gramps' native on-disk database format (BerkeleyDB/BSDD) directly. We parse the XML **export** format only, which is the file format users share and archive.
- Editing/round-tripping Gramps files back to Gramps. Read-only.
- GEDCOM import (a future nicety; the pipeline design below leaves room for it).
- Multi-user / remote web deployment. The web UI is a local single-user tool.
- Media (photos) handling beyond noting their existence.

---

## 2. Reference: how Gramps does it

The existing implementation (`birthdayreport.py`, Gramps 5.1.6, installed at `/usr/lib/python3/dist-packages/gramps/plugins/textreport/`) is the functional spec for our core behavior:

- **Data collection.** For each person: birth event (via the person's `get_birth_ref()`), death event (`get_death_ref()`), and — to avoid double-counting wedding anniversaries — only families where the person is the **father** are scanned for Marriage/Marriage-Alternative events; Divorce/Annulment events invalidate the marriage for that iteration.
- **Age / elapsed computation.** `elapsed = report_year − event_year` (simple year subtraction). When `elapsed == 0` the entry reads "*person, birth"; otherwise "*person, N" — with the number being the years elapsed.
- **Organization.** A `month → day → entries[]` dictionary; the report emits one page per month with day numbers and entries beneath them. Dead people are marked (✝), weddings use ⚭.
- **Options.** Include birthdays / anniversaries / death anniversaries (booleans), report year, show-event-year, living-only, filter by person/filter, name-format, private-data handling, optional holidays per country.

**What we generalize:** Gramps hard-codes three event categories (birth, death, marriage). We treat **every event type** uniformly: an event is an anniversary candidate if it has a calendar date; the calendar view shows it under its month/day and computes elapsed years against a reference year (default: current year, like Gramps' "Year of report").

---

## 3. The Gramps XML data format (what the parser must handle)

Verified against the example file `/home/tr/Documents/gramps_examples/data.gramps` (Gramps XML 1.7.1) and the authoritative DTD installed with Gramps 5.1.6 at `/usr/share/gramps/grampsxml.dtd`.

### 3.1 Container forms

A `.gramps` file can be any of (byte-magic detection):

| Form | Detection | Handling |
| --- | --- | --- |
| Plain XML | starts with `<?xml` / `<` | parse directly |
| Gzip-compressed XML | magic `1f 8b` | `flate2` → XML |
| Zip archive (Gramps "saved tree") | magic `PK` | `zip` crate; find and parse `data.gramps` inside |

The example ships as plain XML named `data.gramps` — which is also exactly the member name inside a zipped `.gramps` tree, so one code path handles both.

### 3.2 Document structure (`<database>` root)

Sections (all optional in practice, order fixed per DTD): `header`, `name-formats`, `tags`, `events`, `people`, `families`, `citations`, `sources`, `places`, `objects`, `repositories`, `notes`, `bookmarks`, `namemaps`.

Every primary record (`person`, `event`, `family`, `placeobj`, `source`, …) carries a unique `handle` attribute (an `ID` in DTD terms); cross-references are `hlink`/IDREF attributes resolved into a `handle → record` index. The root element has a fixed namespace (`http://gramps-project.org/xml/1.7.x/`); the parser must be namespace-tolerant and match on local names.

### 3.3 Elements relevant to events

```text
event   : type?, date?, place?, cause?, description?, attribute*, noteref*,
          citationref*, objref*, tagref*
          (attrs: id, handle, priv, change)

person  : gender, name*, eventref*, …, childof*, parentin*, …
          eventref -> hlink + role (e.g. "Primary")

family  : rel?, father?, mother?, eventref*, childref*, …
          (marriage, divorce, etc. events hang off the family)

placeobj: ptitle?, code?, pname+, coord?, placeref*, location*, …
          (attrs: type — e.g. Country / City / State)

date    : four interchangeable element forms:
          dateval   val="YYYY-MM-DD"  [type=before|after|about]
                     [quality=estimated|calculated] [cformat=…]
                     [dualdated="1"] [newyear=…]
          daterange start=… stop=…   (a range, e.g. "between 1914 and 1918")
          datespan  start=… stop=…   (a span: event lasted from start to stop)
          datestr   val="free text"  (text-only date)
```

### 3.4 Date model — the trickiest part

Gramps dates are **not** plain ISO strings. From `date.py` and `exportxml.py`:

- **Modifiers:** none, before, after, about (stored as `type=` on `dateval`), range, span (their own elements), text-only.
- **Quality:** estimated / calculated.
- **Calendars (7):** Gregorian, Julian, Hebrew, French Republican, Persian, Islamic, Swedish. Stored via `cformat=` when non-Gregorian. The anniversary semantics of Gramps convert everything to Gregorian for the report (`gregorian()`).
- **Dual dating** (`dualdated="1"`, e.g. Julian/Gregorian overlap) and **New Year starts** (`newyear=` — Jan 1, Mar 1, Mar 25, Sep 1) affect historical dates.
- Dates may be partial (year only, year-month) and BC (negative years). `val` is stored in the form `YYYY-MM-DD` via `get_iso_date()`.

**Consequence:** the core needs a small but real *date subsystem* (parse the four element forms → a typed `GrampsDate` with modifier/quality/calendar → normalize to Gregorian for sorting/anniversary math → preserve a human display string). See §6.2. Ranges/spans keep both endpoints; their view behavior (anchor-at-start for the anniversary calendar, full-range display elsewhere) is specified in §8.

### 3.5 Event-to-subject linkage

- Person events: `person.eventref*` with `role` ("Primary" is the default display role).
- Family events: `family.eventref*` — marriage/divorce/separation events are owned by the family, not by either spouse.
- Some events may not be referenced by anyone (orphans). Policy decision in §10 (default: include, subject = "—").
- People may reference the *same* event in multiple roles (e.g., both spouses are "Primary" on their wedding event). Deduplication matters in views.

---

## 4. Recommended high-level architecture

A **Cargo workspace** of small crates with strict layering. One binary; the web UI is a subcommand (`serve`) of the same executable so CLI and GUI share every line of core logic and ship as one artifact.

```text
gramps-events (Cargo workspace)
│
├── crates/
│   ├── gramps-xml/           # §6.1 — .gramps container detection + XML → typed model
│   │                         #        (mirrors the DTD; handles, not resolved links)
│   ├── gramps-dates/         # §6.2 — GrampsDate type, parse 4 element forms,
│   │                         #        modifiers/quality/calendars, Gregorian normalization
│   ├── event-core/           # §7   — resolve handles→records, link events↔subjects,
│   │                         #        compute age/elapsed, filter pipeline, view builders,
│   │                         #        EventRow (the writer contract) + PdfDocument model
│   ├── writers/              # §6.3 — csv / json / parquet / pdf backends behind traits
│   ├── cli/                  # §6.5 — clap: `report`, `list`, `serve` subcommands
│   └── web/                  # §6.4 — axum server, API routes, server-rendered UI, exports
│
├── docs/research/            # this document; later: ARCHITECTURE.md + TODO.md
└── tests/fixtures/           # copy of data.gramps + crafted XML edge-case files
```

**Why a workspace of small crates (vs. one big crate):**

- The boundary `gramps-xml` + `gramps-dates` → `event-core` → `writers` / `web` is exactly the *data pipeline* (ingest → transform → output). Enforcing it at the crate level keeps the pipeline one-directional.
- Each crate is independently unit-testable; the heavy `web` and `writers` deps don't pollute the parser build.
- `event-core` can later seed a GEDCOM importer (`gramps-gedcom` adapter → same model).

**Single binary vs. multiple binaries:** one binary keeps installation trivial (`cargo install` → one command). The doc's front-end recommendation (§6.4) is server-rendered, so `gramps-events serve` starts HTTP + opens a browser tab.

---

## 5. Core pipeline (data flow)

All outputs and all four GUI views consume the same resolved, filtered event stream. This is the heart of the design.

```text
 .gramps file ──▶ [container detect] ──▶ [XML parse] ──▶ Database{ raw records }
                                                              │
                                            handles = index(handle → record)
                                                              ▼
                       [resolve]  person/event/family/place links, name display,
                                  age-at-event, elapsed-vs-reference-year
                                                              ▼
                       [filter]   event types (include/exclude), people, date range,
                                  living-only, privacy           (options object)
                                                              ▼
                         ResolvedEvent list (one row per event-subject pair)
                        ┌───────────┬───────────┬───────────┬───────────┐
                        ▼           ▼           ▼           ▼
                   EventRow    EventRow    EventRow    PdfDocument(model)
                   (flat)      (flat)      (flat)      (title, months→days→entries)
                        │           │           │           │
                        ▼           ▼           ▼           ▼
                      csv        json       parquet       pdf (typst/genpdf)
```

Key property: **writers never see the XML model** — only `EventRow` (structured flat rows) and the pre-built `PdfDocument`. The GUI views are built from the same `ResolvedEvent` stream by `event-core`'s four view builders (`ListView`, `CalendarView`, `CalendarWithYearsView`, `TimelineView`), so CLI text output and GUI HTML render identical data.

---

## 6. Library options and analysis

### 6.1 XML / Gramps parsing

| Option | Pros | Cons |
| --- | --- | --- |
| **Hand-rolled parser on `roxmltree` 0.21** *(recommended)* | Read-only DOM fits a fixed, DTD-defined format; namespace-aware; panic-free API (`Result`); very stable and widely used; we control exactly which fields we keep (schema is small); easy to be *lenient* about unknown/optional sections | We write the mapping code (≈300–500 lines); XML attributes are strings, so we own all conversions/validation |
| `quick-xml` 0.42 (streaming) | Fastest; lowest memory; no DOM | Pull-parser means more state machine code; link resolution needs the whole doc anyway (events/people come in separate sections), so streaming buys little at this scale; more fiddly error handling |
| `serde-xml-rs` 0.8 | Declarative deserialize | Slow; historically poor namespaces/unions (the four date element forms are a union — awkward); attribute/child mixing is painful; largest friction with our hand-written `GrampsDate` |
| Existing crate **`gramps_xml` 0.1.0** (crates.io) | Already models the DTD as serde structs | 0.1.0, ~24 downloads, single-maintainer, has known bugs (cannot parse its own root namespace attribute), never validated against real-world Gramps files; not a safe foundation |
| GEDCOM crates (`gedcom`, `ged_io`, `gedcomx`…) | Would give GEDCOM support "for free" | Wrong input format — Gramps XML is a different schema; we'd still need an XML parser and a GEDCOM→model mapping; no shared type model between both worlds anyway |

**Recommendation:** hand-rolled `roxmltree`-based parser inside `gramps-xml` crate. The DTD is the spec; mirroring ~15 record types with only the fields we need keeps the crate tiny. Structure: `Database` → section structs → records, all keyed by `handle`; unknown elements/attributes ignored (forwards-compatible). A columnar `handle → record` index is built before resolution.

### 6.2 Dates

- **Civil-date math:** **`jiff` 0.2** *(recommended)* — the actively maintained, Temporal-inspired successor to `chrono`. `chrono` 0.4 (`NaiveDate`) is the older crate of the same lineage (slower-evolving, kept for compat); either works, but a new codebase should pick **jiff** and pin it in milestone 4. The `NaiveDate` signatures in §7.2 are chrono's names — swap to jiff's civil-date type with identical semantics.
- **`GrampsDate` model:** an enum capturing `Exact`, `Before(y)`, `After(y)`, `About(y)`, `Range(start,stop)`, `Span(start,stop)`, `Text(String)` plus `quality`, `calendar`, `dual_dated`, `new_year`. Partial dates (`YYYY`, `YYYY-MM`) are allowed by convention (`month=0` / `day=0` in Gregorian terms).
- **Converters:** v1 normalizes **Gregorian** and **Julian** exactly, via SDN conversion (public-domain algorithm, ~40 lines, the same one Gramps uses in `gcalendar.py`). The other five calendars (Hebrew, French Republican, Persian, Islamic, Swedish) are **not converted in v1** (decision D4): they degrade gracefully — display the stored text, exclude from anniversary math, warn once. A later release can add SDN converters for them in the same module.
- **Anniversary key:** (month, day) from the *normalized Gregorian* date — matching Gramps' own `gregorian()` behavior for the report.
- **Leap day:** Feb 29 events only occur in leap years. In non-leap reference years they are **folded to Feb 28 and explicitly labeled** as Feb 29 events (decision D6), so every event appears every year and the discrepancy stays visible to the reader.

### 6.3 Output writers

| Format | Recommended crate | Notes |
| --- | --- | --- |
| CSV | `csv` 1.4 | `serde`-driven; one row per `EventRow`; header line documents the schema |
| JSON | `serde_json` 1.0 | array of `EventRow` objects, `Option`s as `null`; generous pretty/compact toggle |
| Parquet | **`parquet` + `arrow` 60.x** (arrow-rs) *(recommended)* | Fixed flat schema (see the `EventRow` contract in §7.3); build `StructArray` → `RecordBatch` → `ArrowWriter`. Light, stable, no magic; round-trip test via `ParquetRecordBatchReader` |
| Parquet (alt.) | `polars` 0.55 | DataFrame ergonomics (`DataFrame::from_rows(...).write_parquet(...)`) — very convenient, and gives CSV/JSON writers too, but a much bigger dependency than we need for a fixed schema; keep as *optional* if the user later wants analysis features |
| PDF (alt. A) | **`typst` 0.15 + `typst-pdf`** *(recommended)* | Markup-based typesetting; calendar-page grids, page numbers, headers/footers, beautiful typography out of the box — the Gramps report is 12 calendar pages, which Typst renders trivially. Active, high-quality project. Costs: large dependency (long first compile), font supply via `fontdb` (load system fonts). Pin typst/typst-pdf to the exact verified patch (their APIs churn between minor releases) |
| PDF (alt. B) | `genpdf` 0.2 (on `printpdf` 0.12) | Pure Rust, simple document model (paragraphs/tables/styles), small dep tree, perfect for a *text* report like Gramps' text output — but calendar *grid* layout is manual and awkward; project is barely maintained (0.2.0 for years) |
| PDF (alt. C) | `printpdf` 0.12 directly | Full low-level control (what Gramps' drawreport effectively does); most code, most effort; only worth it if we want to replicate exact calendar-page borders |
| PDF (alt. D) | HTML → PDF via headless browser/wkhtmltopdf | Nice output but external non-Rust dependency — violates "CLI standalone, offline" requirement; rejected |

**Recommendation:** writers behind traits (`EventWriter` for rows, `PdfBackend` for the document model). v1 ships **csv, json, parquet (arrow-rs), pdf (typst)**. Because CSV/JSON/Parquet are trivial row dumps, they share `EventRow`; only PDF has its own document model (sections/months/days), which `event-core` builds and `writers::pdf` renders.

### 6.4 Front-end framework — the main decision

Environment facts: Rust 1.91 available; Node 25 also available, but the project is "written in Rust" — a JS toolchain is a legitimate but non-preferred option.

All options below assume the same local-server model: the binary embeds an HTTP server; the browser talks to a small JSON API; exports stream as downloads. (Alternative deployment models — Tauri desktop shell, always-running server — are covered in the last row.)

| Option | Pros | Cons |
| --- | --- | --- |
| **Axum 0.8 + Askama 0.16 (or Maud 0.27) server-rendered HTML + HTMX 2 + ≈150 lines vanilla JS** *(recommended)* | 100% Rust, no Node, no WASM pipeline, no package.json; ideal for our UI shape (a form of checkboxes + four data-dense views: tables, month grids, a year grid, and a bar timeline); HTMX gives interactive re-rendering of filtered views with tiny code; trivially testable (axum `oneshot`/TestServer + `reqwest`); one binary; instant builds vs WASM | Interactivity beyond HTMX idioms needs some hand-written JS/CSS (tab switching, table sorting, calendar day hover, timeline-bar positioning); not a "reactivity framework", so complex client state would be manual — our state is simple enough that this is fine |
| **Leptos 0.8 full-stack (SSR + client-side WASM)** | Pure Rust end-to-end; fine-grained signals make filters/views feel native-SPA; SSR for first paint; strongest "Rust web" story | Heavy compile times; framework churn (0.9 is in beta); learning curve; WASM tooling (trunk/wasm-bindgen) added to every build; we'd still want axum (or Leptos' own server) for exports — two frameworks in the app |
| **Yew 0.23 SPA** | Mature, stable; familiar component/Elm-ish model | Client-only → we must write and maintain a separate JSON API anyway; no SSR; more boilerplate than Leptos for this project's needs |
| **Dioxus 0.7 fullstack** | One codebase; signals; promising | Ecosystem/community smaller; 0.8 alpha in flight; same WASM-tooling costs as Leptos |
| **TypeScript SPA (React/Vue/Svelte, Vite) served by Axum** | Best-in-class UI ecosystem (calendar/table components, charting); fast dev iteration | Two languages + Node toolchain; the "Rust tool" claim weakens; CORS/dev-server complexity; bigger maintenance surface for a tool whose UI is three data views |
| **Tauri 2 desktop app** | Native window + OS file dialogs; packaged installer; same HTML/JS/Rust stack | Not "web-based" (client runs in a webview, no browser tab); heavy build chain (per-OS tooling, bundling); needs a web UI anyway (same decision recurs); overkill for a local tool that's served at `localhost` |
| **egui/eframe 0.36 native GUI** | Pure Rust, immediate mode, zero web | Not web-based at all; forms/tables long-term are workable but clunky vs HTML/CSS; rejects the brief's "web-based front-end" |

**Recommendation — Axum + Askama + HTMX, server-rendered.** Rationale:

1. The UI's real complexity is the *data pipeline*, not the view layer. The views are a filter form plus four data-dense renderings — a sortable table, per-month anniversary grids, a year-by-year calendar grid, and a bar timeline — all trivially expressible as server-rendered HTML with light CSS (and a little JS for bar geometry).
2. The filtering workflow (checkbox set of event types → re-render views) is a textbook HTMX exchange: POST/GET the options to a partial-HTML endpoint; swap in new table/grid content. No client-side state to keep in sync.
3. All views are HTML tables/grids/bars produced by the *same* view builders as the CLI text output — no duplication, and the server renders exactly what the CLI prints.
4. It keeps the toolchain pure Rust with the smallest dependency surface and the fastest iteration (`cargo build` seconds, not minutes) — important because the heavy lifting (parser, dates, PDF) lives in the other crates.

**File selection in the GUI.** A browser `<input type="file">` cannot hand absolute paths to the server, so the web UI uses **upload**: the user picks the `.gramps` file in the browser, it is POSTed to the API, stored in a temp dir, parsed, and the event-type list is returned. This sidesteps browser sandboxing entirely, works on every OS, and doubles as an easy test path. (The CLI keeps ordinary filesystem paths.) A hard upload cap (200 MB) is enforced before the file is read into memory; uploads are stored under **generated** temp-file names (the browser-supplied filename is never used as a path or trusted) and are deleted on `POST /api/reset` and at process exit. A soft size warning appears above 50 MB; Gramps XML is text and compresses well.

### 6.5 CLI framework

| Option | Pros | Cons |
| --- | --- | --- |
| **clap 4 (derive)** *(recommended)* | Standard; `--help` generation, value parsing, subcommands; near-zero ceremony with `#[derive(Parser)]` | Slightly slower compile than hand-rolled parsing (irrelevant here) |
| Hand-rolled `std::env::args` | Zero dependencies | Worse UX and more maintenance than clap |

**CLI surface (sketch):**

```text
gramps-events report  <file.gramps>  [--include-types Birth,Death]   # event selection
                                     [--exclude-types …]
                                     [--format csv,json,parquet,pdf | --format all]
                                     [--out-dir DIR] [--out-prefix NAME]
                                     [--reference-year 2026]          # elapsed-years base
                                     [--living-only] [--include-private]
                                     [--date-range 1900-01-01..2026-12-31]
gramps-events list    <file.gramps>  [same filters] [--view list|calendar|timeline|yrcal]   # default list
                                     # text render of any view (list is default)
gramps-events serve   [--port 8380]  [--open]   # start the web GUI
gramps-events inspect <file.gramps>  # print event types + counts (what the GUI enumerates)
```

`report` is the CLI form of the full GUI workflow: the same options object, the same `event-core` functions, the same writers.

## 7. Detailed module design

### 7.1 `gramps-xml` crate (parse layer)

Public API sketch:

```rust
/// Detect container (xml/gzip/zip), decode bytes, parse into typed model.
pub fn parse_database(bytes: &[u8]) -> Result<Database, GrampsXmlError>;

pub struct Database {
    pub header: Header,                    // created date, version, researcher
    pub events: Vec<Event>,
    pub people: Vec<Person>,
    pub families: Vec<Family>,
    pub places: Vec<Place>,
    pub notes: Vec<Note>,
    pub sources: Vec<Source>,              // parsed but mostly opaque for v1
    pub objects: Vec<MediaObject>,         // presence only
    // repositories, tags, citations, bookmarks, namemaps: light/optional parsing
}

pub struct Event {
    pub handle: String,
    pub gramps_id: Option<String>,         // e.g. "E0000"
    pub event_type: EventType,             // known set + Custom(String) variant
    pub date: Option<GrampsDate>,          // from gramps-dates
    pub place_handle: Option<String>,
    pub cause: Option<String>,
    pub description: Option<String>,
    pub private: bool,                     // priv="1"
    pub change: i64,
}

pub struct Person {
    pub handle: String,
    pub gramps_id: Option<String>,
    pub gender: Gender,                    // M | F | U
    pub names: Vec<PersonName>,            // primary + alternates (Birth/Married/… Name)
    pub event_refs: Vec<EventRef>,         // (event_handle, role)
    pub child_of: Vec<String>,             // family handles
    pub parent_in: Vec<String>,            // family handles
    pub private: bool,
}

pub struct Family {
    pub handle: String,
    pub gramps_id: Option<String>,
    pub father: Option<String>,            // person handle
    pub mother: Option<String>,
    pub children: Vec<String>,
    pub event_refs: Vec<EventRef>,
    pub private: bool,
}

pub struct Place {
    pub handle: String,
    pub gramps_id: Option<String>,
    pub place_type: String,                // Country / City / State / …
    pub name: String,                      // <pname value=…>
    pub parent_handle: Option<String>,     // place hierarchy via placeref, if present
}

pub struct EventRef { pub event_handle: String, pub role: String }
```

XML mapping notes: attributes arrive as strings, so this crate owns all string→typed conversions and date parsing; unknown elements and attributes are skipped (tolerant, forward-compatible); the four `date*` element forms map onto `gramps-dates`. Parse-boundary validation: a record missing its `handle` (and any duplicated handle) is a hard parse error naming the record type/position; `daterange`/`datespan` with `stop < start` is a hard parse error; malformed *known* elements (bad attribute value, truncated section) warn and skip the record so the rest of the file stays readable. Each of these has a dedicated unit test (milestones 2, 4).

### 7.2 `gramps-dates` crate

```rust
pub enum Calendar { Gregorian, Julian, Hebrew, FrenchRepublican, Persian, Islamic, Swedish }
pub enum Modifier { None, Before, After, About, Range, Span, TextOnly }
pub enum Quality { None, Estimated, Calculated }

pub struct GrampsDate {
    pub calendar: Calendar,
    pub modifier: Modifier,
    pub quality: Quality,
    pub ymd: (i32, u32, u32),              // ISO-ish; month/day may be 0 (partial)
    pub stop: Option<(i32, u32, u32)>,     // Range / Span
    pub dual_dated: bool,
    pub new_year: NewYear,                 // Jan1 | Mar1 | Mar25 | Sep1
    pub display: String,                   // what Gramps would print, e.g. "about 1900"
}

impl GrampsDate {
    pub fn parse_xml(&mut self, node: &roxmltree::Node) -> Result<(), DateError>; // all 4 forms
    pub fn to_gregorian(&self) -> Option<NaiveDate>;         // None if not convertible
    pub fn anniversary_key(&self) -> Option<(u32, u32)>;   // anchor (month, day) per §8 rule 1:
                                                           //   full date → (m, d); month without
                                                           //   day → (m, 1); year-only → None
    pub fn start(&self) -> Option<NaiveDate>;              // single date / range-span start
    pub fn stop(&self) -> Option<NaiveDate>;               // range-span end (None for single dates)
    pub fn is_range(&self) -> bool;                        // Range | Span, any quality
    pub fn year(&self) -> Option<i32>;                     // start year
}
```

Gregorian/Julian conversion via SDN (public-domain formulas, the same ones in Gramps' `gcalendar.py`); the other five calendars use the same published SDN algorithms, with a graceful text-only fallback (warn once, exclude from anniversary math) until ported. The `NaiveDate` names above are chrono's; if jiff is chosen (§6.2), use its civil-date type with the same semantics.

### 7.3 `event-core` crate (the engine)

```rust
pub struct ReportOptions {
    pub reference_year: i32,                        // default: current year
    pub include_types: Option<HashSet<String>>,     // None = all; else whitelist
    pub exclude_types: HashSet<String>,
    pub include_people: Option<HashSet<String>>,    // gramps ids / handles
    pub date_range: Option<(NaiveDate, NaiveDate)>,
    pub living_only: bool,
    pub include_private: bool,
    pub leap_day: LeapDayPolicy,                    // FoldToFeb28 (labeled) — D6
    pub dedupe_same: bool,                          // collapse (type, subject, mm-dd) dupes
    pub show_orphans: bool,                         // default true — D7}

pub struct ResolvedEvent {
    pub event_type: String,                         // display type of the event
    pub date: GrampsDate,
    pub gregorian: Option<NaiveDate>,
    pub subjects: Vec<PersonDisplay>,               // resolved from eventrefs / family
    pub role: String,
    pub place_path: Option<Vec<String>>,            // place + parent chain, e.g. ["England"]
    pub age_at_event: Option<(i32, u32)>,           // years/months when birth is known
    pub elapsed_years: Option<i32>,                 // reference_year − year
    pub private: bool,
    pub orphan: bool,
}

pub struct EventRow {                       // THE flat output contract (serde)
    pub person_id: Option<String>,
    pub person_name: String,
    pub event_id: Option<String>,
    pub event_type: String,
    pub event_date: Option<String>,         // ISO of normalized date (or start)
    pub event_date_text: String,            // Gramps display string, e.g. "about 1900"
    pub event_date_stop: Option<String>,    // range/span stop (ISO), None for single dates
    pub date_is_range: bool,                // daterange | datespan (any quality)
    pub year: Option<i32>,
    pub month: Option<u32>,
    pub day: Option<u32>,
    pub anniversary_month: Option<u32>,
    pub anniversary_day: Option<u32>,
    pub leap_day_folded: bool,            // D6: Feb-29 anchor folded to Feb 28 in a non-leap ref year
    pub place: Option<String>,
    pub role: String,
    pub age_at_event: Option<String>,
    pub reference_year: i32,                 // constant per run — keeps a single file self-describing
    pub elapsed_years: Option<i32>,
    pub private: bool,
}

// `EventRow` is the immutable v1 flat-file contract shared by CSV / JSON / Parquet.
// Adding or reordering fields later is a BREAKING schema change for Parquet readers,
// so any evolution must bump a `schema_version` field or ship a new file format.

// The four GUI + CLI views:
pub enum View {
    List(ListView),
    Calendar(CalendarView),                       // anniversary calendar, no years
    CalendarWithYears(CalendarWithYearsView),     // year-by-year month grid (D12)
    Timeline(TimelineView),                       // chronological, range bars (D11)
}

pub fn collect_events(db: &Database, opts: &ReportOptions) -> Vec<ResolvedEvent>;
pub fn build_view(events: &[ResolvedEvent], kind: ViewKind, opts: &ReportOptions) -> View;
pub fn rows(view: &View) -> Vec<EventRow>;                     // → csv / json / parquet
pub fn build_pdf_document(view: &View, opts: &ReportOptions) -> PdfDocument;
```

Semantics implemented here (all matching §8): subject resolution rules, couple rendering, orphan fallback, dedup, elapsed-years computation, leap-day policy.

### 7.4 `writers` crate

```rust
pub trait EventWriter { fn write(&self, rows: &[EventRow], dest: &Path) -> Result<(), WriterError>; }
pub struct CsvWriter;
pub struct JsonWriter;
pub struct ParquetWriter;

pub trait PdfBackend { fn render(&self, doc: &PdfDocument, dest: &Path) -> Result<(), WriterError>; }
pub struct TypstPdf;                      // genpdf/printpdf implementations possible later

pub fn factory(formats: Formats, opts: &WriterOptions) -> Vec<Box<dyn EventWriter>>;
```

`Formats` is a bitflag (`csv | json | parquet | pdf`, any combination) shared by CLI and GUI.

All writers write to a temp file in the destination directory and rename it over the destination only on success (POSIX-atomic), so an aborted export never leaves a partial file at the final path.

## 8. Semantics decisions (the "anniversary" contract)

1. **Anniversary calendar (calendar without years).** A *recurring* view: each event appears once per year on its **anchor** (month, day), labeled `elapsed = reference_year − anchor_year`.
   - **Anchor rule:** an anchor exists iff the date — or the *start* of a range/span — has a month. Full dates anchor on (month, day); dates and range/span starts with a month but no day anchor on **(month, 1)**; year-only dates and year-only ranges have **no anchor** and are excluded from this view (they appear in the year-based views instead, flagged). Only Gregorian-normalizable dates anchor (D4).
   - **Feb 29** anchors fold to Feb 28 in non-leap reference years and are labeled as Feb 29 events (D6); output rows record the fold as `leap_day_folded = true` while `event_date` keeps the true Gregorian date.
2. **Ranges and spans (any quality).** `daterange` and `datespan` behave identically (D9). The full range is always displayed wherever the event appears (e.g. "Nov 1822 – Apr 1823"; ISO endpoints land in `event_date` / `event_date_stop` and `date_is_range = true`). In the anniversary calendar, a range whose start includes a month anchors at the start of the range (day 1 if the day is missing) and `elapsed = reference_year − start_year`; a range whose bounds exclude months (e.g. "1822–1824") is **not** shown in the anniversary calendar, but is shown in full in the list, timeline, and calendar-with-years views.
3. **Partial dates.** Month-only dates (single or range/span start) anchor at day 1 (D10); year-only dates have no anchor.
4. **Reference year.** Defaults to the current year and is user-adjustable (Gramps' "Year of report"). `elapsed < 0` renders "—" (event in the future relative to the reference year); `elapsed == 0` renders "this year".
5. **Weddings (family events).** A family's Marriage / Marriage-Alternative events render as "A ⚭ B"; both spouses are subjects; elapsed years shown as for any event. Every dated event is shown, so a marriage stays visible even if a later divorce exists — no divorce-based suppression (D5).
6. **Subject resolution order:** (a) Primary-role eventrefs on people; (b) otherwise any eventref'd person; (c) family events → the couple; (d) otherwise single-reference events by any role; (e) orphans → `person_name = "—"`, toggleable (D7). Dedup rule: one row per (event, subject) — an event referenced by two people yields two rows; `dedupe_same` also collapses identical (type, subject, month, day) rows.
7. **Privacy:** Gramps marks records `priv="1"`. Default: excluded. `--include-private` shows them (flagged).
8. **Names:** primary name rendered `First Surname`. Gramps' name-format and married-woman-surname options are documented v1.1 gaps.
9. **Age at event** (e.g. age at death) is computed from the person's birth event when both are full dates; partial dates give best-effort years only.
10. **Timeline view (with years).** Chronological order by start date, grouped under year headers; single dates render as entries/markers; ranges and spans render as **visual bars spanning start → stop** (D11) — year-only ranges render as full-year-width bars; each bar is labeled with the event type, subject(s), and the full range text.
11. **Calendar-with-years view.** A fourth view (D12): a year-by-year month grid covering the span of the event data; events sit on their actual date cells; ranges span from their start to stop cells (year-only ranges shade the whole month span across years). Placement reuses the anchors from rule 1; the full range text is always shown.

12. **Deterministic output order.** Views and writers emit a total, reproducible order: list and timeline by (start date ascending, event type, primary subject, event id); the anniversary calendar by (month, day, type, subject); calendar-with-years by (start date, type, subject, id). Output never depends on set/hash iteration order; undated events sort after all dated entries (rule 13).
13. **Undated events.** An event with no date element, or a text-only `datestr`, has no anchor and no year: it appears in the list view and in the timeline as a terminal "Undated" group (sorted by type, then subject), and never in the anniversary calendar or calendar-with-years. `event_date`/`year` are null; `event_date_text` carries the display string.
14. **Type-filter precedence.** When both `--include-types` and `--exclude-types` are set, exclusion wins: a type in both sets is excluded. `include_types = None` (the default) means "all types", with exclusions subtracted.
15. **Living determination (`--living-only`).** Port Gramps' `gramps.gen.utils.alive.probably_alive()`: a person is **dead** when a death-type event (Death, Cremation, Burial) references them (any date); otherwise living *unless* the estimate applies — birth date known and `reference_year − birth_year ≥ 110` (Gramps' default maximum-age span) → presumed dead; birth unknown → living. Land in `event-core`; golden-test against Gramps on the fixture set (§11).

## 9. Web app design (`web` crate)

Server: **Axum 0.8**; templates: **Askama 0.16**; static assets (CSS, ≈150 lines of vanilla JS — timeline-bar and year-grid positioning, tab switching — plus vendored **htmx 2.x** single file) served from a compiled-in `static/` directory. State: `Arc<AppState>` holding the loaded `Database`, current `ReportOptions`, and a temp-dir path.

The server binds **127.0.0.1** by default — this is a local, single-user tool and the data is family PII; no route binds an external interface without an explicit new `--host` flag.

| Route | Purpose |
| --- | --- |
| `GET /` | Landing page (file picker / resume with loaded file) |
| `POST /api/load` | multipart upload (size-capped) → generated temp file → parse → store in state; returns `{event_types: [{type, count}], people_count, …}` |
| `GET /api/options` | current options |
| `PUT /api/options` | update include/exclude types, reference year, living-only, private, leap-day, person filter |
| `GET /api/events?view=list/calendar/yrcal/timeline` | rendered view as an HTML fragment (HTMX target swap) |
| `GET /api/events.json` | same data as `EventRow` JSON (debug / inspection) |
| `GET /api/export?format=csv,parquet,pdf` | streams download(s) with `Content-Disposition`; generated by `event-core` + `writers` |
| `POST /api/reset` | unload current file |

UI layout: a header with the file name, reference-year input, event-type checkboxes with counts, orphan/privacy/leap-day toggles, and an export button group; below, four tabs —

- **List** — sortable HTML table (date, type, person(s), place, elapsed; ranges show start–stop and `date_is_range`).
- **Calendar** — per-month anniversary grids (month → day → entries); range events anchor at the range start with the full range text shown.
- **Calendar with years** — year-by-year month grids; events sit on their actual dates and ranges span their full extent.
- **Timeline** — chronological grouping with year headers; range events render as CSS bars spanning start–stop.

Export buttons call the export endpoint with the *current* options. Everything is server-rendered, so the browser tab survives refresh and the whole UI is testable without a browser. Askama auto-escapes template output; an escaping regression test (§11) locks in safe rendering of hostile names, descriptions, and dates.

## 10. Decisions (confirmed 2026-09-29)

All twelve decisions below were confirmed by the user prior to implementation. D4 and D6 deviate from the original defaults; D9–D12 are the date-range / calendar-handling requirements added on 2026-09-29.

| # | Decision | Outcome | Note |
| --- | --- | --- | --- |
| D1 | Front-end framework | **Axum + Askama + HTMX** (server-rendered) | Full detail in §6.4 |
| D2 | PDF engine | **Typst** (`typst` + `typst-pdf`) via `PdfBackend` trait | genpdf/printpdf remain as swappable alternatives |
| D3 | Parquet writer | **arrow-rs** (`parquet` + `arrow`) | polars alternative rejected in v1 |
| D4 | Non-Gregorian calendars | **Gregorian + Julian only in v1**; other 5 calendars degrade to text display with warnings | *Deviates from original default* (SDN for all 7 deferred to a later release) |
| D5 | Marriages after divorce | **Show all events** — no divorce-based suppression | Confirms the original proposal default; Gramps itself suppresses after divorce (§2) — intentionally dropped |
| D6 | Feb 29 handling | **Fold to Feb 28 in non-leap years, labeled as Feb 29** | *Deviates from original default* (hide); labels keep the discrepancy visible |
| D7 | Orphan events | **Included**, subject "—", toggleable | Matches original default |
| D8 | Zip/gzip containers | **All three supported** (XML, gzip, zip) | Matches original default |
| D9 | Range rule scope | **Both `daterange` and `datespan`, uniformly, any quality** — anchor at start if start has a month | Added 2026-09-29 |
| D10 | Month-only dates | **Anchor at day 1** (single dates and range/span starts) | Added 2026-09-29 |
| D11 | Timeline range rendering | **Visual bars spanning start–stop** (year-only ranges = full-year-width bars) | Added 2026-09-29 |
| D12 | "Calendar with years" | **New fourth view: year-by-year calendar grid** showing ranges in full | Added 2026-09-29 |

## 11. Testing strategy

- **Fixtures.** Commit a copy of `/home/tr/Documents/gramps_examples/data.gramps` (6.9 KB, 5 people, 6 events) as `tests/fixtures/`, plus hand-crafted XML: every date form (before/after/about/range/span/text), ranges with months (e.g. Nov 1822 – Apr 1823), spans, year-only ranges (1822–1824), month-only partial dates, dual dates, partial dates, BC dates, non-Gregorian calendars, zipped and gzipped containers, orphan events, family events, multiple roles, private records, and unknown/unversioned sections (forward-compat), plus a generated large-file smoke fixture (~100k events) used to assert a sanity time/memory budget on `report` (performance proportionality).
- **Unit tests.** Per crate. Dates: golden parse tables checked against Gramps' own parser output on identical strings (Gramps 5.1.6 is installed here). Anchor rule: day-1 default for month-only dates and range starts, year-only exclusion from the anniversary calendar, Feb 29 folding. Range handling: elapsed derived from start year, `event_date_stop` / `date_is_range` in rows, timeline bar geometry, calendar-with-years placement. Resolution: couple rendering, role precedence, dedup. Filters: include/exclude/range/living/private combinations.
- **Property tests (`proptest`).** Date parse→display→reparse round-trips; elapsed-years arithmetic vs an oracle; parquet read-back equals written rows; dedup idempotency (`dedupe_same` applied twice equals once).
- **Writer tests.** CSV/JSON parse-back equality; Parquet `ParquetRecordBatchReader` round-trip; PDF renders to a temp file, starts with `%PDF` magic, page count ≥ 1, plus a spot text check.
- **CLI tests.** Snapshot tests of `inspect` / `list` / `report` output against fixtures, including `--view yrcal`; `--format` edge cases (no formats → clap error; `--format all` expands to exactly csv, json, parquet, pdf).
- **Web tests.** Axum integration tests (Tower `oneshot`): load → options → views → export return 200 with correct `Content-Type` / `Content-Disposition`; the events fragment contains the expected entries. Optional headless-browser pass for visual checks. An escaping test asserts that `<script>`-bearing names/dates render escaped in every view fragment.
- **Cross-check.** Run the same fixture through Gramps' own Birthday & Anniversary report and diff birth/death/marriage entries against our calendar view.

### 11.1 Acceptance criteria (requirement → verification)

| Req (§1) | Verifiable acceptance |
| --- | --- |
| 1. Read any `.gramps` file | All three containers (plain XML / gzip / zip) parse from fixtures; corrupt containers raise a decode error naming the file/position (milestones 2, 14). |
| 2. All event types, incl. custom | `inspect` on a fixture with a user-defined type enumerates it; `list`/`report` emit a row for it; uniform anniversary treatment (milestone 8 golden tests). |
| 3. Event-type selection | include/exclude unit tests incl. rule 14 precedence; the GUI's checkbox list equals `inspect` output (milestones 7, 12). |
| 4. Any combination of the four formats | One `report` run with all four formats produces four files with correct magic headers; each non-empty subset also tested; `--format all` == exactly the four (milestones 9–11). |
| 5. CLI first, GUI second | `cargo build --release` + a single `report` command produces all outputs with no server and no Node toolchain; `serve` starts standalone (milestones 11–13). |
| 6. Four display modes | Each view builder has golden fixtures; the GUI renders the same builders' data in four tabs (milestones 8, 13). |
| 7. Date-range handling | Rules 2, 3, 9, 11, 13 golden-tested: anchor-at-start with day-1, no-month exclusion from the anniversary calendar, range bars in the timeline, full-extent spans in calendar-with-years, undated events in list/timeline only (milestone 8). |
| 8. All Rust | Build, lint and CI use only Rust tooling; htmx is the only vendored non-Rust artifact (a static file); a single Rust binary ships (milestones 1, 12). |
| — `--living-only` | Cross-checked against Gramps' `probably_alive` on the fixture set (rule 15; milestones 7, 11). |
| — Privacy (`priv="1"`) | Default runs exclude private records; `--include-private` shows them flagged (milestones 7, 11). |
| — Feb 29 fold | `leap_day_folded = true` column + label in every output where the fold applies (milestones 8–10). |

## 12. Implementation milestones (commit-by-commit)

Each step is a small, testable unit: write code + tests, run `cargo test` / `cargo clippy`, commit with a conventional message, then proceed.

1. **Workspace scaffold.** Cargo workspace, crate skeletons, `tests/fixtures/` (real example + crafted XML), rustfmt/clippy config.
2. **`gramps-xml`: container + skeleton.** Detect xml/gzip/zip; parse header, events, people, families, places minimally; handle indexes; tests on `data.gramps` plus at least one corrupt-container decode-error test (truncated gzip / non-gramps zip) — the full error-fixture sweep lands in step 14.
3. **`gramps-dates`: core type + `dateval`.** `GrampsDate`, modifiers, quality, partial dates; `dateval` parsing; golden tests incl. about/before/after.
4. **`gramps-dates`: compound + text dates.** `daterange`, `datespan`, `datestr`; display strings; pin the civil-date crate (**jiff**, §6.2) here; Gregorian/Julian SDN conversion; non-Gregorian fallback with warnings.
5. **`gramps-xml`: full records.** Name parsing (multiple names, surname prefix/prim), eventref roles, family members, place hierarchy, privacy flags; unknown-element tolerance.
6. **`event-core`: resolution.** Index building, subject resolution incl. couples and orphans, place paths, dedup sets, `collect_events`.
7. **`event-core`: derived values + filtering.** `elapsed_years`, `age_at_event`, anniversary keys, leap-day policy, full `ReportOptions` pipeline.
8. **`event-core`: views.** List / Anniversary-Calendar / Timeline (with range bars) / Calendar-with-years builders + `EventRow` (incl. `event_date_stop`, `date_is_range`, `leap_day_folded`); golden tests mirroring Gramps report entries for full dates plus our anchor/range rules.
9. **`writers`: csv / json / parquet.** `EventWriter` trait + three writers; round-trip tests.
10. **`writers`: pdf.** `PdfDocument` + Typst backend (calendar-month pages, title, reference year); page-count/magic test.
11. **CLI.** clap commands `inspect`, `list`, `report` with all filters, `--view` covering all four views (incl. `yrcal`), and format combinations; snapshot tests; standalone check: one command produces all four files.
12. **`web`: API + skeleton UI.** Axum server on 127.0.0.1, upload/load/options/export routes with size cap + generated temp names, minimal landing page with htmx; integration and escaping-regression tests.
13. **`web`: full UI.** List / anniversary-calendar / calendar-with-years / timeline views (CSS range bars), event-type checkboxes, toggles, export buttons, styling; end-to-end tests.
14. **Hardening.** Error messages, large-file warnings, empty/error fixtures, generated large-file benchmark fixture with a time/memory budget, `README.md`, `docs/ARCHITECTURE.md`, DTD-derived structure attribution, and a pass of the §11.1 acceptance checklist.

Steps 2–8 are pure core work and land before any UI, keeping the CLI useful from step 11 onward.

## 13. Appendix — reference material found during research

- Gramps XML DTD 1.7.1 / 1.7.2 (authoritative schema): `/usr/share/gramps/grampsxml.dtd` — this plan's §3 mirrors it.
- Gramps 5.1.6 Birthday & Anniversary report (behavioral spec): `/usr/lib/python3/dist-packages/gramps/plugins/textreport/birthdayreport.py`.
- Gramps date engine (modifiers, quality, calendars, SDN): `/usr/lib/python3/dist-packages/gramps/gen/lib/date.py`, `gcalendar.py`; XML serialization in `plugins/export/exportxml.py`.
- Existing `gramps_xml` crate (0.1.0, ~24 downloads): too immature to adopt; noted as a future contributor target instead.
- Prior art: `gramps_examples/data_website` is a Zola site generated by a one-off `gramps_to_zola` tool from the same fixture — evidence the parse is tractable; not part of this plan.
- Crate versions verified 2026-09-29: axum 0.8, askama 0.16, maud 0.27, tera 2.4, leptos 0.8 (0.9 β), yew 0.23, dioxus 0.7, tauri 2.12, egui/eframe 0.36, roxmltree 0.21, quick-xml 0.42, chrono 0.4.45, jiff 0.2, csv 1.4, serde_json 1.0, parquet/arrow 60, polars 0.55, printpdf 0.12, genpdf 0.2, typst/typst-pdf 0.15, clap 4.6, zip 8.6, flate2 1.1, anyhow 1.0, thiserror 2.0.
