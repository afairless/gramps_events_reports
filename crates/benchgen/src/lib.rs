//! benchgen — deterministic generator for the large-file benchmark fixture.
//!
//! The hardening milestone (plan §12 step 14, §11 "generated large-file
//! smoke fixture (~100k events)") commits **no** multi-megabyte XML file:
//! instead this crate produces the document on demand, byte-for-byte
//! reproducibly (a fixed-seed [SplitMix64], no external generator), and the
//! CLI integration test `crates/cli/tests/bench_large.rs` asserts a sanity
//! time/memory budget against it (performance proportionality).
//!
//! The generated document mirrors the `grampsxml.dtd` structure the small
//! committed fixtures exercise feature-by-feature: twin-pronged it only
//! needs to be *large and parseable* — every event carries a handle, a
//! type, a date of one of the four interchangeable forms, a linked person
//! (or an orphan), a deterministic fraction of private records, place
//! references and family events, so the pipeline's resolution, filtering
//! and view/writer stages all see realistic-shaped input.

use std::fmt::Write;

/// Number of events the default benchmark fixture ships with — the plan's
/// "~100k events".
pub const DEFAULT_EVENT_COUNT: usize = 100_000;

/// Shape knobs for the generated document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BenchOptions {
    /// Number of `<event>` records in the document.
    pub events: usize,
    /// SplitMix64 seed. The fixture is identical for a given `(events, seed)`.
    pub seed: u64,
}

impl Default for BenchOptions {
    fn default() -> Self {
        Self {
            events: DEFAULT_EVENT_COUNT,
            seed: 0x9e37_79b9_7f4a_7c15,
        }
    }
}

/// Event types mirrored from the Gramps default taxonomy plus one
/// user-defined type, so `inspect`'s custom-type enumeration (plan §11.1,
/// acceptance 2) sees non-default labels in the large file too.
const EVENT_TYPES: &[&str] = &[
    "Birth",
    "Death",
    "Marriage",
    "Census",
    "Residence",
    "Custom",
];

/// Deterministic name pools — enough variety to exercise name parsing and
/// subject rendering without inflating the document.
const FIRST_NAMES: &[&str] = &[
    "Ada",
    "Grace",
    "Alan",
    "Katherine",
    "Leslie",
    "Barbara",
    "Dorothy",
    "Margaret",
    "Dennis",
    "Edsger",
    "Donald",
    "Claude",
];
const SURNAMES: &[&str] = &[
    "Lovelace", "Hopper", "Turing", "Johnson", "Lamport", "Liskov", "Vaughan", "Hamilton",
    "Ritchie", "Knuth", "McCarthy", "Shannon",
];
/// Fixed place pool — every ~3rd event references one, so place-path
/// resolution in event-core sees repeated, cacheable values.
const PLACES: &[&str] = &[
    "London, England",
    "Paris, France",
    "Berlin, Germany",
    "Cambridge, Massachusetts",
    "Palo Alto, California",
    "Oslo, Norway",
    "Prague, Czechia",
    "Kyoto, Japan",
    "Buenos Aires, Argentina",
    "Cairo, Egypt",
    "Delhi, India",
    "Sydney, Australia",
];
/// Tag pool — a small `tags` section is emitted and a fraction of events
/// reference it via `tagref` (tolerated, forward-compatible elements).
const TAGS: &[&str] = &["Bookmark", "To Check", "Complete"];

/// Deterministic small PRNG (SplitMix64, public domain). Chosen over a
/// crates.io RNG so the fixture generator has **zero dependencies** and
/// produces identical output on every machine — the crux of a benchmark.
struct Sm64(u64);

impl Sm64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// `0..n` (uniform-bias is irrelevant for a fixture's shape).
    fn range(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// `true` roughly `pct` percent of the time.
    fn chance(&mut self, pct: u64) -> bool {
        self.next_u64() % 100 < pct
    }
}

