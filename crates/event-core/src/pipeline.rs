//! Derived-value computation and the filtering pipeline (plan §7.3, §8).
//!
//! [`derive`] stamps each resolved event with its elapsed years (§8.4),
//! its anniversary anchor (§8 rules 1/3), the leap-day fold flag (D6) and
//! the primary subject's age at the event (§8.9); [`passes`] applies the
//! [`ReportOptions`](crate::options::ReportOptions) filters in one pass —
//! privacy (§8.7), orphan visibility (D7), the rule-14 type selection, the
//! person filter, the date-range window and living-only (§8 rule 15, a port
//! of Gramps `gramps.gen.utils.alive.probably_alive`).
//!
//! [`collect_events`](crate::resolve::collect_events) runs `derive` then
//! `passes` for every event, so a resolved event list is exactly the
//! filtered, derived stream the views (milestone 8) and writers consume.

use std::collections::HashSet;

use gramps_dates::GrampsDate;
use gramps_xml::model::{Event, Person};

use crate::model::{PersonDisplay, ResolvedEvent};
use crate::options::ReportOptions;
use crate::resolve::HandleIndex;

/// The death-type events that mark a person dead regardless of their date
/// (plan §8 rule 15, mirroring Gramps `probably_alive`).
const DEATH_TYPES: &[&str] = &["Death", "Cremation", "Burial"];

/// The event type naming a person's birth (plan §8 rule 15).
const BIRTH_TYPE: &str = "Birth";

/// The role a person holds on their own birth event.
const PRIMARY_ROLE: &str = "Primary";

/// Gramps' default maximum-age span: a person with a known birth year more
/// than this many years before the report year is presumed dead (plan §8
/// rule 15; Gramps `probably_alive_max_age` defaults to 110).
const MAX_PROBABLE_AGE: i32 = 110;

/// Stamp `event` with its derived values: elapsed years against
/// `opts.reference_year`, the (month, day) anniversary anchor, the
/// leap-day fold flag (D6) and the primary subject's age at the event.
///
/// The anchor stored is the *true* anchor — a Feb 29 date stays `(2, 29)`;
/// [`LeapDayPolicy::fold_anchor`](crate::options::LeapDayPolicy::fold_anchor)
/// applies the D6 fold and `leap_day_folded` records whether it applied
/// in this run.
pub(crate) fn derive(event: &mut ResolvedEvent, index: &HandleIndex, opts: &ReportOptions) {
    event.elapsed_years = event
        .date
        .as_ref()
        .and_then(|date| elapsed_years(date, opts.reference_year));
    event.anniversary = event.date.as_ref().and_then(|date| date.anniversary_key());
    event.leap_day_folded = event
        .anniversary
        .map(|anchor| opts.leap_day.fold_anchor(anchor, opts.reference_year).1)
        .unwrap_or(false);
    event.age_at_event = primary_age(event, index);
}

/// `reference_year − event_year` (plan §8.4), where the event year is the
/// *start* year of the date — ranges and spans measure from their start
/// (§8.2). `None` when the year is unknown (year 0, text dates).
///
/// Values are kept raw: a negative elapsed (event after the reference year)
/// stays negative, and the `"—"` rendering for that case (§8.4) is the
/// views' job.
pub fn elapsed_years(date: &GrampsDate, reference_year: i32) -> Option<i32> {
    date.year().map(|year| reference_year - year)
}

/// The primary (first) subject's age (years, months) when the event
/// happened, computed from their birth event (plan §8.9). Orphan
/// placeholders and events without a resolvable primary person yield
/// `None`.
fn primary_age(event: &ResolvedEvent, index: &HandleIndex) -> Option<(i32, u32)> {
    if event.subjects.is_empty() {
        return None;
    }
    let subject = &event.subjects[0];
    if subject.handle.is_empty() {
        return None;
    }
    match index.person(subject.handle.as_str()) {
        None => None,
        Some(person) => match birth_event(person, index) {
            None => None,
            Some(birth_event) => match birth_event.date.as_ref() {
                None => None,
                Some(birth) => event
                    .date
                    .as_ref()
                    .and_then(|at_event| age_at_event(birth, at_event)),
            },
        },
    }
}

