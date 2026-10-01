# Fix Plan — Blank Pages in PDF Export

**Status:** Research / proposal — fix not yet applied
**Date:** 2026-10-01
**Scope:** Root-cause analysis and a step-by-step fix plan for the `gramps-events`
PDF export producing **valid but blank** PDFs. Affects both the web UI
(`GET /api/export?format=pdf`) and the CLI `report --format pdf` because both
share the same `TypstPdf` renderer in `crates/writers/src/pdf.rs`.
**Decisions:** confirmed 2026-10-01 — (1) deliver plan only; (2) bundle typst's
fonts (accept ~9 MB binary growth) rather than system-font lookup; (3) include
robustness/test hardening, not just the minimal one-line fix.

---

## 1. Problem Statement

When the tool exports a report to PDF, the resulting file is a valid,
12-page A4 PDF whose pages are **completely blank**:

- Web UI export → `/home/tr/Documents/gramps-events.pdf`
- CLI `report --format pdf` → `/home/tr/Downloads/gramps_events_output/data.pdf`

Evidence from the shipped files:

- `file` reports a well-formed `PDF document, version 1.7, 12 page(s)`.
- `pdfinfo` reads Title / Creator (`Typst 0.14.2`) / page size fine.
- `pdftotext` extracts **only form-feed characters** — no text layer.
- `pdftoppm` renders pages with **0 non-white pixels** (verified by pixel
  analysis) — genuinely blank, not a text-extraction quirk.

## 2. Reproduction

```bash
# CLI
cargo run -p cli --bin gramps-events -- report tests/fixtures/data.gramps \
  --format all --out-dir /tmp/out --reference-year 2030
pdftoppm -png /tmp/out/data.pdf /tmp/page     # pages come out blank

# Web UI
cargo run -p cli --bin gramps-events -- serve [--port 8380]
# load tests/fixtures/data.gramps, click Export → PDF
```

Both routes feed the same code path (`TypstPdf::compile`), so one root cause
explains both.

## 3. Root Cause (confirmed)

The PDF renderer lives in `crates/writers/src/pdf.rs`. `PdfWorld::new` builds
its font book from typst's bundled fonts:

```rust
let fonts: Vec<Font> = typst_assets::fonts()
    .filter_map(|data| Font::new(Bytes::new(data), 0))
    .collect();
let book = FontBook::from_fonts(&fonts);
```

`typst-assets` ships a `fonts` Cargo feature that is **not enabled by default**:

```rust
/// Bundled fonts.
///
/// This returns an empty iterator if the `fonts` feature is disabled.
pub fn fonts() -> impl Iterator<Item = &'static [u8]> {
    #[cfg(not(feature = "fonts"))]
    return [].into_iter();

    #[cfg(feature = "fonts")]
    [ /* Libertinus Serif, NewCM, DejaVuSansMono … */ ]
}
```

The workspace declares the dependency without the feature:

```toml
# Cargo.toml (workspace)
typst-assets = "=0.14.2"        # ← `fonts` feature OFF (the default)
```

`crates/writers/Cargo.toml` uses `typst-assets = { workspace = true }`, and no
other crate enables `fonts`, so `typst_assets::fonts()` returns **an empty
iterator**. `PdfWorld` therefore builds an **empty `FontBook`** — zero fonts
available.

### What happens next (and why it is silent)

1. The generated Typst markup begins with `#set text(font: "Libertinus Serif")`.
2. With an empty font book, Typst still **compiles** the document — it does not
   hard-fail. It emits one diagnostic, `unknown font family: libertinus serif`,
   at *warning* severity.
3. Because there are **no fonts at all**, every text glyph in the document is
   silently dropped at layout — not just the title and entries, but even the
   implicit footer text `Page N` (which uses the same font).
4. The page-break structure survives (11 `#pagebreak()` calls between the twelve
   month pages), so the layout still reports **12 pages** and `document.pages.len()`
   returns 12.
5. `typst_pdf::pdf(...)` happily exports a valid PDF. Each page content stream
   — FlateDecode-compressed like every typst-pdf 0.14.2 stream, so visible only
   after inflation — contains only a footer **artifact marker**
   (`/Artifact … BDC EMC`) and no text-drawing operators.
6. The result: a structurally valid, 12-page, byte-for-byte well-formed PDF with
   empty frames → blank pages.

### Why the existing tests did not catch it

`crates/writers/src/pdf.rs` tests assert only that:

- the bytes start with `%PDF-` and end with `%%EOF` (`render_pdf_has_magic_…`);
- `rendered.pages == 12` (layout page count);
- the atomic write leaves the file and a clean temp dir.

**All of these pass on a blank PDF.** No test ever asserts that the rendered
pages contain actual visible content (glyphs / text-showing operators / a
non-empty font book). Additionally, `TypstPdf::compile` discards `warned.warnings`
entirely and only surfaces *errors*, so the telling `unknown font family`
warning was never printed anywhere.