/// Generate the complete `.gramps` XML document for `opts`.
///
/// Shape per event (all deterministic):
///
/// - 55% exact `dateval`, 8% year-only, 7% month-only, 10% `daterange`,
///   10% modifier `dateval` (`before`/`after`/`about`), 5% `datestr`,
///   5% exact dates spilled onto a `datespan`;
/// - 2% flagged `priv="1"`;
/// - 30% carry a `place hlink` into one of 12 fixed places;
/// - every 13th event is an orphan (no person — the subject "—" path);
/// - every 23rd event is a Marriage referenced by a family (couple path)
///   and by its two people with the `Family` role;
/// - every other event is linked to exactly one person with `Primary`.
pub fn generate(opts: BenchOptions) -> String {
    let mut rng = Sm64(opts.seed);
    let mut out = String::with_capacity(opts.events * 340 + 4096);

    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(
        "<database xmlns=\"http://gramps-project.org/xml/1.7.1/\">\n\
         <header>\n  <created date=\"2026-09-29\" version=\"5.1.6\"/>\n  \
         <researcher><resname>benchmark</resname></researcher>\n</header>\n",
    );

    // ---- tags ----
    out.push_str("  <tags>\n");
    for (i, tag) in TAGS.iter().enumerate() {
        let _ = writeln!(
            out,
            "    <tag handle=\"_bench_t{i}\" change=\"{}\" priority=\"{i}\" name=\"{tag}\" color=\"#000000\"/>",
            1000 + i as u64
        );
    }
    out.push_str("  </tags>\n");

    // ---- places ----
    out.push_str("  <places>\n");
    for (i, place) in PLACES.iter().enumerate() {
        let _ = writeln!(
            out,
            "    <placeobj handle=\"_bench_pl{i}\" change=\"100\" id=\"P0000{i}\">\n\
             \x20     <ptype>City</ptype>\n\
             \x20     <pname value=\"{place}\"/>\n    </placeobj>"
        );
    }
    out.push_str("  </places>\n");

    // ---- events ----
    out.push_str("  <events>\n");
    for i in 0..opts.events {
        write_event(&mut out, &mut rng, i);
    }
    out.push_str("  </events>\n");

    // ---- people ----
    out.push_str("  <people>\n");
    for i in 0..opts.events {
        if is_orphan(i) {
            continue;
        }
        write_person(&mut out, &mut rng, i);
    }
    out.push_str("  </people>\n");

    // ---- families (one per Marriage event) ----
    out.push_str("  <families>\n");
    for i in 0..opts.events {
        if is_marriage(i) {
            write_family(&mut out, i, opts.events);
        }
    }
    out.push_str("  </families>\n");

    out.push_str("</database>\n");
    out
}

/// Every 13th event has no linked person — the orphan "—" subject path.
fn is_orphan(i: usize) -> bool {
    i % 13 == 6
}

/// Every 23rd event is a Marriage owned by a family (couple subject path).
fn is_marriage(i: usize) -> bool {
    i % 23 == 11
}

/// Deterministic event id from its index: `E0000000`, `E0000001`, ...
fn event_id(i: usize) -> String {
    format!("E{i:07}")
}

fn write_event(out: &mut String, rng: &mut Sm64, i: usize) {
    let event_type = if is_marriage(i) {
        "Marriage"
    } else {
        EVENT_TYPES[rng.range(EVENT_TYPES.len())]
    };

    let _ = write!(
        out,
        "    <event handle=\"_bench_e{i}\" change=\"{}\" id=\"{}\"",
        1786566326 + i as u64,
        event_id(i)
    );
    if rng.chance(2) {
        out.push_str(" priv=\"1\"");
    }
    out.push_str(">\n");
    let _ = writeln!(out, "      <type>{event_type}</type>");
    write_date(out, rng);
    if rng.chance(30) {
        let place = rng.range(PLACES.len());
        let _ = writeln!(out, "      <place hlink=\"_bench_pl{place}\"/>");
    }
    if rng.chance(8) {
        let tag = rng.range(TAGS.len());
        let _ = writeln!(out, "      <tagref hlink=\"_bench_t{tag}\"/>");
    }
    out.push_str("    </event>\n");
}