/// The age (years, months) reached when `event` happened, for someone born
/// on `birth`, from the raw calendar triples (plan §8.9):
///
/// - full birth and event dates → a precise years/months age;
/// - partial dates (month or day `0`) → a best-effort years-only estimate;
/// - `None` when either year is unknown, or the event predates the birth.
pub fn age_at_event(birth: &GrampsDate, event: &GrampsDate) -> Option<(i32, u32)> {
    let birth_year = birth.year()?;
    let event_year = event.year()?;
    let birth_month = birth.ymd.1;
    let event_month = event.ymd.1;
    let birth_day = birth.ymd.2;
    let event_day = event.ymd.2;
    if event_year < birth_year {
        return None;
    }
    if birth_month == 0 || event_month == 0 {
        // Best-effort years only.
        return Some((event_year - birth_year, 0));
    }
    let mut month_total =
        (event_year - birth_year) * 12 + (event_month as i32 - birth_month as i32);
    if birth_day != 0 && event_day != 0 && event_day < birth_day {
        month_total -= 1;
    }
    if month_total < 0 {
        return None;
    }
    let years = month_total / 12;
    if birth_day == 0 || event_day == 0 {
        // Years only — the day is unknown, so month precision is a guess.
        return Some((years, 0));
    }
    Some((years, (month_total % 12) as u32))
}

/// The person's birth event — the event of type `Birth` they reference,
/// preferring a `Primary`-role ref (Gramps `get_birth_ref` semantics, so a
/// Witness ref to someone else's birth never counts).
fn birth_event<'a>(person: &'a Person, index: &'a HandleIndex) -> Option<&'a Event> {
    for ev_ref in &person.event_refs {
        if let Some(event) = index.event(ev_ref.event_handle.as_str())
            && event.event_type == BIRTH_TYPE
            && ev_ref.role.as_str() == PRIMARY_ROLE
        {
            return Some(event);
        }
    }
    for ev_ref in &person.event_refs {
        if let Some(event) = index.event(ev_ref.event_handle.as_str())
            && event.event_type == BIRTH_TYPE
        {
            return Some(event);
        }
    }
    None
}

/// Whether `person` is (probably) alive in `reference_year` — a port of
/// Gramps `gramps.gen.utils.alive.probably_alive()` (plan §8 rule 15):
///
/// 1. a `Death`, `Cremation` or `Burial` event referencing the person
///    (any date) marks them dead;
/// 2. otherwise a known birth year more than [`MAX_PROBABLE_AGE`] years
///    before the reference year presumes death;
/// 3. otherwise — no birth event, unknown birth year, or a recent enough
///    birth — they are presumed alive.
pub fn probably_alive(person: &Person, index: &HandleIndex, reference_year: i32) -> bool {
    for ev_ref in &person.event_refs {
        if let Some(event) = index.event(ev_ref.event_handle.as_str())
            && is_death_type(event.event_type.as_str())
        {
            return false;
        }
    }
    match birth_event(person, index) {
        Some(birth_event) => match birth_event.date.as_ref() {
            Some(birth) => match birth.year() {
                Some(year) => reference_year - year < MAX_PROBABLE_AGE,
                None => true,
            },
            None => true,
        },
        None => true,
    }
}

/// One of the death-type event names (plan §8 rule 15).
fn is_death_type(event_type: &str) -> bool {
    for death_type in DEATH_TYPES {
        if event_type == *death_type {
            return true;
        }
    }
    false
}

/// Does `event` pass every `opts` filter?
///
/// Applies the plan's defaults: private (§8.7) and orphan (D7) events are
/// dropped unless opted in; the type filter follows rule 14 (exclusion
/// wins); the person filter matches any subject by Gramps id or handle; the
/// date range bounds the normalized start date (dropping undated events
/// when set); living-only drops an event when any subject is (probably)
/// dead (§8 rule 15).
pub(crate) fn passes(event: &ResolvedEvent, index: &HandleIndex, opts: &ReportOptions) -> bool {
    if !opts.include_private && event.private {
        return false;
    }
    if !opts.show_orphans && event.orphan {
        return false;
    }
    if !opts.type_allowed(event.event_type.as_str()) {
        return false;
    }
    if let Some(people) = opts.include_people.as_ref() {
        let selected = event
            .subjects
            .iter()
            .any(|subject| subject_matches(subject, people));
        if !selected {
            return false;
        }
    }
    match opts.date_range.as_ref() {
        None => {}
        Some((start, stop)) => match event.gregorian.as_ref() {
            None => return false,
            Some(date) if date < start || stop < date => return false,
            _ => {}
        },
    }
    if opts.living_only {
        let someone_dead = event
            .subjects
            .iter()
            .any(|subject| subject_is_dead(subject, index, opts.reference_year));
        if someone_dead {
            return false;
        }
    }
    true
}

