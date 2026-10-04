# Fixtures

Committed test data for the workspace's unit, snapshot and integration
tests. All fixtures are **read-only inputs** — tests never write here.

## Datum fixtures

| File | Purpose | Source |
| --- | --- | --- |
| `data.gramps` | The primary fixture: 5 people, 6 events, 3 families, 4 places — every section the v1 pipeline consumes. | A copy of the example file at `/home/tr/Documents/gramps_examples/data.gramps` (Gramps XML 1.7.1) |
| `dates.gramps` | Every date form: `dateval` (before/after/about, quality, partial, BC, non-Gregorian), `daterange`, `datespan`, `datestr`. | Crafted by hand for this project |
| `edge-cases.gramps` | Privacy flags, hostile-but-well-formed text (escaping regression), unknown elements/attributes/`future-*` sections, an unversioned header. | Crafted by hand for this project |
| `families.gramps` | Family events (marriage/divorce), multi-role eventrefs, place hierarchy. | Crafted by hand for this project |

## Empty / error fixtures (added in the hardening milestone)

| File | Expected behavior |
| --- | --- |
| `empty.gramps` | 0 bytes → `GrampsXmlError::EmptyInput` |
| `garbage.gramps` | Plain text, no XML markup and no recognized container magic → `GrampsXmlError::UnknownContainer` with a byte preview |
| `empty-database.gramps` | Well-formed `<database>` with an empty body → parses successfully with 0 events (the CLI reports "0 events" rather than erroring) |
| `warnings.gramps` | Parses successfully; two records warn-and-skip (bad `priority` on a tag, bad `gender` on a person) so `Database::warnings` is non-empty |
| `malformed-dates.gramps` | Parses successfully; three events skip for malformed dates (reversed `daterange`, reversed `datespan`, invalid `dateval`) so `Database::date_issues` is non-empty and `report` writes `{prefix}.errors.json` |
| `containers/data.gramps.gz` | The `data.gramps` fixture gzipped → parses to the identical database |
| `containers/tree.gramps.zip` | A zip archive whose only member is `data.gramps` (a Gramps "saved tree") → parses to the identical database |
| `containers/truncated.gramps.gz` | The first half of `data.gramps.gz` → `GrampsXmlError::GzipDecode` (truncated stream) |
| `containers/zip-no-data.gramps.zip` | A zip archive whose member is `other.txt`, not `data.gramps` → `GrampsXmlError::ZipMissingMember` |

## The large-file benchmark fixture

A ~100k-event `.gramps` file is a generated artifact, not committed: the
`benchgen` crate produces it on demand (see `crates/benchgen/`, and
`crates/cli/tests/bench_large.rs` for the time/memory budget it is
asserted against).

## Attribution

The fixture family mirrors the **Gramps XML schema** as published in
`grampsxml.dtd` (Gramps XML 1.7.1 / 1.7.2), and `data.gramps` originates
from the Gramps project's own example data. Gramps is licensed under the
GNU GPL v2-or-later (https://gramps-project.org). The XML files here were
written or adapted by this project's authors; no Gramps source code is
copied — only the document structure (section/record elements and
attributes) that the DTD defines, which is the interchange format users
share.