/// Emit one of the four interchangeable date elements (plan §7.1) for an
/// event. All dates fall in 1500..=2024 so anniversary math and elapsed
/// columns stay in range for any reference year.
fn write_date(out: &mut String, rng: &mut Sm64) {
    let year = 1500 + rng.range(525); // 1500..=2024
    let month = 1 + rng.range(12);
    let roll = rng.range(100);
    match roll {
        0..=54 => {
            let day = 1 + rng.range(28); // never Feb 29 / short-month invalid
            let _ = writeln!(
                out,
                "      <dateval val=\"{year:04}-{month:02}-{day:02}\"/>"
            );
        }
        55..=62 => {
            let _ = writeln!(out, "      <dateval val=\"{year:04}\"/>");
        }
        63..=69 => {
            let _ = writeln!(out, "      <dateval val=\"{year:04}-{month:02}\"/>");
        }
        70..=79 => {
            let stop_year = year + 1 + rng.range(9);
            let stop_month = 1 + rng.range(12);
            let _ = writeln!(
                out,
                "      <daterange start=\"{year:04}-{month:02}-{day:02}\" stop=\"{stop_year:04}-{stop_month:02}-{stop_month:02}\"/>",
                day = 1 + rng.range(28)
            );
        }
        80..=89 => {
            let kind = ["before", "after", "about"][rng.range(3)];
            let _ = writeln!(
                out,
                "      <dateval val=\"{year:04}-{month:02}\" type=\"{kind}\"/>"
            );
        }
        90..=94 => {
            let _ = writeln!(
                out,
                "      <datestr val=\"circa {year:04}, season of {month}\"/>"
            );
        }
        _ => {
            // Exact dates spilled onto a `datespan` (rare, exercises the
            // compound start/stop path at scale).
            let stop_year = year + 1 + rng.range(3);
            let day = 1 + rng.range(28);
            let _ = writeln!(
                out,
                "      <datespan start=\"{year:04}-{month:02}-{day:02}\" stop=\"{stop_year:04}-{month:02}-{day:02}\"/>"
            );
        }
    }
}

/// One person per non-orphan, non-marriage event (role `Primary`); the two
/// Marriage partners each reference the marriage event with the `Family`
/// role, matching Gramps' real export shape.
fn write_person(out: &mut String, rng: &mut Sm64, i: usize) {
    let gender = ["M", "F", "U"][rng.range(3)];
    let first = FIRST_NAMES[rng.range(FIRST_NAMES.len())];
    let surname = SURNAMES[rng.range(SURNAMES.len())];
    let _ = write!(
        out,
        "    <person handle=\"_bench_p{i}\" change=\"{}\" id=\"I{i:07}\">\n\
         \x20     <gender>{gender}</gender>\n\
         \x20     <name type=\"Birth Name\">\n\
         \x20       <first>{first}</first>\n\
         \x20       <surname>{surname}</surname>\n\
         \x20     </name>\n",
        1787158839 + i as u64
    );
    let role = if is_marriage(i) { "Family" } else { "Primary" };
    let _ = writeln!(
        out,
        "      <eventref hlink=\"_bench_e{i}\" role=\"{role}\"/>"
    );
    out.push_str("    </person>\n");
}

/// The family owning a Marriage event: father + mother plus the eventref,
/// so event-core's couple resolution has a real couple to resolve. The
/// partner indices are derived deterministically from the event index, but
/// always resolve to *existing* people: non-orphan, in-range, distinct
/// from the event's own person and from each other.
///
/// The mother search prefers the first of the four partner candidates, and
/// falls back to a full bounded scan only for degenerate sizes the
/// generator never emits (the invariant — plenty of non-orphan residues —
/// holds for every real benchmark size, so the fallback never fires there).
fn write_family(out: &mut String, i: usize, events: usize) {
    let father = partner_idx(i, events, 1, None);
    let mother = (2..5)
        .map(|offset| partner_idx(i, events, offset, Some(father)))
        .find(|&m| m != father)
        .unwrap_or_else(|| {
            // Bounded scan over every residue. For events ≥ 4 this always
            // finds a valid partner distinct from `father` (and `i`); the
            // `events` default is orders of magnitude larger, so the
            // fallback value below is unreachable in practice. It exists
            // to keep generation panic-free on any input.
            (1..events)
                .map(|offset| partner_idx(i, events, offset, Some(father)))
                .find(|&m| m != father)
                .unwrap_or(father)
        });
    let _ = write!(
        out,
        "    <family handle=\"_bench_f{i}\" change=\"{}\" id=\"F{i:06}\">\n\
         \x20     <rel type=\"Marriage\"/>\n\
         \x20     <father hlink=\"_bench_p{father}\"/>\n\
         \x20     <mother hlink=\"_bench_p{mother}\"/>\n\
         \x20     <eventref hlink=\"_bench_e{i}\" role=\"Family\"/>\n\
         \x20   </family>\n",
        1786566326 + i as u64
    );
}

