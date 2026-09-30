//! The generated large-file benchmark (plan §12 step 14, §11).
//!
//! No multi-megabyte `.gramps` file is committed; [`benchgen`] produces the
//! ~100k-event fixture on demand, byte-for-byte reproducibly, and this
//! test pushes it through the whole `report` pipeline (parse → resolve →
//! filter → view → writer) to assert **performance proportionality**: a
//! sanity wall-time budget plus a peak-memory budget, so a regression that
//! turns the pipeline quadratic or leaks memory fails the suite instead of
//! lingering for users of large Gramps databases.
//!
//! The budgets are deliberately generous (they catch blow-ups, not
//! micro-deltas) and were calibrated on the dev machine against a debug
//! build — the same profile `cargo test` runs — from the measured numbers,
//! then padded by a comfortable margin.

use std::time::Instant;

use cli::run;
use event_core::ReportOptions;
use writers::Formats;

/// Wall-time budget for generate → parse → `report` (csv+json) on 100k
/// events. Debug profile; calibrated 2026-09-30 at ≈6s → 2.5× margin.
const TIME_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// Peak-RSS budget for the same workload, as a *delta* of the Linux
/// `VmHWM` high-water mark. 1.2 GB — measured ≈450 MB with a deliberate
/// headroom for toolchain/linker variance between machines.
const MEMORY_DELTA_BUDGET_KB: u64 = 1_200_000; // ≈1.2 GiB

/// Peak resident set so far (`/proc/self/status` `VmHWM`, kB) — Linux only;
/// on other platforms the memory half of the budget is skipped (the time
/// half still runs).
fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let rest = line.strip_prefix("VmHWM:")?;
        rest.trim().strip_suffix(" kB")?.trim().parse().ok()
    })
}

/// The full large-file pipeline stays inside its time and memory budgets.
#[test]
fn generated_large_file_report_stays_within_budget() {
    let bench = benchgen::BenchOptions::default();
    let mem_before = peak_rss_kb();
    let start = Instant::now();

    let xml = benchgen::generate(bench);
    let db = gramps_xml::parse_database(xml.as_bytes())
        .expect("the generated 100k-event fixture must parse");
    assert_eq!(
        db.events.len(),
        bench.events,
        "every generated event parses"
    );
    assert!(
        db.warnings.is_empty(),
        "the generator must produce clean input: {:?}",
        db.warnings
    );

    let out_dir = tempfile::tempdir().unwrap();
    let written = run::report(
        &db,
        &ReportOptions::with_reference_year(2030),
        Formats::CSV | Formats::JSON,
        out_dir.path(),
        "bench",
    )
    .expect("report writes the large file set");
    let elapsed = start.elapsed();
    let mem_delta_kb = match (mem_before, peak_rss_kb()) {
        (Some(before), Some(after)) => Some(after.saturating_sub(before)),
        _ => None,
    };

    // Correctness sanity beside the budgets: the writer paths really ran
    // at scale (not, say, short-circuited by a filter or an empty view).
    let csv = std::fs::read_to_string(out_dir.path().join("bench.csv")).unwrap();
    let csv_rows = csv.lines().count().saturating_sub(1); // drop the header
    assert!(csv_rows > bench.events * 9 / 10, "CSV rows: {csv_rows}");
    let json = std::fs::read_to_string(out_dir.path().join("bench.json")).unwrap();
    assert_eq!(written.len(), 2);

    eprintln!(
        "large-file benchmark ({} events): {elapsed:?}, peak-RSS delta {}",
        bench.events,
        mem_delta_kb
            .map(|kb| format!("{kb} kB"))
            .unwrap_or_else(|| "n/a (no /proc)".into())
    );
    eprintln!("  csv rows: {csv_rows}, json bytes: {}", json.len());

    assert!(
        elapsed <= TIME_BUDGET,
        "100k-event report took {elapsed:?}, budget {TIME_BUDGET:?} — \
         performance proportionality regression"
    );
    if let Some(delta_kb) = mem_delta_kb {
        assert!(
            delta_kb <= MEMORY_DELTA_BUDGET_KB,
            "100k-event report grew peak RSS by {delta_kb} kB, budget \
             {MEMORY_DELTA_BUDGET_KB} kB — memory regression"
        );
    } else {
        eprintln!("  (skipping the memory assertion: no /proc/self/status)");
    }
}
