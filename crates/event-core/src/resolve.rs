//! Handle → record indexing and event-to-subject resolution (plan §7.1,
//! §7.3, §8.6).
//!
//! [`build_index`] builds the `handle → record` index the pipeline consults
//! once; [`collect_events`] resolves every `<event>` record to a
//! [`ResolvedEvent`], linking each event to the people or couples that
//! reference it. Subject resolution follows the plan's precedence order
//! (§8.6):
//!
//! | Step | Subjects | Where the link comes from |
//! | --- | --- | --- |
//! | (a) | people holding a `Primary`-role eventref | person eventrefs |
//! | (b) | else any eventref'd person | person eventrefs |
//! | (c) | else family events → the couple (father ⚭ mother) | family eventrefs |
//! | (d) | a family that links a single spouse → that one subject | family eventrefs |
//! | (e) | else orphans → placeholder `"—"` (D7) | no link |
//!
//! (d) is folded into (c): resolution never yields an empty subject set for
//! a referenced event. Within one event a person appears at most once —
//! someone holding several eventrefs to the same event keeps their best
//! (Primary-first) role (§3.5 dedup), and a couple shared across several
//! referencing families collapses to one entry. Names render `First Surname`
//! from the primary (first non-alternate) name (plan §8.8), surnames with
//! their `prefix`; place paths walk the `placeref` parent chain (event
//! place first, ancestors after) with a cycle guard for malformed
//! hierarchies.

use std::collections::{HashMap, HashSet};

use gramps_xml::model::{Database, Event, EventRef, Family, Person, PersonName, Place, Surname};

use crate::model::{PersonDisplay, ResolvedEvent};

/// The role Gramps writes for a person's principal eventref
/// (`<eventref role="Primary"/>`).
const PRIMARY_ROLE: &str = "Primary";

/// Display name of an orphan event's placeholder subject (plan D7).
const ORPHAN_NAME: &str = "—";

/// A `handle → record` index over the primary record kinds resolution
/// consults: events, people, families and places.
#[derive(Debug, Clone)]
pub struct HandleIndex {
    pub events: HashMap<String, Event>,
    pub people: HashMap<String, Person>,
    pub families: HashMap<String, Family>,
    pub places: HashMap<String, Place>,
}

impl HandleIndex {
    /// The event record with `handle`, or `None` when unknown.
    pub fn event(&self, handle: &str) -> Option<&Event> {
        self.events.get(handle)
    }

    /// The person record with `handle`, or `None` when unknown.
    pub fn person(&self, handle: &str) -> Option<&Person> {
        self.people.get(handle)
    }

    /// The family record with `handle`, or `None` when unknown.
    pub fn family(&self, handle: &str) -> Option<&Family> {
        self.families.get(handle)
    }

    /// The place record with `handle`, or `None` when unknown.
    pub fn place(&self, handle: &str) -> Option<&Place> {
        self.places.get(handle)
    }
}

/// Build the `handle → record` index over `db`'s primary records.
pub fn build_index(db: &Database) -> HandleIndex {
    let mut index = HandleIndex {
        events: HashMap::new(),
        people: HashMap::new(),
        families: HashMap::new(),
        places: HashMap::new(),
    };
    for event in &db.events {
        index.events.insert(event.handle.clone(), event.clone());
    }
    for person in &db.people {
        index.people.insert(person.handle.clone(), person.clone());
    }
    for family in &db.families {
        index.families.insert(family.handle.clone(), family.clone());
    }
    for place in &db.places {
        index.places.insert(place.handle.clone(), place.clone());
    }
    index
}

/// Resolve every event in `db` to a [`ResolvedEvent`], in document order.
///
/// No event is dropped here — privacy and orphan filtering are the
/// milestone-7 `ReportOptions` pipeline's job (plan §8.7, D7); resolution
/// only flags them on [`ResolvedEvent::private`] / [`ResolvedEvent::orphan`].
pub fn collect_events(db: &Database) -> Vec<ResolvedEvent> {
    let index = build_index(db);
    db.events
        .iter()
        .map(|event| resolve_event(event, db, &index))
        .collect::<Vec<_>>()
}

/// The outcome of one event's subject resolution.
struct Resolution {
    subjects: Vec<PersonDisplay>,
    role: String,
    orphan: bool,
}