/// First index `>= i + offset` (wrapping mod `events`) whose event is not
/// an orphan and (when asked) differs from `exclude`. Guaranteed to
/// terminate: non-orphan indices repeat every 13 positions.
fn partner_idx(i: usize, events: usize, offset: usize, exclude: Option<usize>) -> usize {
    let mut j = i + offset;
    loop {
        let jj = j % events;
        if !is_orphan(jj) && jj != i && exclude != Some(jj) {
            return jj;
        }
        j += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{BenchOptions, DEFAULT_EVENT_COUNT, generate, is_marriage, is_orphan};
    use gramps_xml::parse_database;

    /// The full default-size document parses through the real pipeline and
    /// yields the expected record counts — the "large-file smoke fixture"
    /// acceptance (plan §11): usable end to end, not just well-formed XML.
    #[test]
    fn default_fixture_parses_to_expected_counts() {
        let xml = generate(BenchOptions::default());
        let db = parse_database(xml.as_bytes()).expect("parse the generated document");
        assert_eq!(db.events.len(), DEFAULT_EVENT_COUNT);

        let marriages = (0..DEFAULT_EVENT_COUNT).filter(|i| is_marriage(*i)).count();
        let orphans = (0..DEFAULT_EVENT_COUNT).filter(|i| is_orphan(*i)).count();
        // One person per non-orphan event index (marriage indices carry one
        // of the two partners); families link two further *existing* people.
        let expected_people = DEFAULT_EVENT_COUNT - orphans;
        assert_eq!(db.people.len(), expected_people);
        let mut seen_handles = std::collections::HashSet::new();
        assert!(
            db.people
                .iter()
                .all(|p| seen_handles.insert(p.handle.as_str())),
            "person handles must be unique"
        );
        assert!(marriages * 2 > 0, "marriages present");
        assert_eq!(db.families.len(), marriages);
        assert_eq!(db.places.len(), super::PLACES.len());
        assert_eq!(db.tags.len(), super::TAGS.len());
        assert!(
            db.warnings.is_empty(),
            "generator produced defective records: {:?}",
            db.warnings
        );
    }

    /// Byte-for-byte reproducibility — the budget test reruns the fixture
    /// across runs and machines, so generation must be deterministic.
    #[test]
    fn generation_is_deterministic() {
        let a = generate(BenchOptions::default());
        let b = generate(BenchOptions::default());
        assert_eq!(a, b);
        let c = generate(BenchOptions {
            seed: 7,
            ..BenchOptions::default()
        });
        assert_ne!(a, c, "a different seed must change the document");
    }

    /// A tiny count keeps the shape checks cheap while exercising the same
    /// code paths.
    #[test]
    fn small_fixture_has_no_warnings_and_some_private_events() {
        let xml = generate(BenchOptions {
            events: 3000,
            ..BenchOptions::default()
        });
        let db = parse_database(xml.as_bytes()).unwrap();
        assert!(db.warnings.is_empty());
        let private = db.events.iter().filter(|e| e.private).count();
        assert!(private > 0, "expected a few private events in 3000");
        let custom = db
            .events
            .iter()
            .filter(|e| e.event_type == "Custom")
            .count();
        assert!(custom > 0, "expected custom-typed events");
    }
}
