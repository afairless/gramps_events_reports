//! XML document → [`Database`] mapping.
//!
//! The Gramps DTD is the spec: a `<database>` root with fixed-order
//! sections, every primary record carrying a unique `handle` (an XML `ID`,
//! hence unique per document). This module walks the document with
//! `roxmltree` and mirrors the sections the v1 pipeline consumes — header,
//! events, people, families, places — skipping everything else (objects,
//! notes, sources, `future-*` sections ...) for forward compatibility.
//!
//! **Parse-boundary rules** (plan §7.1):
//!
//! - A record missing its `handle`, or two records sharing one, is a *hard*
//!   error naming the record type/position — enforced via the
//!   [`HandleIndex`] built while walking the document.
//! - A `daterange`/`datespan` whose `stop` sorts before `start` is a *hard*
//!   [`GrampsXmlError::InvalidDate`].
//! - Any other malformed known field (bad `change` timestamp, malformed
//!   date value) *warns and skips the record* so the rest of the file stays
//!   readable; each occurrence is appended to [`Database::warnings`].
//!
//! Namespace tolerance: the exporter writes a fixed namespace
//! (`http://gramps-project.org/xml/1.7.x/`); matching uses the local tag
//! name only, so foreign or unversioned namespaces parse identically.

use std::collections::HashMap;

use gramps_dates::parse::{
    DateRangeAttrs, DatevalAttrs, parse_daterange, parse_datespan, parse_datestr, parse_dateval,
};
use gramps_dates::{DateError, GrampsDate};
use roxmltree::Node;

use crate::decode_container;
use crate::error::GrampsXmlError;
use crate::model::{Database, Event, Family, Header, Person, Place};

/// Parse a `.gramps` file: detect the container, decode it, and map the
/// XML document onto a typed [`Database`].
///
/// This is the crate's single entry point — container detection and
/// decoding live in [`super::detect_container`] / [`super::decode_container`];
/// XML structure errors surface from here once the document is parsed.
///
/// ```
/// use gramps_xml::parse_database;
///
/// let xml = br#"<database><events>
///   <event handle="_e0" id="E0000"><type>Birth</type>
///     <dateval val="2000-03-03"/></event>
/// </events></database>"#;
/// let db = parse_database(xml).unwrap();
/// assert_eq!(db.events.len(), 1);
/// assert_eq!(db.events[0].date.as_ref().unwrap().ymd, (2000, 3, 3));
/// ```
pub fn parse_database(bytes: &[u8]) -> Result<Database, GrampsXmlError> {
    let xml = decode_container(bytes)?;
    parse_document(&xml)
}

/// Map a well-formed XML document onto a [`Database`].
///
/// Reached only after container decoding turns the bytes into UTF-8 text;
/// parse_database routes here.
pub(crate) fn parse_document(xml: &str) -> Result<Database, GrampsXmlError> {
    let doc = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            // Real Gramps exports always carry the `<!DOCTYPE database ...>`
            // declaration; the DTD itself is not needed for parsing.
            allow_dtd: true,
            ..Default::default()
        },
    )?;
    // `Document::parse` rejects rootless documents (its error surfaces as
    // `GrampsXmlError::Xml`), so `root_element` cannot panic here; only the
    // root-tag check remains.
    let root = doc.root_element();
    if root.tag_name().name() != "database" {
        return Err(GrampsXmlError::MissingRoot);
    }

    let mut header = Header::default();
    let mut events = Vec::new();
    let mut people = Vec::new();
    let mut families = Vec::new();
    let mut places = Vec::new();
    let mut warnings = Vec::new();
    let mut index = HandleIndex::default();

    for section in element_children(root) {
        match section.tag_name().name() {
            "header" => parse_header(section, &mut header),
            "events" => parse_events(section, &mut index, &mut warnings, &mut events)?,
            "people" => parse_people(section, &mut index, &mut people)?,
            "families" => parse_families(section, &mut index, &mut families)?,
            "places" => parse_places(section, &mut index, &mut places)?,
            // Unknown or not-yet-modeled sections (objects, notes, sources,
            // citations, tags, bookmarks, `future-*`...) are tolerated.
            _ => {}
        }
    }

    Ok(Database {
        header,
        events,
        people,
        families,
        places,
        warnings,
    })
}