/// Resolve one event to its subjects, place path and derived fields.
fn resolve_event(event: &Event, db: &Database, index: &HandleIndex) -> ResolvedEvent {
    let resolution = resolve_subjects(event, db, index);
    let date = event.date.as_ref();
    ResolvedEvent {
        event_type: event.event_type.clone(),
        date: date.cloned(),
        gregorian: date.and_then(|gramps| gramps.to_gregorian()),
        subjects: resolution.subjects,
        role: resolution.role,
        place_path: place_path(event, index),
        private: event.private,
        orphan: resolution.orphan,
    }
}

/// Apply the subject-resolution precedence order (plan §8.6 a–e).
fn resolve_subjects(event: &Event, db: &Database, index: &HandleIndex) -> Resolution {
    // (a) Primary-role eventrefs on people.
    let referencing_people = person_refs_for(&event.handle, db);
    let primary = referencing_people
        .iter()
        .filter(|entry| entry.1.role.as_str() == PRIMARY_ROLE)
        .collect::<Vec<_>>();
    if !primary.is_empty() {
        return Resolution {
            subjects: dedup_people(
                primary
                    .iter()
                    .map(|entry| display_person(entry.0, PRIMARY_ROLE))
                    .collect::<Vec<_>>(),
            ),
            role: PRIMARY_ROLE.to_string(),
            orphan: false,
        };
    }
    // (b) otherwise any eventref'd person.
    if !referencing_people.is_empty() {
        let subjects = dedup_people(
            referencing_people
                .iter()
                .map(|entry| display_person(entry.0, entry.1.role.as_str()))
                .collect::<Vec<_>>(),
        );
        let role = subjects[0].role.clone();
        return Resolution {
            subjects,
            role,
            orphan: false,
        };
    }
    // (c) family events → the couple; a family linking a single spouse (d)
    // yields that one subject — never an empty set.
    let family_refs = family_refs_for(&event.handle, db);
    if !family_refs.is_empty() {
        let role = family_refs[0].1.role.clone();
        let mut subjects = Vec::new();
        let mut seen = HashSet::new();
        for (family, ev_ref) in family_refs {
            let spouse_role = ev_ref.role.as_str();
            push_spouse(
                &mut subjects,
                &mut seen,
                index,
                family.father.clone(),
                spouse_role,
            );
            push_spouse(
                &mut subjects,
                &mut seen,
                index,
                family.mother.clone(),
                spouse_role,
            );
        }
        return Resolution {
            subjects,
            role,
            orphan: false,
        };
    }
    // (e) orphan — no person or family references the event (plan D7).
    Resolution {
        subjects: vec![PersonDisplay {
            gramps_id: None,
            handle: String::new(),
            name: ORPHAN_NAME.to_string(),
            role: String::new(),
        }],
        role: String::new(),
        orphan: true,
    }
}

/// The people referencing `event_handle`, each with their *best* eventref
/// to it — a `Primary`-role ref when they hold one, else their first ref.
/// Preferring `Primary` keeps the (a)/(b) split independent of eventref
/// document order.
fn person_refs_for<'a>(event_handle: &'a str, db: &'a Database) -> Vec<(&'a Person, &'a EventRef)> {
    db.people
        .iter()
        .filter_map(|person| best_ref(person, event_handle).map(|ev_ref| (person, ev_ref)))
        .collect::<Vec<_>>()
}

/// The families referencing `event_handle`, each with their first eventref
/// to it.
fn family_refs_for<'a>(event_handle: &'a str, db: &'a Database) -> Vec<(&'a Family, &'a EventRef)> {
    db.families
        .iter()
        .filter_map(|family| {
            family
                .event_refs
                .iter()
                .find(|ev_ref| ev_ref.event_handle == event_handle)
                .map(|ev_ref| (family, ev_ref))
        })
        .collect::<Vec<_>>()
}

/// A person's best eventref to `event_handle`, or `None` when they hold no
/// ref to it.
fn best_ref<'a>(person: &'a Person, event_handle: &'a str) -> Option<&'a EventRef> {
    match person
        .event_refs
        .iter()
        .find(|ev_ref| ev_ref.event_handle == event_handle)
    {
        None => None,
        Some(first) => match person
            .event_refs
            .iter()
            .find(|ev_ref| ev_ref.event_handle == event_handle && ev_ref.role == PRIMARY_ROLE)
        {
            Some(primary) => Some(primary),
            None => Some(first),
        },
    }
}