/// Does `subject` match the person filter? A subject is selected when
/// either their Gramps id or their handle is in `people`; the orphan
/// placeholder (no id, no handle) never matches.
fn subject_matches(subject: &PersonDisplay, people: &HashSet<String>) -> bool {
    if let Some(id) = subject.gramps_id.as_ref()
        && people.contains(id.as_str())
    {
        return true;
    }
    !subject.handle.is_empty() && people.contains(subject.handle.as_str())
}

/// Is `subject` a resolvable person who is (probably) dead in
/// `reference_year` (plan §8 rule 15)? Orphan placeholders and unresolvable
/// handles are presumed alive (not known dead).
fn subject_is_dead(subject: &PersonDisplay, index: &HandleIndex, reference_year: i32) -> bool {
    if subject.handle.is_empty() {
        return false;
    }
    match index.person(subject.handle.as_str()) {
        None => false,
        Some(person) => !probably_alive(person, index, reference_year),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect_events;
    use crate::options::LeapDayPolicy;
    use crate::resolve::build_index;
    use gramps_xml::parse_database;
    use jiff::civil::Date;

    /// The committed example tree (step 1): 5 people, 6 events incl. one
    /// death; everyone else is alive in 2026 (ages 26–88).
    const DATA_GRAMPS: &[u8] = include_bytes!("../../../tests/fixtures/data.gramps");

    /// A hand-written database exercising derivation and the filters:
    /// Feb 29 and year-only births, Death/Cremation/Burial, a private
    /// event, a family couple, an orphan, ranges, an undated event, a
    /// text date and a person without a birth event.
    const FILTERS_XML: &[u8] = br#"<database>
  <events>
    <event handle="_e0" id="E0" change="1">
      <type>Birth</type>
      <dateval val="2000-02-29"/>
    </event>
    <event handle="_e1" id="E1" change="1">
      <type>Birth</type>
      <dateval val="1900-01-15"/>
    </event>
    <event handle="_e2" id="E2" change="1">
      <type>Birth</type>
      <dateval val="1980-05-20"/>
    </event>
    <event handle="_e3" id="E3" change="1">
      <type>Death</type>
      <dateval val="2010-06-10"/>
    </event>
    <event handle="_e4" id="E4" change="1">
      <type>Birth</type>
      <dateval val="1990-01-01"/>
    </event>
    <event handle="_e5" id="E5" change="1">
      <type>Burial</type>
      <dateval val="2015-02-03"/>
    </event>
    <event handle="_e6" id="E6" change="1">
      <type>Birth</type>
      <dateval val="1985-03-03"/>
    </event>
    <event handle="_e7" id="E7" change="1">
      <type>Cremation</type>
      <dateval val="2019-04-04"/>
    </event>
    <event handle="_e8" id="E8" change="1">
      <type>Marriage</type>
      <dateval val="1999-06-15"/>
    </event>
    <event handle="_e9" id="E9" change="1" priv="1">
      <type>Adoption</type>
      <dateval val="2001-02-01"/>
    </event>
    <event handle="_e10" id="E10" change="1">
      <type>Immigration</type>
      <daterange start="1995-07-07" stop="1995-07-14"/>
    </event>
    <event handle="_e11" id="E11" change="1">
      <type>Graduation</type>
      <daterange start="1998-09-01" stop="1998-09-30"/>
    </event>
    <event handle="_e12" id="E12" change="1">
      <type>Birth</type>
      <dateval val="2001-11-15"/>
    </event>
    <event handle="_e13" id="E13" change="1">
      <type>Conference</type>
    </event>
    <event handle="_e14" id="E14" change="1">
      <type>Fete</type>
      <datestr val="around midsummer"/>
    </event>
    <event handle="_e15" id="E15" change="1">
      <type>Quest</type>
      <dateval val="1994-04-04"/>
    </event>
  </events>
  <people>
    <person handle="_p0" id="I0000">
      <name type="Birth Name">
        <first>Alice</first>
        <surname>Leaps</surname>
      </name>
      <eventref hlink="_e0" role="Primary"/>
      <eventref hlink="_e9" role="Primary"/>
      <eventref hlink="_e10" role="Witness"/>
    </person>
    <person handle="_p1" id="I0001">
      <name type="Birth Name">
        <first>Boris</first>
        <surname>Old</surname>
      </name>
      <eventref hlink="_e1" role="Primary"/>
      <eventref hlink="_e11" role="Primary"/>
    </person>
    <person handle="_p2" id="I0002">
      <name type="Birth Name">
        <first>Cara</first>
        <surname>Gone</surname>
      </name>
      <eventref hlink="_e2" role="Primary"/>
      <eventref hlink="_e3" role="Primary"/>
    </person>
    <person handle="_p3" id="I0003">
      <name type="Birth Name">
        <first>Dana</first>
        <surname>Buried</surname>
      </name>
      <eventref hlink="_e4" role="Primary"/>
      <eventref hlink="_e5" role="Primary"/>
    </person>
    <person handle="_p4" id="I0004">
      <name type="Birth Name">
        <first>Erin</first>
        <surname>Cremated</surname>
      </name>
      <eventref hlink="_e6" role="Primary"/>
      <eventref hlink="_e7" role="Primary"/>
    </person>
    <person handle="_p5" id="I0005">
      <name type="Birth Name">
        <first>Fiona</first>
        <surname>Lively</surname>
      </name>
      <eventref hlink="_e12" role="Primary"/>
      <eventref hlink="_e13" role="Primary"/>
    </person>
    <person handle="_p6" id="I0006">
      <name type="Birth Name">
        <first>Greta</first>
        <surname>Hmm</surname>
      </name>
      <eventref hlink="_e14" role="Primary"/>
    </person>
  </people>
  <families>
    <family handle="_f0" id="F0" change="3">
      <father hlink="_p0"/>
      <mother hlink="_p3"/>
      <eventref hlink="_e8" role="Family"/>
    </family>
  </families>
</database>"#;

    fn opts() -> ReportOptions {
        ReportOptions::with_reference_year(2026)
    }

    fn resolved() -> Vec<ResolvedEvent> {
        let db = parse_database(FILTERS_XML).unwrap();
        collect_events(&db, &opts())
    }

    /// The event of `event_type` in `events`, which must be present.
    fn by_type<'a>(events: &'a [ResolvedEvent], event_type: &str) -> &'a ResolvedEvent {
        events
            .iter()
            .find(|event| event.event_type == event_type)
            .expect("event type must be present")
    }

    fn set_of(values: &[&str]) -> HashSet<String> {
        let mut set = HashSet::new();
        for value in values {
            set.insert(value.to_string());
        }
        set
    }

    // --- elapsed years ------------------------------------------------------

    #[test]
    fn elapsed_years_from_event_years() {
        let events = resolved();

        // Alice's Feb 29 2000 birth — 26 years elapsed in 2026.
        assert_eq!(events[0].event_type, "Birth");
        assert_eq!(events[0].elapsed_years, Some(26));

        // Boris born 1900 — 126.
        assert_eq!(events[1].elapsed_years, Some(126));

        // the 1999 marriage measured from its start year
        assert_eq!(by_type(&events, "Marriage").elapsed_years, Some(27));

        // ranges measure from the range start (§8.2)
        assert_eq!(by_type(&events, "Immigration").elapsed_years, Some(31));
        assert_eq!(by_type(&events, "Graduation").elapsed_years, Some(28));

        // undated and text-only dates carry no year → no elapsed
        assert_eq!(by_type(&events, "Conference").elapsed_years, None);
        assert_eq!(by_type(&events, "Fete").elapsed_years, None);

        // reference year equal to and before the event year
        let db = parse_database(FILTERS_XML).unwrap();
        let same_year = collect_events(&db, &ReportOptions::with_reference_year(2000));
        assert_eq!(same_year[0].elapsed_years, Some(0));
        let negative = collect_events(&db, &ReportOptions::with_reference_year(1999));
        assert_eq!(negative[0].elapsed_years, Some(-1));
    }

    #[test]
    fn elapsed_years_is_reference_year_minus_event_year() {
        let db = parse_database(DATA_GRAMPS).unwrap();
        let events = collect_events(&db, &opts());
        assert_eq!(events[0].elapsed_years, Some(26)); // Harry born 2000
        assert_eq!(events[5].elapsed_years, Some(6)); // Abraham died 2020
    }

    // --- anniversary anchors + leap-day fold --------------------------------

    #[test]
    fn anniversary_anchors_come_from_the_start_date() {
        let events = resolved();

        // full dates anchor on (month, day)
        assert_eq!(by_type(&events, "Marriage").anniversary, Some((6, 15)));
        assert_eq!(by_type(&events, "Quest").anniversary, Some((4, 4)));

        // ranges anchor at their start (§8.2)
        assert_eq!(by_type(&events, "Immigration").anniversary, Some((7, 7)));

        // undated and text dates have no anchor (rule 1)
        assert_eq!(by_type(&events, "Conference").anniversary, None);
        assert_eq!(by_type(&events, "Fete").anniversary, None);
    }

    #[test]
    fn feb_29_anchor_folds_in_a_non_leap_reference_year() {
        let db = parse_database(FILTERS_XML).unwrap();

        // 2026 is not a leap year → folded, and the fold is recorded.
        let folded = collect_events(&db, &ReportOptions::with_reference_year(2026));
        let alice_birth = &folded[0];
        assert_eq!(alice_birth.anniversary, Some((2, 29)));
        assert!(alice_birth.leap_day_folded);

        // 2024 is a leap year → the anchor stays put.
        let unfolded = collect_events(&db, &ReportOptions::with_reference_year(2024));
        assert_eq!(unfolded[0].anniversary, Some((2, 29)));
        assert!(!unfolded[0].leap_day_folded);

        // the Keep policy never folds.
        let mut keep = ReportOptions::with_reference_year(2026);
        keep.leap_day = LeapDayPolicy::Keep;
        let kept = collect_events(&db, &keep);
        assert!(!kept[0].leap_day_folded);
    }

    // --- age at event -------------------------------------------------------

    #[test]
    fn age_at_event_from_full_dates() {
        let events = resolved();

        // Cara: born 1980-05-20, died 2010-06-10 → 30y 0m (day 10 < 20).
        assert_eq!(by_type(&events, "Death").age_at_event, Some((30, 0)));

        // Dana: born 1990-01-01, buried 2015-02-03 → 25y 1m.
        assert_eq!(by_type(&events, "Burial").age_at_event, Some((25, 1)));

        // Erin: born 1985-03-03, cremated 2019-04-04 → 34y 1m.
        assert_eq!(by_type(&events, "Cremation").age_at_event, Some((34, 1)));

        // Alice's own birth → 0y 0m.
        assert_eq!(by_type(&events, "Birth").age_at_event, Some((0, 0)));

        // Boris (born 1900) at his 1998 graduation → 98y 7m.
        assert_eq!(by_type(&events, "Graduation").age_at_event, Some((98, 7)));

        // The 1999 marriage predates Alice's 2000 birth → no age.
        assert_eq!(by_type(&events, "Marriage").age_at_event, None);

        // Undated events have no age.
        assert_eq!(by_type(&events, "Conference").age_at_event, None);
    }

    #[test]
    fn age_at_event_uses_the_primary_birth_ref() {
        let db = parse_database(DATA_GRAMPS).unwrap();
        let events = collect_events(&db, &opts());
        // Abraham: born 1938-11-11, died 2020-12-03 → 82y 0m (day 3 < 11).
        assert_eq!(by_type(&events, "Death").age_at_event, Some((82, 0)));
    }

    #[test]
    fn age_at_event_years_only_for_partial_dates() {
        let birth = GrampsDate {
            calendar: gramps_dates::Calendar::Gregorian,
            modifier: gramps_dates::Modifier::None,
            quality: gramps_dates::Quality::None,
            ymd: (1985, 6, 0),
            stop: None,
            dual_dated: false,
            new_year: gramps_dates::NewYear::Jan1,
            display: "1985-06".to_string(),
        };
        let event = GrampsDate {
            calendar: gramps_dates::Calendar::Gregorian,
            modifier: gramps_dates::Modifier::None,
            quality: gramps_dates::Quality::None,
            ymd: (2019, 4, 0),
            stop: None,
            dual_dated: false,
            new_year: gramps_dates::NewYear::Jan1,
            display: "2019-04".to_string(),
        };
        // both months known, days unknown → best-effort years only
        assert_eq!(age_at_event(&birth, &event), Some((33, 0)));

        let no_year = GrampsDate {
            calendar: gramps_dates::Calendar::Gregorian,
            modifier: gramps_dates::Modifier::None,
            quality: gramps_dates::Quality::None,
            ymd: (0, 5, 12),
            stop: None,
            dual_dated: false,
            new_year: gramps_dates::NewYear::Jan1,
            display: String::new(),
        };
        assert_eq!(age_at_event(&no_year, &event), None);

        let later_birth = GrampsDate {
            calendar: gramps_dates::Calendar::Gregorian,
            modifier: gramps_dates::Modifier::None,
            quality: gramps_dates::Quality::None,
            ymd: (2020, 1, 1),
            stop: None,
            dual_dated: false,
            new_year: gramps_dates::NewYear::Jan1,
            display: String::new(),
        };
        // event before birth → no age
        assert_eq!(age_at_event(&later_birth, &birth), None);
    }

    // --- probably_alive -----------------------------------------------------

    /// A focused population for the `probably_alive` port (plan §8 rule 15).
    const ALIVE_XML: &[u8] = br#"<database>
  <events>
    <event handle="_a0" change="1"><type>Birth</type><dateval val="1900-01-01"/></event>
    <event handle="_a1" change="1"><type>Birth</type><dateval val="2001-06-15"/></event>
    <event handle="_a2" change="1"><type>Birth</type><dateval val="1970-03-03"/></event>
    <event handle="_a3" change="1"><type>Death</type><dateval val="1999-12-31"/></event>
    <event handle="_a4" change="1"><type>Birth</type><dateval val="1980-04-04"/></event>
    <event handle="_a5" change="1"><type>Cremation</type><dateval val="2010-05-05"/></event>
    <event handle="_a6" change="1"><type>Birth</type><dateval val="1990-07-07"/></event>
    <event handle="_a7" change="1"><type>Burial</type><dateval val="2011-08-08"/></event>
    <event handle="_a8" change="1"><type>Birth</type><datestr val="long ago"/></event>
    <event handle="_a9" change="1"><type>Party</type><daterange start="2020-01-01" stop="2020-01-02"/></event>
  </events>
  <people>
    <person handle="_x0" id="I00"><name type="Birth Name"><first>One</first><surname>Old</surname></name>
      <eventref hlink="_a0" role="Primary"/></person>
    <person handle="_x1" id="I01"><name type="Birth Name"><first>Two</first><surname>Young</surname></name>
      <eventref hlink="_a1" role="Primary"/></person>
    <person handle="_x2" id="I02"><name type="Birth Name"><first>Three</first><surname>Deady</surname></name>
      <eventref hlink="_a2" role="Primary"/>
      <eventref hlink="_a3" role="Primary"/></person>
    <person handle="_x3" id="I03"><name type="Birth Name"><first>Four</first><surname>Cremy</surname></name>
      <eventref hlink="_a4" role="Primary"/>
      <eventref hlink="_a5" role="Primary"/></person>
    <person handle="_x4" id="I04"><name type="Birth Name"><first>Five</first><surname>Bury</surname></name>
      <eventref hlink="_a6" role="Primary"/>
      <eventref hlink="_a7" role="Primary"/></person>
    <person handle="_x5" id="I05"><name type="Birth Name"><first>Six</first><surname>Antique</surname></name>
      <eventref hlink="_a8" role="Primary"/></person>
    <person handle="_x6" id="I06"><name type="Birth Name"><first>Seven</first><surname>Ref</surname></name>
      <eventref hlink="_a9" role="Witness"/></person>
  </people>