/// Parse-time handle index.
///
/// Every primary record carries a DTD `ID`-typed `handle`, which must be
/// present and unique within the document. Walking the sections, this index
/// records which record kind owns each handle, so both violations surface
/// as hard errors naming the record type/position (plan §7.1). The
/// *resolution* index (handle → record, used to resolve links) is built
/// later, in `event-core`.
#[derive(Debug, Default)]
struct HandleIndex {
    /// handle → record kind ("event", "person", ...).
    handles: HashMap<String, &'static str>,
}

impl HandleIndex {
    /// Claim a handle for a record. Errors if the handle is missing or
    /// already claimed by another record.
    fn insert(
        &mut self,
        handle: &str,
        record: &'static str,
        position: usize,
    ) -> Result<(), GrampsXmlError> {
        if handle.is_empty() {
            return Err(GrampsXmlError::MissingHandle { record, position });
        }
        if let Some(first) = self.handles.get(handle) {
            return Err(GrampsXmlError::DuplicateHandle {
                handle: handle.to_string(),
                first,
                second: record,
            });
        }
        self.handles.insert(handle.to_string(), record);
        Ok(())
    }
}

/// Parse the `<header>` section: the `created` stamp only; `researcher` and
/// friends land in the full-record milestone.
fn parse_header(node: Node, header: &mut Header) {
    for child in element_children(node) {
        if child.tag_name().name() == "created" {
            header.created_date = child.attribute("date").map(str::to_string);
            header.version = child.attribute("version").map(str::to_string);
            return;
        }
    }
}