/// Add one spouse of a referencing family to `subjects` unless already
/// present (an event referenced by two families sharing a spouse, or a
/// person listed as both father and mother, yields one entry).
fn push_spouse(
    subjects: &mut Vec<PersonDisplay>,
    seen: &mut HashSet<String>,
    index: &HandleIndex,
    handle: Option<String>,
    role: &str,
) {
    if let Some(person_handle) = handle
        && let Some(person) = index.person(person_handle.as_str())
        && seen.insert(person.handle.clone())
    {
        subjects.push(display_person(person, role));
    }
}

/// Collapse duplicate people in `people` (same `handle`), keeping the first
/// occurrence — the plan's one-row-per-(event, subject) dedup at the event
/// level (§8.6).
fn dedup_people(people: Vec<PersonDisplay>) -> Vec<PersonDisplay> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for person in people {
        if seen.insert(person.handle.clone()) {
            out.push(person);
        }
    }
    out
}

/// A person's primary display name — the first non-alternate `<name>`, or
/// the first name when every name is alternate (plan §8.8).
fn primary_name(person: &Person) -> Option<&PersonName> {
    match person.names.iter().find(|name| !name.alt) {
        Some(name) => Some(name),
        None if person.names.is_empty() => None,
        None => Some(&person.names[0]),
    }
}

/// The primary surname of a name — the first `prim`-marked surname, else
/// the first surname; `None` for a name without surnames.
fn primary_surname(surnames: &[Surname]) -> Option<&Surname> {
    match surnames.iter().find(|surname| surname.prim) {
        Some(surname) => Some(surname),
        None if surnames.is_empty() => None,
        None => Some(&surnames[0]),
    }
}

/// Render the display name `First Surname` (surname prefix included) from a
/// person's primary name; `""` for a person without names.
fn render_person_name(person: &Person) -> String {
    let mut out = String::new();
    match primary_name(person) {
        None => {}
        Some(name) => {
            if let Some(first) = &name.first {
                out.push_str(first.as_str());
            }
            match primary_surname(&name.surnames) {
                None => {}
                Some(surname) => {
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    if let Some(prefix) = &surname.prefix {
                        out.push_str(prefix.as_str());
                        out.push(' ');
                    }
                    out.push_str(surname.value.as_str());
                }
            }
        }
    }
    out
}

/// One subject entry for a person playing `role` on their event.
fn display_person(person: &Person, role: &str) -> PersonDisplay {
    PersonDisplay {
        gramps_id: person.gramps_id.clone(),
        handle: person.handle.clone(),
        name: render_person_name(person),
        role: role.to_string(),
    }
}