</database>"#;

    #[test]
    fn probably_alive_matches_the_gramps_port() {
        let db = parse_database(ALIVE_XML).unwrap();
        let index = build_index(&db);
        let by_id = |id: &str| {
            db.people
                .iter()
                .find(|person| person.gramps_id.as_deref() == Some(id))
                .unwrap()
        };
        let alive = |id: &str| probably_alive(by_id(id), &index, 2026);

        // death-type events (any date) mark a person dead.
        assert!(!alive("I02")); // Death
        assert!(!alive("I03")); // Cremation
        assert!(!alive("I04")); // Burial

        // the estimate: birth known and reference − birth ≥ 110 → dead.
        assert!(!alive("I00")); // born 1900, 126 years before 2026

        // otherwise presumed alive: recent birth, unknown birth year, none.
        assert!(alive("I01")); // born 2001
        assert!(alive("I05")); // birth year unknown (datestr)
        assert!(alive("I06")); // no birth event at all
    }

    // --- the filtering pipeline --------------------------------------------

    #[test]
    fn defaults_hide_private_but_keep_everything_else() {
        let events = resolved();
        // 16 events, one private (E9 Adoption) → 15 survive.
        assert_eq!(events.len(), 15);
        assert!(events.iter().all(|event| !event.private));
        assert_eq!(
            events.iter().find(|event| event.event_type == "Adoption"),
            None
        );
    }

    #[test]
    fn include_private_keeps_private_events() {
        let mut include = opts();
        include.include_private = true;
        let db = parse_database(FILTERS_XML).unwrap();
        let events = collect_events(&db, &include);
        assert_eq!(events.len(), 16);
        assert_eq!(by_type(&events, "Adoption").event_type, "Adoption");
        assert!(by_type(&events, "Adoption").private);
    }

    #[test]
    fn orphan_events_are_shown_by_default_and_toggled_off() {
        let mut hidden = opts();
        hidden.show_orphans = false;
        let db = parse_database(FILTERS_XML).unwrap();
        let visible = collect_events(&db, &opts());
        let without_orphans = collect_events(&db, &hidden);
        assert_eq!(visible.len(), 15);
        assert_eq!(without_orphans.len(), 14);
        assert!(without_orphans.iter().all(|event| !event.orphan));
        assert!(by_type(&visible, "Quest").orphan);
    }

    #[test]
    fn type_filters_implement_whitelist_exclusion_and_precedence() {
        let db = parse_database(FILTERS_XML).unwrap();
        let mut include_private = opts();
        include_private.include_private = true;

        // whitelist: only Birth
        let mut only_birth = include_private.clone();
        only_birth.include_types = Some(set_of(&["Birth"]));
        let births = collect_events(&db, &only_birth);
        assert_eq!(births.len(), 6);
        assert!(births.iter().all(|event| event.event_type == "Birth"));

        // exclusion subtracts from the "all types" default
        let mut no_births = include_private.clone();
        no_births.exclude_types = set_of(&["Birth", "Death"]);
        let rest = collect_events(&db, &no_births);
        assert_eq!(rest.len(), 9);
        assert!(
            rest.iter()
                .all(|event| event.event_type != "Birth" && event.event_type != "Death")
        );

        // exclusion wins over the whitelist (rule 14)
        let mut mixed = include_private.clone();
        mixed.include_types = Some(set_of(&["Birth", "Death", "Cremation"]));
        mixed.exclude_types = set_of(&["Death"]);
        let chosen = collect_events(&db, &mixed);
        assert_eq!(chosen.len(), 7);
        let found_birth = chosen.iter().find(|event| event.event_type == "Birth");
        let found_cremation = chosen.iter().find(|event| event.event_type == "Cremation");
        let found_death = chosen.iter().find(|event| event.event_type == "Death");
        assert!(found_birth.is_some());
        assert!(found_cremation.is_some());
        assert_eq!(found_death, None);
    }

    #[test]
    fn person_filter_matches_by_id_or_handle() {
        let db = parse_database(FILTERS_XML).unwrap();

        // by Gramps id — Alice's events: her Birth, the Immigration
        // witness role and the family Marriage; the private Adoption is
        // hidden by the default privacy filter.
        let mut by_id = opts();
        by_id.include_people = Some(set_of(&["I0000"]));
        let alice = collect_events(&db, &by_id);
        assert_eq!(alice.len(), 3);

        // by handle — the same person
        let mut by_handle = opts();
        by_handle.include_people = Some(set_of(&["_p0"]));
        let alice_again = collect_events(&db, &by_handle);
        assert_eq!(alice_again.len(), 3);

        // a person whose only event is theirs (Greta's Fete)
        let mut by_greta = opts();
        by_greta.include_people = Some(set_of(&["I0006"]));
        let greta = collect_events(&db, &by_greta);
        assert_eq!(greta.len(), 1);
        assert_eq!(greta[0].event_type, "Fete");

        // the orphan never matches a person filter
        assert!(alice.iter().all(|event| !event.orphan));

        // unknown people select nothing
        let mut nobody = opts();
        nobody.include_people = Some(set_of(&["I9999"]));
        assert_eq!(collect_events(&db, &nobody).len(), 0);
    }

    #[test]
    fn date_range_bounds_the_normalized_start_date() {
        let db = parse_database(FILTERS_XML).unwrap();
        let mut windowed = opts();
        windowed.date_range = Some((
            Date::new(2000, 1, 1).ok().unwrap(),
            Date::new(2009, 12, 31).ok().unwrap(),
        ));
        let events = collect_events(&db, &windowed);
        // Alice's Feb 29 2000 birth and Fiona's 2001-11-15 birth fall
        // inside the window; every other event sorts outside it, and the
        // undated/text/private events have no normalizable date at all.
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| event.event_type == "Birth"));
        assert!(events.iter().all(|event| event.gregorian.is_some()));

        // a window over a decade isolates that decade's dated events
        let mut later = opts();
        later.date_range = Some((
            Date::new(2010, 1, 1).ok().unwrap(),
            Date::new(2019, 12, 31).ok().unwrap(),
        ));
        let decade = collect_events(&db, &later);
        assert_eq!(decade.len(), 3);
        assert_eq!(by_type(&decade, "Death").event_type, "Death");
        assert_eq!(by_type(&decade, "Burial").event_type, "Burial");
        assert_eq!(by_type(&decade, "Cremation").event_type, "Cremation");

        // an empty window keeps nothing (boundary inclusivity: 2020 is out)
        let mut narrow = opts();
        narrow.date_range = Some((
            Date::new(2021, 1, 1).ok().unwrap(),
            Date::new(2021, 12, 31).ok().unwrap(),
        ));
        assert_eq!(collect_events(&db, &narrow).len(), 0);
    }

    #[test]
    fn living_only_keeps_events_with_no_dead_subject() {
        let mut living = opts();
        living.living_only = true;
        let db = parse_database(FILTERS_XML).unwrap();
        let events = collect_events(&db, &living);

        // Alice's Birth + Immigration witness, Fiona's Birth + Conference,
        // Greta's Fete, and the orphan Quest (no subjects → living).
        assert_eq!(events.len(), 6);
        assert!(events.iter().any(|event| event.event_type == "Birth"));
        assert!(events.iter().any(|event| event.event_type == "Immigration"));
        assert!(events.iter().any(|event| event.event_type == "Conference"));
        assert!(events.iter().any(|event| event.event_type == "Fete"));
        assert!(events.iter().any(|event| event.event_type == "Quest"));

        // Boris (presumed dead by age), Cara/Dana/Erin (dead by death-type
        // events) and the couple Marriage (one dead spouse) are all gone.
        for gone in ["Death", "Burial", "Cremation", "Marriage", "Graduation"] {
            assert_eq!(events.iter().find(|event| event.event_type == gone), None);
        }
        // the private Adoption stays hidden under the default privacy rule
        assert_eq!(
            events.iter().find(|event| event.event_type == "Adoption"),
            None
        );
    }

    #[test]
    fn filters_compose_in_a_single_run() {
        let mut opts = opts();
        opts.include_private = true;
        opts.include_types = Some(set_of(&["Birth", "Cremation", "Adoption"]));
        opts.exclude_types = set_of(&["Cremation"]);
        opts.include_people = Some(set_of(&["I0000", "I0005"]));
        opts.date_range = Some((
            Date::new(2000, 1, 1).ok().unwrap(),
            Date::new(2005, 12, 31).ok().unwrap(),
        ));
        let db = parse_database(FILTERS_XML).unwrap();
        let events = collect_events(&db, &opts);
        // Alice's Feb 29 2000 Birth, her 2001 Adoption (private, now
        // shown) and Fiona's 2001 Birth — Cremation is excluded by
        // precedence even though whitelisted; everything else fails the
        // person or the date window.
        assert_eq!(events.len(), 3);
        assert!(events.iter().any(|event| event.event_type == "Birth"));
        assert!(events.iter().any(|event| event.event_type == "Adoption"));
        assert_eq!(
            events.iter().find(|event| event.event_type == "Cremation"),
            None
        );
        // exactly the Feb 29 birth folds
        let folded = events.iter().find(|event| event.leap_day_folded).unwrap();
        assert_eq!(folded.anniversary, Some((2, 29)));
    }
}
