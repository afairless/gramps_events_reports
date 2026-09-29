//! `ReportOptions` — the configuration every resolution run is filtered
//! against (plan §7.3), plus the leap-day policy (decision D6).
//!
//! All filters are optional at the boundary: `None`/`false` fields mean
//! "no restriction". [`ReportOptions::default`] pins the reference year to
//! the current year (plan §8.4) and applies the plan's defaults — every
//! event type admitted, orphans visible (D7), private records hidden
//! (§8.7), Feb 29 anchors folded (D6); [`ReportOptions::with_reference_year`]
//! builds the same defaults with a deterministic reference year (tests, CLI
//! `--reference-year`).

use std::collections::HashSet;

use jiff::civil::Date;

/// How Feb 29 anniversary anchors behave (decision D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeapDayPolicy {
    /// Fold a Feb 29 anchor to Feb 28 in non-leap reference years and label
    /// the row as a Feb 29 event — every event stays visible every year and
    /// the fold stays visible to the reader.
    FoldToFeb28,
    /// Never fold: keep the Feb 29 anchor in every reference year.
    Keep,
}

impl LeapDayPolicy {
    /// Is `year` a leap year in the proleptic Gregorian calendar?
    pub fn is_leap_year(year: i32) -> bool {
        match year % 4 {
            0 => year % 100 != 0 || year % 400 == 0,
            _ => false,
        }
    }

    /// Apply the policy to an anniversary anchor `(month, day)` for
    /// `reference_year`.
    ///
    /// Returns `(effective_anchor, folded)`: under `FoldToFeb28` a Feb 29
    /// anchor folds to `(2, 28)` — and reports the fold — exactly when the
    /// reference year is not a leap year; every other anchor (and the
    /// `Keep` policy) passes through untouched with `folded = false`.
    pub fn fold_anchor(&self, anchor: (u32, u32), reference_year: i32) -> ((u32, u32), bool) {
        match *self {
            LeapDayPolicy::FoldToFeb28 => {
                if anchor == (2, 29) && !Self::is_leap_year(reference_year) {
                    ((2, 28), true)
                } else {
                    (anchor, false)
                }
            }
            LeapDayPolicy::Keep => (anchor, false),
        }
    }
}

/// The default policy — fold to Feb 28, labeled (D6).
impl Default for LeapDayPolicy {
    fn default() -> LeapDayPolicy {
        LeapDayPolicy::FoldToFeb28
    }
}

/// The run configuration for event-core resolution and filtering
/// (plan §7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportOptions {
    /// The "year of report" every elapsed value is measured against
    /// (plan §8.4) — the Gramps report-year equivalent.
    pub reference_year: i32,
    /// `None` = every event type; `Some(set)` = only the listed types
    /// (whitelist).
    pub include_types: Option<HashSet<String>>,
    /// Types always rejected — a type present in both sets is excluded
    /// (plan §8 rule 14: exclusion wins).
    pub exclude_types: HashSet<String>,
    /// `None` = every subject; `Some(set)` = only events whose primary
    /// subject's Gramps id **or handle** is in the set.
    pub include_people: Option<HashSet<String>>,
    /// Inclusive window on the event's normalized (Gregorian) start date;
    /// when set, events without a normalizable date are excluded.
    pub date_range: Option<(Date, Date)>,
    /// Keep only events whose subjects are all presumably alive (plan §8
    /// rule 15 — the `probably_alive` port).
    pub living_only: bool,
    /// Keep `priv="1"` records (default: excluded — plan §8.7).
    pub include_private: bool,
    /// Feb 29 anchor behavior (decision D6).
    pub leap_day: LeapDayPolicy,
    /// Collapse identical (type, subject, month-day) rows to one. The flag
    /// rides on the options object; the collapse itself is applied when
    /// views expand events into rows (milestone 8, plan §8.6).
    pub dedupe_same: bool,
    /// Show orphan events with subject `"—"` (decision D7; default true).
    pub show_orphans: bool,
}

impl ReportOptions {
    /// The plan defaults with a deterministic `reference_year` — the
    /// deterministic form tests and the CLI's `--reference-year` use.
    pub fn with_reference_year(reference_year: i32) -> ReportOptions {
        ReportOptions {
            reference_year,
            include_types: None,
            exclude_types: HashSet::new(),
            include_people: None,
            date_range: None,
            living_only: false,
            include_private: false,
            leap_day: LeapDayPolicy::default(),
            dedupe_same: false,
            show_orphans: true,
        }
    }

    /// Rule-14 type selection: the whitelist admits a type only when
    /// `include_types` is set and contains it; the exclusion list rejects a
    /// type regardless of the whitelist (exclusion wins).
    pub fn type_allowed(&self, event_type: &str) -> bool {
        if self.exclude_types.contains(event_type) {
            return false;
        }
        match self.include_types.as_ref() {
            None => true,
            Some(allowed) => allowed.contains(event_type),
        }
    }
}