/// The event's place chain — the place's name followed by its `placeref`
/// parents' names ascending — or `None` when the event carries no place
/// handle or the handle does not resolve. A cycle guard bounds malformed
/// parent chains.
fn place_path(event: &Event, index: &HandleIndex) -> Option<Vec<String>> {
    match event.place_handle.as_ref() {
        None => None,
        Some(handle) => {
            let mut path = Vec::new();
            let mut seen = HashSet::new();
            let mut current = index.place(handle.as_str());
            while let Some(place) = current {
                if !seen.insert(place.handle.clone()) {
                    break;
                }
                path.push(place.name.clone());
                current = place
                    .parent_handle
                    .clone()
                    .and_then(|parent| index.place(parent.as_str()));
            }
            match path.is_empty() {
                true => None,
                false => Some(path),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gramps_xml::parse_database;

    /// The committed example tree (step 1): 5 people, 3 families, 6 events,
    /// 4 places, no family eventrefs — every event resolves through a
    /// person's `Primary` eventref.
    const DATA_GRAMPS: &[u8] = include_bytes!("../../../tests/fixtures/data.gramps");

    /// The committed linkage-shapes fixture: family-owned events, shared
    /// Primary refs, mixed roles, any-role fallback, orphans.
    const FAMILIES_GRAMPS: &[u8] = include_bytes!("../../../tests/fixtures/families.gramps");

    /// A hand-written database exercising the branches the fixtures do not:
    /// one person holding two roles on one event, surname prefixes,
    /// alternate names, a nameless person, a single-spouse family, a place
    /// parent chain, a cyclic place hierarchy and an unresolvable place
    /// handle.
    const RESOLUTION_XML: &[u8] = br#"<database>
  <events>
    <event handle="_e0" id="E0" change="1">
      <type>Birth</type>
      <dateval val="1970-01-01"/>
    </event>
    <event handle="_e1" id="E1" change="1">
      <type>Marriage</type>
      <dateval val="1990-06-01"/>
    </event>
    <event handle="_e2" id="E2" change="1">
      <type>Death</type>
      <dateval val="1900-05-05"/>
      <place hlink="_city"/>
    </event>
    <event handle="_e3" id="E3" change="1">
      <type>Burial</type>
      <dateval val="1901-01-01"/>
      <place hlink="_cycle_a"/>
    </event>
    <event handle="_e4" id="E4" change="1">
      <type>Baptism</type>
      <dateval val="1890-04-01"/>
      <place hlink="_missing"/>
    </event>
  </events>
  <people>
    <person handle="_solo" id="I0" change="2">
      <name type="Birth Name">
        <first>Vanessa</first>
        <surname prefix="van der">Berg</surname>
      </name>
      <eventref hlink="_e0" role="Primary"/>
      <eventref hlink="_e0" role="Witness"/>
    </person>
    <person handle="_dad" id="I1" change="2">
      <name type="Birth Name">
        <first>Odin</first>
        <surname>Allfather</surname>
      </name>
      <parentin hlink="_fam0"/>
      <parentin hlink="_fam1"/>
      <parentin hlink="_fam2"/>
    </person>
    <person handle="_mom" id="I2" change="2">
      <name type="Birth Name">
        <first>Frigg</first>
        <surname>Allmother</surname>
      </name>
      <parentin hlink="_fam0"/>
    </person>
    <person handle="_child" id="I3" change="2">
      <name type="Birth Name">
        <first>Baldr</first>
        <surname>Allfather</surname>
      </name>
      <childof hlink="_fam0"/>
    </person>
    <person handle="_navn" id="I4" change="2">
      <name type="Also Known As" alt="1">
        <first>Kitten</first>
        <surname>Fuzzy</surname>
      </name>
      <name type="Birth Name">
        <first>Nameless</first>
        <surname>Cat</surname>
      </name>
    </person>
    <person handle="_nameless" id="I5" change="2">
      <eventref hlink="_e3" role="Primary"/>
    </person>
  </people>
  <families>
    <family handle="_fam0" id="F0" change="3">
      <father hlink="_dad"/>
      <mother hlink="_mom"/>
      <eventref hlink="_e1" role="Family"/>
    </family>
    <family handle="_fam1" id="F1" change="3">
      <father hlink="_dad"/>
      <eventref hlink="_e2" role="Family"/>
    </family>
    <family handle="_fam2" id="F2" change="3">
      <father hlink="_dad"/>
      <eventref hlink="_e2" role="Family"/>
    </family>
  </families>
  <places>
    <placeobj handle="_city" id="P0" change="4" type="City">
      <pname value="Uppsala"/>
      <placeref hlink="_country"/>
    </placeobj>
    <placeobj handle="_country" id="P1" change="4" type="Country">
      <pname value="Sweden"/>
    </placeobj>
    <placeobj handle="_cycle_a" id="P2" change="4" type="Place">
      <pname value="Mobius"/>
      <placeref hlink="_cycle_b"/>
    </placeobj>
    <placeobj handle="_cycle_b" id="P3" change="4" type="Place">
      <pname value="Loop"/>
      <placeref hlink="_cycle_a"/>
    </placeobj>
  </places>
</database>"#;

    #[test]
    fn handle_index_maps_records_and_misses() {
        let db = parse_database(DATA_GRAMPS).unwrap();
        let index = build_index(&db);
        assert_eq!(index.events.len(), 6);
        assert_eq!(index.people.len(), 5);
        assert_eq!(index.families.len(), 3);
        assert_eq!(index.places.len(), 4);

        let first_event = index.event(db.events[0].handle.as_str()).unwrap();
        assert_eq!(first_event.handle, db.events[0].handle);
        assert_eq!(first_event.event_type, "Birth");

        let george = index.person(db.people[1].handle.as_str()).unwrap();
        assert_eq!(george.gramps_id.as_deref(), Some("I0001"));
        assert_eq!(render_person_name(george), "George Meowser");

        assert_eq!(index.family("no-such-handle"), None);
        assert_eq!(index.place("no-such-handle"), None);
    }

    #[test]
    fn index_equality_does_not_depend_on_insertion_order() {
        let plain = parse_database(DATA_GRAMPS).unwrap();
        let index = build_index(&plain);
        for event in plain.events {
            let found = index.event(event.handle.as_str()).unwrap();
            assert_eq!(found.handle, event.handle);
            assert_eq!(found.event_type, event.event_type);
        }
        for person in plain.people {
            let found = index.person(person.handle.as_str()).unwrap();
            assert_eq!(found.handle, person.handle);
            assert_eq!(found.names.len(), person.names.len());
        }
        for family in plain.families {
            let found = index.family(family.handle.as_str()).unwrap();
            assert_eq!(found.handle, family.handle);
            assert_eq!(found.event_refs.len(), family.event_refs.len());
        }
        for place in plain.places {
            let found = index.place(place.handle.as_str()).unwrap();
            assert_eq!(found.handle, place.handle);
            assert_eq!(found.name, place.name);
        }
    }

    #[test]
    fn data_fixture_resolves_every_event_to_its_primary_person() {
        let db = parse_database(DATA_GRAMPS).unwrap();
        let events = collect_events(&db);
        assert_eq!(events.len(), 6);

        let harry = &events[0];
        assert_eq!(harry.event_type, "Birth");
        assert_eq!(harry.subjects.len(), 1);
        assert_eq!(harry.subjects[0].name, "Harry Meowser");
        assert_eq!(harry.subjects[0].role, "Primary");
        assert_eq!(harry.subjects[0].gramps_id.as_deref(), Some("I0000"));
        assert_eq!(harry.role, "Primary");
        assert!(!harry.orphan);
        assert!(!harry.private);
        assert_eq!(harry.date.as_ref().unwrap().ymd, (2000, 3, 3));
        assert_eq!(harry.gregorian.as_ref().unwrap().to_string(), "2000-03-03");
        assert_eq!(harry.place_path.as_ref().unwrap().join("/"), "England");

        assert_eq!(events[1].subjects[0].name, "George Meowser");
        assert_eq!(events[2].subjects[0].name, "Sally Furball");
        assert_eq!(events[3].subjects[0].name, "Mark Hairball");
        assert_eq!(events[3].place_path.as_ref().unwrap().join("/"), "Furmany");
        assert_eq!(events[4].subjects[0].name, "Abraham Meowser");
        assert_eq!(events[4].place_path.as_ref().unwrap().join("/"), "Ur");

        let death = &events[5];
        assert_eq!(death.event_type, "Death");
        assert_eq!(death.subjects[0].name, "Abraham Meowser");
        assert_eq!(death.gregorian.as_ref().unwrap().to_string(), "2020-12-03");
        assert_eq!(death.place_path.as_ref().unwrap().join("/"), "Ur");

        assert!(events.iter().all(|event| !event.orphan));
        assert!(events.iter().all(|event| event.subjects.len() == 1));
    }

    #[test]
    fn family_fixture_applies_role_precedence_and_couples() {
        let db = parse_database(FAMILIES_GRAMPS).unwrap();
        let events = collect_events(&db);
        assert_eq!(events.len(), 8);

        // (c) family-owned events resolve to the couple — divorce does not
        // suppress the marriage or its alternative (decision D5).
        assert_eq!(events[0].event_type, "Marriage");
        assert_eq!(events[0].subjects.len(), 2);
        assert_eq!(events[0].subjects[0].name, "Adam Uplands");
        assert_eq!(events[0].subjects[1].name, "Eve Uplands");
        assert_eq!(events[0].subjects[0].role, "Family");
        assert_eq!(events[0].subjects[1].role, "Family");
        assert_eq!(events[0].role, "Family");
        assert_eq!(events[2].event_type, "Divorce");
        assert_eq!(events[2].subjects.len(), 2);
        assert!(!events[0].orphan);

        // (a) one event, two Primary eventrefs → one subject per person.
        assert_eq!(events[3].subjects.len(), 2);
        assert_eq!(events[3].subjects[0].name, "Adam Uplands");
        assert_eq!(events[3].subjects[1].name, "Eve Uplands");
        assert!(
            events[3]
                .subjects
                .iter()
                .all(|subject| subject.role == "Primary")
        );

        // (a) beats (b): while a Primary ref exists, Eve's Witness-only
        // ref does not add a subject.
        assert_eq!(events[4].subjects.len(), 1);
        assert_eq!(events[4].subjects[0].name, "Adam Uplands");
        assert_eq!(events[4].subjects[0].role, "Primary");

        // (b) no Primary ref at all → the any-role person becomes the subject.
        assert_eq!(events[5].subjects.len(), 1);
        assert_eq!(events[5].subjects[0].name, "Adam Uplands");
        assert_eq!(events[5].subjects[0].role, "Witness");
        assert_eq!(events[5].role, "Witness");

        // (e) orphan events keep a "—" subject and the orphan flag.
        for orphan in events.iter().skip(6) {
            assert!(orphan.orphan);
            assert_eq!(orphan.subjects.len(), 1);
            assert_eq!(orphan.subjects[0].name, "—");
            assert_eq!(orphan.subjects[0].role, "");
            assert_eq!(orphan.role, "");
        }
    }

    #[test]
    fn crafted_fixture_covers_dedup_prefixes_single_spouse_and_places() {
        let db = parse_database(RESOLUTION_XML).unwrap();
        let events = collect_events(&db);
        assert_eq!(events.len(), 5);

        // One person holding Primary + Witness refs to the same event
        // resolves once, with the Primary role winning (event-level dedup).
        let birth = &events[0];
        assert_eq!(birth.subjects.len(), 1);
        assert_eq!(birth.subjects[0].name, "Vanessa van der Berg");
        assert_eq!(birth.subjects[0].role, "Primary");
        assert_eq!(birth.subjects[0].gramps_id.as_deref(), Some("I0"));
        assert_eq!(birth.gregorian.as_ref().unwrap().to_string(), "1970-01-01");

        // (c) the couple is father + mother; the child is not a subject.
        let marriage = &events[1];
        assert_eq!(marriage.subjects.len(), 2);
        assert_eq!(marriage.subjects[0].name, "Odin Allfather");
        assert_eq!(marriage.subjects[1].name, "Frigg Allmother");
        assert_eq!(marriage.subjects[0].role, "Family");
        assert_eq!(marriage.role, "Family");

        // (d) two families reference the same single-spouse event: the
        // shared spouse appears exactly once.
        let death = &events[2];
        assert!(!death.orphan);
        assert_eq!(death.subjects.len(), 1);
        assert_eq!(death.subjects[0].name, "Odin Allfather");
        assert_eq!(death.subjects[0].role, "Family");
        assert_eq!(
            death.place_path.as_ref().unwrap().join("/"),
            "Uppsala/Sweden"
        );

        // A nameless Primary person renders ""; a cyclic place hierarchy
        // terminates instead of hanging.
        let burial = &events[3];
        assert!(!burial.orphan);
        assert_eq!(burial.subjects.len(), 1);
        assert_eq!(burial.subjects[0].name, "");
        assert_eq!(burial.place_path.as_ref().unwrap().join("/"), "Mobius/Loop");

        // An unresolvable place handle yields no path; the unreferenced
        // event is an orphan.
        let baptism = &events[4];
        assert!(baptism.orphan);
        assert_eq!(baptism.subjects.len(), 1);
        assert_eq!(baptism.subjects[0].name, "—");
        assert_eq!(baptism.place_path, None);
    }

    #[test]
    fn primary_name_skips_alternate_names() {
        let db = parse_database(RESOLUTION_XML).unwrap();
        let navn = db
            .people
            .iter()
            .find(|p| p.handle.as_str() == "_navn")
            .unwrap();
        assert_eq!(render_person_name(navn), "Nameless Cat");
        let nameless = db
            .people
            .iter()
            .find(|p| p.handle.as_str() == "_nameless")
            .unwrap();
        assert_eq!(render_person_name(nameless), "");
    }
}