### Why the fix is blind to the rest of the pipeline

The PDF *model* (`PdfDocument`) and `build_pdf_document` are correct — the
fixture yields two entries and the frame dump after the fix shows the expected
title, month, day and entry text. The defect is confined to the renderer's font
supply (`PdfWorld::new` + the dependency feature graph).

## 4. The Fix (verified)

Enable the `fonts` feature on the `typst-assets` workspace dependency so the
bundled fonts are compiled in:

```toml
# Cargo.toml (workspace)
typst-assets = { version = "=0.14.2", features = ["fonts"] }
```

This is the deliberate decision already documented (D2 in
`docs/research/gramps-events-architecture.md`: *"Fonts come from `typst-assets`
(bundled Libertinus), so PDF output is deterministic with no system font
lookup"*) — the feature flag was simply missed at implementation time.

**Verification performed during this investigation** (with the feature enabled
and the temporary diagnostics test):

- Compile warnings disappear (`unknown font family` gone).
- The first page frame now contains real `Text` elements: `"Gramps Events —
  Anniversary Calendar"`, `"January"`, `"14"`, `"Alice — Birth (2020-01-14) ·
  6 years"`, and the footer `"Page 1"`.
- End-to-end CLI export renders all 12 pages with a non-blank pixel count and
  an extractable `pdftotext` text layer.
- Full workspace test suite: **311 tests pass** — the current 304-test suite
  plus the temporary diagnostics test and the planned hardening/regression
  tests.

### Trade-off

`typst-assets` bundles 9.0 MB of font files (Libertinus Serif family, NewCM
math, DejaVuSansMono). Enabling `fonts` embeds that data into the binary.
Relative to the current debug build (~677 MB) that is ~1.5%; a release build
grows roughly 9–13 MB (compressed font data). This is the accepted cost for
deterministic, zero-runtime-dependency output and matches architecture decision
D2.

## 5. Hardening Scope (also planned)

The minimal one-line fix prevents the reported bug, but two gaps allowed it to
escape silently. The plan includes closing both so a blank-PDF regression can
never ship undetected again.

### 5.1 Fail fast on an empty font book

`PdfWorld::new` should refuse to build a world with zero fonts. This converts
the silent blank-output failure into a loud, actionable error at export time.

- Change `PdfWorld::new` to return `Result<Self, WriterError>` and propagate it
  from `TypstPdf::compile` with `?`; an empty `FontBook`/`fonts` vector maps to
  `WriterError::Pdf("no fonts loaded — the typst-assets `fonts` feature is
  disabled")`.
- Test seam: the fail-fast test lives in the same module (`#[cfg(test)] mod
  tests`, with private-field access to `PdfWorld`), so the failure path is
  exercised by constructing `PdfWorld` directly with an empty `fonts` vector —
  the guard's branch is otherwise unreachable once fonts are bundled.
- This is a genuine invariant: a report always contains text (title, month
  names, entries), so an empty font book can never yield valid output.

### 5.2 Surface typst warnings

`TypstPdf::compile` currently routes only *errors* to
`WriterError::Pdf`; `warned.warnings` is dropped. Warnings are how missing
fonts / unresolvable families currently present. Options (pick the lighter one):

- **A (minimum):** collect `warned.warnings` and append them to the returned
  `RenderedPdf` (new field, e.g. `warnings: Vec<String>`), so callers/tests can
  inspect them.
- **B (richer):** surface warnings through `WriterError` once warnings exist,
  or `eprintln!` them in `render`. Choose with the caveat that warnings should
  not hard-fail normal output (typst emits benign warnings in normal docs).

Recommend **A** (store warnings on `RenderedPdf`) plus a shared
`format_diagnostics`-style renderer, keeping the error path unchanged.

### 5.3 Regression tests that assert real content

Add tests that fail if the rendered PDF is blank:

1. **Font book invariant test** — construct a `PdfWorld` (or the internal
   `FontBook`) and assert it is non-empty; assert `"Libertinus Serif"` family
   resolves from the book. Directly guards the dependency-feature regression.
2. **Compiled-frames content test** — after compiling a known `PdfDocument`,
   walk the laid-out pages and assert the frames contain `Text` items (e.g. the
   title and at least one entry string). Catches "pages exist but are empty".
3. **End-to-end content-stream test** — render to bytes and assert the PDF is
   not blank by checking for evidence of drawing:
   - **Raw-byte search for `/Font` / `FontFile`** — page resource dictionaries
     and font objects are written uncompressed, so `/Font` is a clean
     discriminator (0 hits on the blank export, 26 on the fixed one) with no
     extra dependencies.
   - **Do not scan raw bytes for text operators** (`Tj`/`TJ`, `BT … ET`):
     typst-pdf 0.14.2 compresses every content stream with FlateDecode (krilla
     `compress_content_streams: true`), so a raw search returns 0 even on a
     *correct* PDF. If the operator-level signal is wanted, inflate the streams
     first with `flate2` (already a pinned workspace dependency) and search the
     decompressed bytes.
4. **Warning assertions on `RenderedPdf`** — assert the documented doc compiles
   with no `unknown font family` warning once fonts are bundled. This lands in
   step 4, once `RenderedPdf.warnings` exists.

Because the CLI (`run::report`) and web (`handlers::export`) both delegate to
`TypstPdf.compile`, a single writer-level content test covers both; step 3 adds
one mandatory integration check in `crates/web/tests/api.rs` asserting the
exported PDF is not blank (raw `/Font` probe on the response bytes), locking
the web route to the same guarantee.

## 6. Implementation Steps (commit-by-commit)

Follow the project's incremental-development workflow: one small logical unit
per commit, each with passing tests.

| # | Commit message | Logical unit | Key deliverables | Tests |
| --- | --- | --- | --- | --- |
| 1 | `fix: enable bundled fonts for the typst pdf renderer` | Dependency change | Enable `features = ["fonts"]` on workspace `typst-assets`; update the affected `#` comment in `Cargo.toml` | New unit test: the font book is non-empty and `Libertinus Serif` resolves from it |
| 2 | `fix: fail fast on an empty font book in the pdf renderer` | Invariant guard | `PdfWorld::new` returns `Result<Self, WriterError>`; `TypstPdf::compile` propagates with `?`; empty `FontBook`/`fonts` vector maps to `WriterError::Pdf` (see §5.1) | Fail-fast test (empty world → `WriterError::Pdf`) |
| 3 | `test: assert typed pdf pages contain text` | Regression tests | Compiled-frames content test (title + at least one entry `Text` in page 1); end-to-end content test asserting a raw `/Font` resource ref (per §5.3 test 3); mandatory web check in `crates/web/tests/api.rs`: the exported PDF is not blank | Unit + integration |
| 4 | `feat: surface typst compile warnings on RenderedPdf` | Warning surfacing | Add `warnings: Vec<String>` to `RenderedPdf`, populate from `warned.warnings` in `compile`; update construction sites/`PartialEq`/tests | Unit test: a doc that triggers a warning exposes it on `RenderedPdf`; golden docs yield no `unknown font family` warning for the bundled default (§5.3 test 4) |
| 5 | `docs: document the pdf fonts feature fix` | Docs | Note the `fonts` feature requirement and its ~9 MB cost in `docs/ARCHITECTURE.md` (writers crate / PDF section) and the plan link; keep the existing D2 text accurate; refresh the now-stale §11 sentence claiming askama templates and vendored static assets are "the only embedded non-code files" — the bundled fonts become embedded binary data | — |

Step 5 is small but valuable: it records *why* the feature flag is mandatory so
a future reader does not "simplify" `typst-assets` back to a featureless pin and
reintroduce the bug, and so binary-size reviewers understand the ~9 MB.

## 7. Verification / Acceptance

1. `cargo test --workspace` → all pass (the current 304-test suite plus the new regression tests).
2. CLI export: `report --format pdf` on `tests/fixtures/data.gramps`; confirm
   `pdftoppm` renders **all 12 pages non-blank** and `pdftotext` extracts the
   title, month names, day numbers and entry lines (e.g. `Mark Hairball — Birth
   (1946-01-22) · 84 years`).
3. Web UI export of the same file at `GET /api/export?format=pdf` produces an
   identical non-blank PDF (pixel check on page 1).
4. Repeat the root-cause probes from §1: `pdftotext` non-empty text layer,
   `pdftoppm` non-zero pixel counts, no `unknown font family` warnings.
5. `cargo clippy --workspace --all-targets` clean for the touched crates.
6. Release binary contains the bundled fonts (sanity: `du` roughly +9–13 MB).

## 8. Risks / Notes

- **Font data size**: ~9 MB embedded; accepted per decision (2). If it ever
  becomes unacceptable, the documented alternative is subsetting the embedded
  fonts to only the glyphs the report uses — *not* system-font lookup, which
  breaks the D2 determinism guarantee.
- **Feature unification**: `fonts` is additive and nothing else toggles it, so
  there is no feature-conflict risk; `cargo tree -e features -p writers` should
  show `typst-assets feature "fonts"`.
- **Warning-surfacing scope**: keep warnings non-fatal (typst emits benign
  warnings in ordinary docs); only the *empty-font-book* condition is a hard
  error. This preserves round-trip behavior while eliminating silent blindness.
- **No model/pipeline changes**: `PdfDocument`, `build_pdf_document`, CLI and
  web handlers are all correct; the fix is confined to the renderer's font
  supply and the dependency graph.

## 9. Out of Scope

- Switching the PDF engine away from Typst, or to system-font rendering.
- Adding new PDF layouts/content beyond the existing twelve-page calendar shape.
- Optimizing binary size via font subsetting (flagged as a future option only).
