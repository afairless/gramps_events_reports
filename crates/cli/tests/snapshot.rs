//! Snapshot tests of the CLI commands (plan §12, "tests: Snapshot, Unit").
//!
//! `inspect` and `list` output — and the file set a `report --format all`
//! run produces — are locked against committed `tests/snapshots/*.snap`
//! fixtures, so a change to any rendering or wiring shows up as a diff.
//!
//! Regenerate the snapshots after an intentional change:
//!
//! ```sh
//! UPDATE_SNAPSHOTS=1 cargo test -p cli --test snapshot
//! ```
//!
//! Snapshots pin the reference year to a fixed non-leap year (2030) so the
//! elapsed columns and Feb 29 folds are deterministic. Only the fixture at
//! `tests/fixtures/data.gramps` is used; it is what the plan's §11
//! acceptance fixtures exercise.

use std::path::Path;

use cli::run;
use event_core::{ReportOptions, ViewKind};
use writers::Formats;

/// The datum fixture committed at the repository root (plan §11).
const DATA_GRAMPS: &[u8] = include_bytes!("../../../tests/fixtures/data.gramps");

fn db() -> gramps_xml::Database {
    gramps_xml::parse_database(DATA_GRAMPS).unwrap()
}

/// The fixed snapshot reference year — a non-leap year so Feb 29 folds are
/// exercised and elapsed values are stable forever.
const REF: i32 = 2030;

fn opts() -> ReportOptions {
    ReportOptions::with_reference_year(REF)
}

fn snap_dir() -> std::path::PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.join("tests").join("snapshots")
}

/// Assert `actual` equals the committed snapshot `name`, or (when
/// `UPDATE_SNAPSHOTS` is set) write `actual` as the new snapshot.
fn assert_snapshot(name: &str, actual: &str) {
    let dir = snap_dir();
    let path = dir.join(name);
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(&dir).expect("create snapshots dir");
        std::fs::write(&path, actual).unwrap_or_else(|e| panic!("write {path:?}: {e}"));
        return;
    }
    let expected =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing snapshot {path:?}: {e}"));
    assert_eq!(
        actual, expected,
        "snapshot {name} drifted from committed fixture"
    );
}

/// `inspect` enumerates event types with counts — deterministically sorted
/// (count desc, name asc).
#[test]
fn inspect_data_snapshot() {
    assert_snapshot("inspect_data.snap", &run::inspect(&db()));
}

/// `list` text for each of the four views, including `yrcal` (plan §6.5).
#[test]
fn list_data_list_snapshot() {
    assert_snapshot(
        "list_data_list.snap",
        &run::list(&db(), &opts(), ViewKind::List),
    );
}

#[test]
fn list_data_calendar_snapshot() {
    assert_snapshot(
        "list_data_calendar.snap",
        &run::list(&db(), &opts(), ViewKind::Calendar),
    );
}

#[test]
fn list_data_timeline_snapshot() {
    assert_snapshot(
        "list_data_timeline.snap",
        &run::list(&db(), &opts(), ViewKind::Timeline),
    );
}

#[test]
fn list_data_yrcal_snapshot() {
    assert_snapshot(
        "list_data_yrcal.snap",
        &run::list(&db(), &opts(), ViewKind::CalendarWithYears),
    );
}

/// `report --format all` produces the full four-file set — snapshotted as
/// the ordered list of written filenames (plan §11 acceptance 4: "one
/// command produces all four files").
#[test]
fn report_all_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let written = run::report(&db(), &opts(), Formats::ALL, dir.path(), "data").unwrap();
    let names: Vec<String> = written
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    let actual = format!("{}\n", names.join("\n"));
    assert_snapshot("report_all.snap", &actual);
}
