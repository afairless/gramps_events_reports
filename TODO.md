# Implementation Plan: Fix Blank PDF Export (bundled fonts for Typst)

Source: `docs/research/fix-blank-pdf-export.md`

Fixes the `gramps-events` PDF export producing **valid but blank** PDFs (both
web `GET /api/export?format=pdf` and CLI `report --format pdf`, which share
`TypstPdf` in `crates/writers/src/pdf.rs`). Root cause confirmed: the workspace
pins `typst-assets = "=0.14.2"` **without** its `fonts` feature, so
`typst_assets::fonts()` returns an empty iterator, `PdfWorld` builds an empty
`FontBook`, and every text glyph is silently dropped at layout while the
12-page structure survives. Confirmed decisions (2026-10-01): deliver plan
only, bundle typst's fonts (~9 MB growth) rather than system-font lookup, and
include hardening so a blank-PDF regression can never ship silently again.
Existing plan for the original project is superseded by this one.

## Steps

| # | Commit message | Logical unit | Key deliverables | Tests |
| --- | --- | --- | --- | --- |
| 1 | fix: enable bundled fonts for the typst pdf renderer | Dependency change | `Cargo.toml` (workspace): change `typst-assets = "=0.14.2"` to `{ version = "=0.14.2", features = ["fonts"] }`, update the affected `#` comment | Unit |
| 2 | fix: fail fast on an empty font book in the pdf renderer | Invariant guard | `crates/writers/src/pdf.rs` — `PdfWorld::new` returns `Result<Self, WriterError>`; empty `FontBook`/`fonts` vector maps to `WriterError::Pdf("no fonts loaded — the typst-assets `fonts` feature is disabled")`; `TypstPdf::compile` propagates with `?` | Unit |
| 3 | test: assert typed pdf pages contain text | Regression tests | `crates/writers/src/pdf.rs` — compiled-frames content test (page 1 contains title + at least one entry `Text`); end-to-end content test asserting a raw `/Font` resource ref (raw-byte probe, not `Tj`/`TJ` operator scan); `crates/web/tests/api.rs` — mandatory web check that the exported PDF is not blank (raw `/Font` probe on response bytes) | Unit, Integration |
| 4 | feat: surface typst compile warnings on RenderedPdf | Warning surfacing | `crates/writers/src/pdf.rs` — add `warnings: Vec<String>` to `RenderedPdf`, populate from `warned.warnings` in `compile`; update construction sites (`render`), `PartialEq`/`Eq` derive, and affected tests | Unit |
| 5 | docs: document the pdf fonts feature fix | Docs | `docs/ARCHITECTURE.md` — note the `fonts` feature requirement and ~9 MB cost in the writers/PDF section, link the fix plan, keep D2 text accurate, refresh the now-stale §11 sentence claiming askama/static assets are the only embedded non-code files (bundled fonts become embedded binary data) | — |

## Notes

- **Ordering rationale**: step 1 flips the dependency feature that the empty-book
  guard (step 2), content tests (step 3) and warning surfacing (step 4) all
  depend on. Steps 1–2 contain the fix; 3–4 lock the regression out; 5 documents
  why the flag is mandatory.
- **Warning surfacing chooses option A** (store on `RenderedPdf`, keep error
  path non-fatal) per plan §5.2 recommendation — typst emits benign warnings in
  ordinary docs, so only the empty-font-book condition is a hard error.
- **Content test discriminator**: search raw bytes for `/Font` / `FontFile`
  (written uncompressed); do **not** scan for text operators `Tj`/`TJ`/`BT…ET`
  because typst-pdf 0.14.2 FlateDecode-compresses every content stream.
- Step 3's web check runs only once `crates/web` builds against the fixed
  renderer; step 4's golden warning test asserts the bundled default compiles
  with no `unknown font family` warning once `RenderedPdf.warnings` exists.