fn parse_events(
    node: Node,
    index: &mut HandleIndex,
    warnings: &mut Vec<String>,
    out: &mut Vec<Event>,
) -> Result<(), GrampsXmlError> {
    for (position, record) in event_records(node).enumerate() {
        match parse_event(record, index, warnings, position) {
            Ok(Some(event)) => out.push(event),
            Ok(None) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn parse_people(
    node: Node,
    index: &mut HandleIndex,
    out: &mut Vec<Person>,
) -> Result<(), GrampsXmlError> {
    for (position, record) in element_children(node)
        .filter(|c| c.tag_name().name() == "person")
        .enumerate()
    {
        let handle = required_handle(record, "person", position)?;
        index.insert(handle, "person", position)?;
        out.push(Person {
            handle: handle.to_string(),
            gramps_id: record.attribute("id").map(str::to_string),
        });
    }
    Ok(())
}

fn parse_families(
    node: Node,
    index: &mut HandleIndex,
    out: &mut Vec<Family>,
) -> Result<(), GrampsXmlError> {
    for (position, record) in element_children(node)
        .filter(|c| c.tag_name().name() == "family")
        .enumerate()
    {
        let handle = required_handle(record, "family", position)?;
        index.insert(handle, "family", position)?;
        out.push(Family {
            handle: handle.to_string(),
            gramps_id: record.attribute("id").map(str::to_string),
        });
    }
    Ok(())
}

fn parse_places(
    node: Node,
    index: &mut HandleIndex,
    out: &mut Vec<Place>,
) -> Result<(), GrampsXmlError> {
    for (position, record) in element_children(node)
        .filter(|c| c.tag_name().name() == "placeobj")
        .enumerate()
    {
        let handle = required_handle(record, "place", position)?;
        index.insert(handle, "place", position)?;
        let name = element_children(record)
            .find(|c| c.tag_name().name() == "pname")
            .and_then(|c| c.attribute("value"))
            .unwrap_or("")
            .to_string();
        out.push(Place {
            handle: handle.to_string(),
            gramps_id: record.attribute("id").map(str::to_string),
            name,
            place_type: record.attribute("type").map(str::to_string),
        });
    }
    Ok(())
}

/// Parse one `<event>` record.
///
/// Returns `Ok(None)` when the record is skipped after a recoverable
/// malformed field (warning appended), `Ok(Some(event))` on success, and a
/// hard [`GrampsXmlError`] for handle violations and reversed range/span
/// endpoints.
fn parse_event(
    node: Node,
    index: &mut HandleIndex,
    warnings: &mut Vec<String>,
    position: usize,
) -> Result<Option<Event>, GrampsXmlError> {
    let handle = required_handle(node, "event", position)?;
    let id = node.attribute("id").unwrap_or(handle).to_string();

    let mut event = Event {
        handle: handle.to_string(),
        gramps_id: node.attribute("id").map(str::to_string),
        event_type: String::new(),
        date: None,
        place_handle: None,
        change: 0,
    };

    for child in element_children(node) {
        match child.tag_name().name() {
            "type" => {
                event.event_type = child.text().unwrap_or("").trim().to_string();
            }
            "dateval" | "daterange" | "datespan" | "datestr" => {
                // The DTD allows exactly one date element; keep the first.
                if event.date.is_some() {
                    continue;
                }
                match parse_date_element(child) {
                    Ok(Some(date)) => event.date = Some(date),
                    Ok(None) => {}
                    Err(source) => {
                        if matches!(source, DateError::RangeStartAfterStop(..)) {
                            return Err(GrampsXmlError::InvalidDate { id, source });
                        }
                        warnings.push(format!(
                            "skipping event {id}: malformed {} date ({source})",
                            child.tag_name().name()
                        ));
                        return Ok(None);
                    }
                }
            }
            "place" => {
                event.place_handle = child.attribute("hlink").map(str::to_string);
            }
            // Unknown per-record elements (noteref, citationref, objref,
            // tagref, attribute, description, cause, ...) are tolerated;
            // full-record parsing lands in a later milestone.
            _ => {}
        }
    }

    if let Some(change) = node.attribute("change") {
        match change.parse::<i64>() {
            Ok(value) => event.change = value,
            Err(_) => {
                warnings.push(format!(
                    "skipping event {id}: malformed change attribute {change:?}"
                ));
                return Ok(None);
            }
        }
    }

    index.insert(handle, "event", position)?;
    Ok(Some(event))
}

/// Parse one of the four interchangeable date elements into a [`GrampsDate`].
///
/// `Ok(None)` only for elements other than the four date forms; a matching
/// element always yields a value or a [`DateError`].
fn parse_date_element(node: Node) -> Result<Option<GrampsDate>, DateError> {
    match node.tag_name().name() {
        "dateval" => {
            let val = node.attribute("val").ok_or(DateError::EmptyVal)?;
            let date = parse_dateval(DatevalAttrs {
                val,
                kind: node.attribute("type"),
                quality: node.attribute("quality"),
                cformat: node.attribute("cformat"),
                dualdated: node.attribute("dualdated"),
                newyear: node.attribute("newyear"),
            })?;
            Ok(Some(date))
        }
        "daterange" => parse_compound(node, parse_daterange).map(Some),
        "datespan" => parse_compound(node, parse_datespan).map(Some),
        "datestr" => {
            let val = node.attribute("val").ok_or(DateError::EmptyVal)?;
            Ok(Some(parse_datestr(val)?))
        }
        _ => Ok(None),
    }
}

/// Shared `daterange`/`datespan` wiring: required `start`/`stop` attributes
/// plus the shared optional attributes.
fn parse_compound(
    node: Node,
    parse: fn(DateRangeAttrs<'_>) -> Result<GrampsDate, DateError>,
) -> Result<GrampsDate, DateError> {
    parse(DateRangeAttrs {
        start: node.attribute("start").ok_or(DateError::EmptyVal)?,
        stop: node.attribute("stop").ok_or(DateError::EmptyVal)?,
        quality: node.attribute("quality"),
        cformat: node.attribute("cformat"),
        dualdated: node.attribute("dualdated"),
        newyear: node.attribute("newyear"),
    })
}

/// Read a record's `handle`, failing hard when it is missing. Callers pass
/// the record's position within its section for the error message.
fn required_handle<'a, 'input>(
    node: Node<'a, 'input>,
    record: &'static str,
    position: usize,
) -> Result<&'a str, GrampsXmlError> {
    match node.attribute("handle") {
        Some(handle) if !handle.is_empty() => Ok(handle),
        _ => Err(GrampsXmlError::MissingHandle { record, position }),
    }
}

/// Element children of a node (text, comments and PIs are ignored).
fn element_children<'a, 'input>(
    node: Node<'a, 'input>,
) -> impl Iterator<Item = Node<'a, 'input>> + 'a {
    node.children().filter(|c| c.is_element())
}

/// The `<event>` records of a section, in document order.
fn event_records<'a, 'input>(
    section: Node<'a, 'input>,
) -> impl Iterator<Item = Node<'a, 'input>> + 'a {
    element_children(section).filter(|c| c.tag_name().name() == "event")
}

#[cfg(test)]
mod tests {
    use gramps_dates::Modifier;

    use super::parse_document;
    use crate::{Database, GrampsXmlError};

    const MINIMAL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<database xmlns="http://gramps-project.org/xml/1.7.1/">
  <header>
    <created date="2026-09-09" version="5.1.6"/>
    <researcher/>
  </header>
  <events>
    <event handle="_e0000" change="100" id="E0000">
      <type>Birth</type>
      <dateval val="2000-03-03"/>
      <place hlink="_p0000"/>
    </event>
    <event handle="_e0001" change="101" id="E0001">
      <type>Marriage</type>
      <daterange start="1914-07-28" stop="1918-11-11"/>
    </event>
    <event handle="_e0002" change="102" id="E0002"/>
  </events>
  <people>
    <person handle="_pe000" change="200" id="I0000">
      <gender>M</gender>
      <name type="Birth Name"><first>Harry</first><surname>Meowser</surname></name>
    </person>
  </people>
  <families>
    <family handle="_f0000" change="300" id="F0000">
      <rel type="Unknown"/>
      <father hlink="_pe000"/>
    </family>
  </families>
  <places>
    <placeobj handle="_p0000" change="400" id="P0000" type="Country">
      <pname value="England"/>
    </placeobj>
  </places>
</database>"#;

    fn parse(xml: &str) -> Result<Database, GrampsXmlError> {
        parse_document(xml)
    }

    #[test]
    fn minimal_database_parses_with_all_sections() {
        let db = parse(MINIMAL).unwrap();
        assert_eq!(db.header.created_date.as_deref(), Some("2026-09-09"));
        assert_eq!(db.header.version.as_deref(), Some("5.1.6"));
        assert_eq!(db.events.len(), 3);
        assert_eq!(db.people.len(), 1);
        assert_eq!(db.families.len(), 1);
        assert_eq!(db.places.len(), 1);
        assert!(db.warnings.is_empty());
    }

    #[test]
    fn event_fields_wire_handle_id_type_and_change() {
        let db = parse(MINIMAL).unwrap();
        let event = &db.events[0];
        assert_eq!(event.handle, "_e0000");
        assert_eq!(event.gramps_id.as_deref(), Some("E0000"));
        assert_eq!(event.event_type, "Birth");
        assert_eq!(event.place_handle.as_deref(), Some("_p0000"));
        assert_eq!(event.change, 100);
    }

    #[test]
    fn all_four_date_forms_wire_into_events() {
        let db = parse(MINIMAL).unwrap();
        // dateval
        let date = db.events[0].date.as_ref().unwrap();
        assert_eq!(date.ymd, (2000, 3, 3));
        assert_eq!(date.modifier, Modifier::None);
        assert!(!date.is_range());
        // daterange
        let date = db.events[1].date.as_ref().unwrap();
        assert_eq!(date.modifier, Modifier::Range);
        assert!(date.is_range());
        assert_eq!(date.ymd, (1914, 7, 28));
        assert_eq!(date.stop, Some((1918, 11, 11)));
        // undated event keeps None
        assert!(db.events[2].date.is_none());
    }

    #[test]
    fn event_type_trims_whitespace_and_defaults_empty() {
        let db = parse(MINIMAL).unwrap();
        assert_eq!(db.events[0].event_type, "Birth");
        assert_eq!(db.events[2].event_type, "");
    }

    #[test]
    fn person_family_place_skeletons_parse() {
        let db = parse(MINIMAL).unwrap();
        assert_eq!(db.people[0].handle, "_pe000");
        assert_eq!(db.people[0].gramps_id.as_deref(), Some("I0000"));
        assert_eq!(db.families[0].handle, "_f0000");
        assert_eq!(db.families[0].gramps_id.as_deref(), Some("F0000"));
        assert_eq!(db.places[0].handle, "_p0000");
        assert_eq!(db.places[0].gramps_id.as_deref(), Some("P0000"));
        assert_eq!(db.places[0].name, "England");
        assert_eq!(db.places[0].place_type.as_deref(), Some("Country"));
    }

    #[test]
    fn place_without_pname_defaults_to_empty_name() {
        let db = parse(
            r#"<database><places>
            <placeobj handle="_p0" id="P0" type="City"/>
            </places></database>"#,
        )
        .unwrap();
        assert_eq!(db.places[0].name, "");
        assert_eq!(db.places[0].place_type.as_deref(), Some("City"));
    }

    #[test]
    fn header_defaults_when_absent() {
        let db = parse("<database><events></events></database>").unwrap();
        assert_eq!(db.header.created_date, None);
        assert_eq!(db.header.version, None);
    }

    #[test]
    fn missing_handle_names_record_type_and_position() {
        let err = parse(
            r#"<database><people>
            <person handle="_p0" id="I0000"/>
            <person id="I0001"/>
            </people></database>"#,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            GrampsXmlError::MissingHandle {
                record: "person",
                position: 1
            }
        ));
        let msg = err.to_string();
        assert!(msg.contains("person"), "message: {msg}");
        assert!(msg.contains("position 1"), "message: {msg}");
    }

    #[test]
    fn duplicate_handle_is_a_hard_error_across_sections() {
        let err = parse(
            r#"<database>
            <events><event handle="_dup" id="E0000"/></events>
            <people><person handle="_dup" id="I0000"/></people>
            </database>"#,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            GrampsXmlError::DuplicateHandle {
                ref handle,
                first: "event",
                second: "person"
            } if handle == "_dup"
        ));
        let msg = err.to_string();
        assert!(msg.contains("_dup"), "message: {msg}");
    }

    #[test]
    fn duplicate_handle_within_a_section_is_a_hard_error() {
        let err = parse(
            r#"<database><families>
            <family handle="_f0" id="F0000"/>
            <family handle="_f0" id="F0001"/>
            </families></database>"#,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            GrampsXmlError::DuplicateHandle {
                ref handle,
                first: "family",
                second: "family"
            } if handle == "_f0"
        ));
    }

    #[test]
    fn reversed_range_is_a_hard_error_naming_the_event() {
        let err = parse(
            r#"<database><events>
            <event handle="_e0" id="E0000">
              <daterange start="1918" stop="1914"/>
            </event>
            </events></database>"#,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            GrampsXmlError::InvalidDate { ref id, .. } if id == "E0000"
        ));
        let msg = err.to_string();
        assert!(msg.contains("E0000"), "message: {msg}");
        assert!(msg.contains("1914"), "message: {msg}");
    }

    #[test]
    fn malformed_date_warns_and_skips_the_record() {
        let db = parse(
            r#"<database><events>
            <event handle="_e0" id="E0000"><dateval val="1822-13"/></event>
            <event handle="_e1" id="E0001"><dateval val="1900-01-01"/></event>
            </events></database>"#,
        )
        .unwrap();
        assert_eq!(db.events.len(), 1, "bad-date event must be skipped");
        assert_eq!(db.events[0].handle, "_e1");
        assert_eq!(db.warnings.len(), 1);
        assert!(
            db.warnings[0].contains("E0000"),
            "warning: {}",
            db.warnings[0]
        );
        assert!(
            db.warnings[0].contains("date"),
            "warning: {}",
            db.warnings[0]
        );
    }

    #[test]
    fn malformed_change_warns_and_skips_the_record() {
        let db = parse(
            r#"<database><events>
            <event handle="_e0" id="E0000" change="not-a-number"/>
            <event handle="_e1" id="E0001" change="42"/>
            </events></database>"#,
        )
        .unwrap();
        assert_eq!(db.events.len(), 1);
        assert_eq!(db.events[0].change, 42);
        assert_eq!(db.warnings.len(), 1);
        assert!(
            db.warnings[0].contains("E0000"),
            "warning: {}",
            db.warnings[0]
        );
    }

    #[test]
    fn unknown_sections_and_elements_are_tolerated() {
        let db = parse(
            r#"<database>
            <future-section><marker>ignore me</marker></future-section>
            <events>
              <event handle="_e0" id="E0000" future_attr="x">
                <type>Birth</type>
                <dateval val="1933-03-03"/>
                <favorite-meme>spinning cat</favorite-meme>
              </event>
            </events>
            </database>"#,
        )
        .unwrap();
        assert_eq!(db.events.len(), 1);
        assert_eq!(db.events[0].event_type, "Birth");
        assert!(db.warnings.is_empty());
    }

    #[test]
    fn non_database_root_is_rejected() {
        let err = parse("<not-a-database/>").unwrap_err();
        assert!(matches!(err, GrampsXmlError::MissingRoot));
    }

    #[test]
    fn unnamespaced_database_root_is_accepted() {
        // parse_document is reached only after container decoding; the root
        // element is matched by local name, so an unversioned or foreign
        // namespace still parses.
        assert!(parse("<database/>").is_ok());
    }

    #[test]
    fn real_fixture_parses_without_warnings() {
        let db = parse(include_str!("../../../tests/fixtures/data.gramps")).unwrap();
        assert_eq!(db.events.len(), 6);
        assert_eq!(db.people.len(), 5);
        assert_eq!(db.families.len(), 3);
        assert_eq!(db.places.len(), 4);
        assert_eq!(db.header.version.as_deref(), Some("5.1.6"));
        assert!(db.warnings.is_empty());
    }

    #[test]
    fn edge_case_fixture_parses_tolerantly() {
        let db = parse(include_str!("../../../tests/fixtures/edge-cases.gramps")).unwrap();
        assert_eq!(db.events.len(), 2);
        assert_eq!(db.people.len(), 2);
        assert!(db.warnings.is_empty());
    }

    #[test]
    fn dates_fixture_wires_every_date_form() {
        let db = parse(include_str!("../../../tests/fixtures/dates.gramps")).unwrap();
        assert_eq!(db.events.len(), 25);
        // dateval with modifier
        assert_eq!(
            db.events[3].date.as_ref().unwrap().modifier,
            Modifier::About
        );
        // year-only range 1822–1824
        let range = &db.events[19];
        let date = range.date.as_ref().unwrap();
        assert!(date.is_range());
        assert_eq!(date.ymd, (1822, 0, 0));
        assert_eq!(date.stop, Some((1824, 0, 0)));
        // text-only date
        let text = &db.events[24];
        let date = text.date.as_ref().unwrap();
        assert_eq!(date.modifier, Modifier::TextOnly);
        assert_eq!(date.text(), Some("circa the harvest festival"));
        assert!(db.warnings.is_empty());
    }
}