/// The plan defaults with the reference year set to the current year
/// (plan §8.4).
impl Default for ReportOptions {
    fn default() -> ReportOptions {
        ReportOptions::with_reference_year(current_year())
    }
}

/// The current year in the local timezone — [`ReportOptions::default`]'s
/// reference year (plan §8.4).
fn current_year() -> i32 {
    jiff::Zoned::now().date().year() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pins_current_year_and_plan_defaults() {
        let opts = ReportOptions::default();
        assert_eq!(opts.reference_year, current_year());
        assert_eq!(opts.include_types, None);
        assert!(opts.exclude_types.is_empty());
        assert_eq!(opts.include_people, None);
        assert_eq!(opts.date_range, None);
        assert!(!opts.living_only);
        assert!(!opts.include_private);
        assert_eq!(opts.leap_day, LeapDayPolicy::FoldToFeb28);
        assert!(!opts.dedupe_same);
        assert!(opts.show_orphans);

        let opts = ReportOptions::with_reference_year(2026);
        assert_eq!(opts.reference_year, 2026);
        assert!(opts.exclude_types.is_empty());
        assert!(opts.show_orphans);
    }

    #[test]
    fn reference_year_is_deterministic_when_pinned() {
        let opts = ReportOptions::with_reference_year(2026);
        assert_eq!(opts.reference_year, 2026);
        assert!(opts.type_allowed("Birth"));
    }

    #[test]
    fn leap_year_rules_match_the_gregorian_cycle() {
        assert!(LeapDayPolicy::is_leap_year(2000));
        assert!(LeapDayPolicy::is_leap_year(2024));
        assert!(LeapDayPolicy::is_leap_year(1996));
        assert!(!LeapDayPolicy::is_leap_year(1900));
        assert!(!LeapDayPolicy::is_leap_year(2026));
        assert!(!LeapDayPolicy::is_leap_year(2025));
        assert!(!LeapDayPolicy::is_leap_year(2100));
    }

    #[test]
    fn feb_29_folds_only_in_non_leap_years_under_fold_policy() {
        let fold = LeapDayPolicy::FoldToFeb28;
        // non-leap reference year → fold, reported
        assert_eq!(fold.fold_anchor((2, 29), 2026), ((2, 28), true));
        // leap reference year → anchor kept, no fold
        assert_eq!(fold.fold_anchor((2, 29), 2024), ((2, 29), false));
        // leap reference year for a century year
        assert_eq!(fold.fold_anchor((2, 29), 2000), ((2, 29), false));
        // century-boundary non-leap year
        assert_eq!(fold.fold_anchor((2, 29), 1900), ((2, 28), true));
        // non-Feb-29 anchors never fold
        assert_eq!(fold.fold_anchor((3, 1), 2026), ((3, 1), false));
        assert_eq!(fold.fold_anchor((2, 28), 2026), ((2, 28), false));
        assert_eq!(fold.fold_anchor((11, 13), 2026), ((11, 13), false));
    }

    #[test]
    fn keep_policy_never_folds() {
        let keep = LeapDayPolicy::Keep;
        assert_eq!(keep.fold_anchor((2, 29), 2026), ((2, 29), false));
        assert_eq!(keep.fold_anchor((2, 29), 2024), ((2, 29), false));
    }

    #[test]
    fn type_selection_follows_the_whitelist_and_precedence_rules() {
        let mut opts = ReportOptions::with_reference_year(2026);

        // include_types = None → everything allowed
        assert!(opts.type_allowed("Birth"));
        assert!(opts.type_allowed("Actually A Custom Type"));

        // exclusions apply to the "all types" default
        opts.exclude_types = set_of(&["Death"]);
        assert!(opts.type_allowed("Birth"));
        assert!(!opts.type_allowed("Death"));
        assert!(opts.type_allowed("Marriage"));

        // whitelist restricts the universe
        opts.include_types = Some(set_of(&["Birth", "Marriage"]));
        assert!(opts.type_allowed("Birth"));
        assert!(opts.type_allowed("Marriage"));
        assert!(!opts.type_allowed("Death"));
        assert!(!opts.type_allowed("Immigration"));

        // exclusion wins over the whitelist (rule 14)
        opts.exclude_types = set_of(&["Marriage"]);
        assert!(!opts.type_allowed("Marriage"));
        assert!(opts.type_allowed("Birth"));

        // ... including a type that is whitelisted but also excluded
        opts.include_types = Some(set_of(&["Death", "Marriage"]));
        assert!(!opts.type_allowed("Marriage"));
        assert!(opts.type_allowed("Death"));
    }

    /// A `HashSet<String>` from string literals — tests seed the options'
    /// type/person sets this way.
    fn set_of(values: &[&str]) -> HashSet<String> {
        let mut set = HashSet::new();
        for value in values {
            set.insert(value.to_string());
        }
        set
    }
}
