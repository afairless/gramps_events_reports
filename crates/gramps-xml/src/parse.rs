//! XML document → [`Database`] mapping.
//!
//! The Gramps DTD is the spec: a `<database>` root with fixed-order
//! sections, every primary record carrying a unique `handle` (an XML `ID`,
//! hence unique per document). This module walks the document with
//! `roxmltree` and mirrors the sections the v1 pipeline consumes — header,
//! tags, events, people, families, places — skipping everything else
//! (objects, notes, sources, `future-*` sections ...) for forward
//! compatibility.
//!
//! Full-record scope (plan §12 step 5): `header` (`created` + `researcher`)
//! and the `tags` section; events carry `cause`/`description`/`priv`;
//! people carry gender, multiple names (surname prefix/`prim`), `eventref`
//! roles and `childof`/`parentin` links; families carry `rel`, spouses,
//! children and `eventref`s; places carry the `placeref` parent chain — all
//! with privacy flags. Unknown elements and attributes are tolerated
//! everywhere (forward compatibility).
//!
//! **Parse-boundary rules** (plan §7.1):
//!
//! - A record missing its `handle`, or two records sharing one, is a *hard*
//!   error naming the record type/position — enforced via the
//!   [`HandleIndex`] built while walking the document.
//! - A malformed date element — a reversed `daterange`/`datespan`, an
//!   invalid value/month/day/modifier/quality/calendar, an empty `datestr`
//!   — *warns and skips the record* like any other recoverable defect, so
//!   the rest of the file stays readable. Each occurrence is appended to
//!   [`Database::warnings`] and recorded structurally in
//!   [`Database::date_issues`] ([`DateIssue`])
//!   for the error report and web UI.
//! - Any other malformed known field (bad `change` timestamp, bad attribute
//!   value) *warns and skips the record*; each occurrence is appended to
//!   [`Database::warnings`].
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
use crate::model::{
    Database, DateIssue, Event, EventRef, Family, Gender, Header, Person, PersonName, Place,
    Surname, Tag,
};

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
    let mut tags = Vec::new();
    let mut events = Vec::new();
    let mut people = Vec::new();
    let mut families = Vec::new();
    let mut places = Vec::new();
    let mut warnings = Vec::new();
    let mut date_issues = Vec::new();
    let mut index = HandleIndex::default();

    for section in element_children(root) {
        match section.tag_name().name() {
            "header" => parse_header(section, &mut header),
            "tags" => parse_tags(section, &mut index, &mut warnings, &mut tags)?,
            "events" => parse_events(
                section,
                &mut index,
                &mut warnings,
                &mut date_issues,
                &mut events,
            )?,
            "people" => parse_people(section, &mut index, &mut warnings, &mut people)?,
            "families" => parse_families(section, &mut index, &mut warnings, &mut families)?,
            "places" => parse_places(section, &mut index, &mut warnings, &mut places)?,
            // Unknown or not-yet-modeled sections (objects, notes, sources,
            // citations, bookmarks, `future-*`...) are tolerated.
            _ => {}
        }
    }

    Ok(Database {
        header,
        tags,
        events,
        people,
        families,
        places,
        warnings,
        date_issues,
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

/// Parse the `<header>` section: the `<created>` stamp and the
/// `<researcher>` name; unknown header children (`mediapath`, `...`) are
/// tolerated.
fn parse_header(node: Node, header: &mut Header) {
    for child in element_children(node) {
        match child.tag_name().name() {
            "created" => {
                header.created_date = child.attribute("date").map(str::to_string);
                header.version = child.attribute("version").map(str::to_string);
            }
            "researcher" => {
                if let Some(resname) = element_children(child)
                    .find(|c| c.tag_name().name() == "resname")
                    .and_then(|c| c.text())
                {
                    header.researcher_name = Some(resname.trim().to_string());
                }
            }
            _ => {}
        }
    }
}

/// Parse the `<tags>` section. `tag` records are primary records: they must
/// carry a unique handle, and their numeric `priority`/`change` attributes
/// are validated; a malformed numeric attribute warns and skips the tag so
/// the rest of the section stays readable.
fn parse_tags(
    node: Node,
    index: &mut HandleIndex,
    warnings: &mut Vec<String>,
    out: &mut Vec<Tag>,
) -> Result<(), GrampsXmlError> {
    for (position, record) in element_children(node)
        .filter(|c| c.tag_name().name() == "tag")
        .enumerate()
    {
        let handle = required_handle(record, "tag", position)?;
        index.insert(handle, "tag", position)?;
        // A malformed tail attribute already claimed the handle and appended
        // a warning; the record itself is skipped.
        if let Some(tag) = parse_tag(record, warnings) {
            out.push(tag);
        }
    }
    Ok(())
}

/// Parse one `<tag>` record's soft fields. Returns `None` (after appending
/// a warning) when a numeric attribute fails to parse.
fn parse_tag(node: Node, warnings: &mut Vec<String>) -> Option<Tag> {
    let handle = node.attribute("handle").unwrap_or("").to_string();
    let priority = match node
        .attribute("priority")
        .and_then(|v| v.parse::<i32>().ok())
    {
        Some(value) => value,
        None => {
            warnings.push(format!(
                "skipping tag {handle}: malformed priority attribute"
            ));
            return None;
        }
    };
    let change = match node.attribute("change").and_then(|v| v.parse::<i64>().ok()) {
        Some(value) => value,
        None => {
            warnings.push(format!("skipping tag {handle}: malformed change attribute"));
            return None;
        }
    };
    Some(Tag {
        handle,
        name: node.attribute("name").unwrap_or("").to_string(),
        color: node.attribute("color").unwrap_or("").to_string(),
        priority,
        change,
    })
}

fn parse_events(
    node: Node,
    index: &mut HandleIndex,
    warnings: &mut Vec<String>,
    date_issues: &mut Vec<DateIssue>,
    out: &mut Vec<Event>,
) -> Result<(), GrampsXmlError> {
    for (position, record) in event_records(node).enumerate() {
        match parse_event(record, index, warnings, date_issues, position) {
            Ok(Some(event)) => out.push(event),
            Ok(None) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// Parse a `<person>` record — full record: gender, names, eventref roles,
/// `childof`/`parentin` family links and the privacy flag (plan §12 step 5).
///
/// Returns `Ok(None)` when the record is skipped after a recoverable
/// malformed field (typically a `gender` that is not `M`/`F`/`U`), with the
/// warning appended; handle violations stay hard errors.
fn parse_person(
    node: Node,
    index: &mut HandleIndex,
    warnings: &mut Vec<String>,
    position: usize,
) -> Result<Option<Person>, GrampsXmlError> {
    let handle = required_handle(node, "person", position)?;
    let id = node.attribute("id").unwrap_or(handle).to_string();

    let mut gender = None;
    let mut names = Vec::new();
    let mut event_refs = Vec::new();
    let mut child_of = Vec::new();
    let mut parent_in = Vec::new();

    for child in element_children(node) {
        match child.tag_name().name() {
            "gender" => {
                let raw = child.text().unwrap_or("").trim();
                gender = Some(match raw {
                    "M" => Gender::Male,
                    "F" => Gender::Female,
                    "U" => Gender::Unknown,
                    other => {
                        warnings.push(format!("skipping person {id}: malformed gender {other:?}"));
                        return Ok(None);
                    }
                });
            }
            "name" => names.push(parse_name(child)),
            "eventref" => {
                if let Some(ev) = parse_event_ref(child, &format!("person {id}"), warnings) {
                    event_refs.push(ev);
                }
            }
            "childof" => {
                if let Some(h) = parse_hlink(child, &format!("person {id}"), "childof", warnings) {
                    child_of.push(h);
                }
            }
            "parentin" => {
                if let Some(h) = parse_hlink(child, &format!("person {id}"), "parentin", warnings) {
                    parent_in.push(h);
                }
            }
            // Unknown per-record elements (objref, address, attribute, url,
            // lds_ord, personref, noteref, citationref, tagref, ...) are
            // tolerated.
            _ => {}
        }
    }

    index.insert(handle, "person", position)?;
    Ok(Some(Person {
        handle: handle.to_string(),
        gramps_id: node.attribute("id").map(str::to_string),
        // The DTD makes `gender` required; a well-formed-but-minimal record
        // without one degrades to Unknown instead of being skipped.
        gender: gender.unwrap_or(Gender::Unknown),
        names,
        event_refs,
        child_of,
        parent_in,
        private: node.attribute("priv") == Some("1"),
    }))
}

/// Parse one `<name>` element into a [`PersonName`]. Names carry only soft
/// fields, so parsing never fails — unknown children (`call`, `title`,
/// `nick`, notes, ...) are tolerated.
fn parse_name(node: Node) -> PersonName {
    let mut name = PersonName {
        name_type: node.attribute("type").unwrap_or("").to_string(),
        alt: node.attribute("alt") == Some("1"),
        first: None,
        surnames: Vec::new(),
        suffix: None,
    };
    for child in element_children(node) {
        match child.tag_name().name() {
            "first" => name.first = child.text().map(str::trim).map(str::to_string),
            "surname" => name.surnames.push(Surname {
                value: child.text().unwrap_or("").trim().to_string(),
                prefix: child.attribute("prefix").map(str::to_string),
                prim: child.attribute("prim") == Some("1"),
            }),
            "suffix" => name.suffix = child.text().map(str::trim).map(str::to_string),
            _ => {}
        }
    }
    name
}

/// Parse one `<eventref>` link (on a person or a family). A missing `hlink`
/// is a recoverable defect: the link is dropped with a warning and the
/// owning record is kept.
fn parse_event_ref(node: Node, owner: &str, warnings: &mut Vec<String>) -> Option<EventRef> {
    match node.attribute("hlink") {
        Some(hlink) => Some(EventRef {
            event_handle: hlink.to_string(),
            role: node.attribute("role").unwrap_or("").to_string(),
        }),
        None => {
            warnings.push(format!("skipping eventref on {owner}: missing hlink"));
            None
        }
    }
}

/// Read a bare `hlink` reference element (`childof`, `parentin`, ...),
/// warning and dropping the link (but keeping the record) when the
/// reference is absent.
fn parse_hlink(node: Node, owner: &str, kind: &str, warnings: &mut Vec<String>) -> Option<String> {
    match node.attribute("hlink") {
        Some(hlink) => Some(hlink.to_string()),
        None => {
            warnings.push(format!("skipping {kind} on {owner}: missing hlink"));
            None
        }
    }
}

fn parse_people(
    node: Node,
    index: &mut HandleIndex,
    warnings: &mut Vec<String>,
    out: &mut Vec<Person>,
) -> Result<(), GrampsXmlError> {
    for (position, record) in element_children(node)
        .filter(|c| c.tag_name().name() == "person")
        .enumerate()
    {
        match parse_person(record, index, warnings, position) {
            Ok(Some(person)) => out.push(person),
            Ok(None) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn parse_families(
    node: Node,
    index: &mut HandleIndex,
    warnings: &mut Vec<String>,
    out: &mut Vec<Family>,
) -> Result<(), GrampsXmlError> {
    for (position, record) in element_children(node)
        .filter(|c| c.tag_name().name() == "family")
        .enumerate()
    {
        let handle = required_handle(record, "family", position)?;
        index.insert(handle, "family", position)?;
        let id = record.attribute("id").unwrap_or(handle).to_string();

        let mut family = Family {
            handle: handle.to_string(),
            gramps_id: record.attribute("id").map(str::to_string),
            rel: None,
            father: None,
            mother: None,
            children: Vec::new(),
            event_refs: Vec::new(),
            private: record.attribute("priv") == Some("1"),
        };
        for child in element_children(record) {
            match child.tag_name().name() {
                "rel" => family.rel = child.attribute("type").map(str::to_string),
                "father" => {
                    family.father = match child.attribute("hlink") {
                        Some(hlink) => Some(hlink.to_string()),
                        None => {
                            warnings.push(format!("skipping father on family {id}: missing hlink"));
                            None
                        }
                    };
                }
                "mother" => {
                    family.mother = match child.attribute("hlink") {
                        Some(hlink) => Some(hlink.to_string()),
                        None => {
                            warnings.push(format!("skipping mother on family {id}: missing hlink"));
                            None
                        }
                    };
                }
                "childref" => {
                    if let Some(h) =
                        parse_hlink(child, &format!("family {id}"), "childref", warnings)
                    {
                        family.children.push(h);
                    }
                }
                "eventref" => {
                    if let Some(ev) = parse_event_ref(child, &format!("family {id}"), warnings) {
                        family.event_refs.push(ev);
                    }
                }
                // Unknown per-record elements (lds_ord, objref, attribute,
                // noteref, citationref, tagref, ...) are tolerated.
                _ => {}
            }
        }
        out.push(family);
    }
    Ok(())
}

fn parse_places(
    node: Node,
    index: &mut HandleIndex,
    warnings: &mut Vec<String>,
    out: &mut Vec<Place>,
) -> Result<(), GrampsXmlError> {
    for (position, record) in element_children(node)
        .filter(|c| c.tag_name().name() == "placeobj")
        .enumerate()
    {
        let handle = required_handle(record, "place", position)?;
        index.insert(handle, "place", position)?;

        let mut name = String::new();
        let mut parent_handle = None;
        for child in element_children(record) {
            match child.tag_name().name() {
                "pname" if name.is_empty() => {
                    if let Some(value) = child.attribute("value") {
                        name = value.to_string();
                    }
                }
                "placeref" if parent_handle.is_none() => {
                    parent_handle = match child.attribute("hlink") {
                        Some(hlink) => Some(hlink.to_string()),
                        None => {
                            warnings.push(format!(
                                "skipping placeref on place {handle}: missing hlink"
                            ));
                            None
                        }
                    };
                }
                // Unknown per-record elements (ptitle, code, coord,
                // location, objref, url, noteref, citationref, tagref, ...)
                // are tolerated.
                _ => {}
            }
        }
        out.push(Place {
            handle: handle.to_string(),
            gramps_id: record.attribute("id").map(str::to_string),
            name,
            place_type: record.attribute("type").map(str::to_string),
            parent_handle,
            private: record.attribute("priv") == Some("1"),
        });
    }
    Ok(())
}

/// Parse one `<event>` record.
///
/// Returns `Ok(None)` when the record is skipped after a recoverable
/// malformed field (warning appended; a broken date element also records a
/// [`DateIssue`] into `date_issues`), `Ok(Some(event))` on success, and a
/// hard [`GrampsXmlError`] only for handle violations.
///
/// Date-element errors are **deferred**: the child loop keeps running after
/// a broken date element so a `<type>` following it is still captured, and
/// the event is skipped only once the loop finishes. When an event carries
/// both a broken date and a malformed `change`, the date takes precedence:
/// the event is skipped for the date with no second warning.
fn parse_event(
    node: Node,
    index: &mut HandleIndex,
    warnings: &mut Vec<String>,
    date_issues: &mut Vec<DateIssue>,
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
        cause: None,
        description: None,
        private: node.attribute("priv") == Some("1"),
        change: 0,
    };
    // The first date element that failed to parse, if any. Filled with the
    // event type after the child loop so a following `<type>` contributes;
    // a second broken date element is ignored (only one is recorded).
    let mut pending_date_issue: Option<DateIssue> = None;

    for child in element_children(node) {
        match child.tag_name().name() {
            "type" => {
                event.event_type = child.text().unwrap_or("").trim().to_string();
            }
            "dateval" | "daterange" | "datespan" | "datestr" => {
                // The DTD allows exactly one date element; keep the first
                // (and once one has failed, the event is skipped anyway).
                if event.date.is_some() || pending_date_issue.is_some() {
                    continue;
                }
                match parse_date_element(child) {
                    Ok(Some(date)) => event.date = Some(date),
                    Ok(None) => {}
                    Err(source) => {
                        pending_date_issue = Some(DateIssue {
                            event_handle: handle.to_string(),
                            event_id: id.clone(),
                            event_type: String::new(),
                            date_kind: child.tag_name().name().to_string(),
                            message: source.to_string(),
                        });
                    }
                }
            }
            "place" => {
                event.place_handle = match child.attribute("hlink") {
                    Some(hlink) => Some(hlink.to_string()),
                    None => {
                        warnings.push(format!("skipping place on event {id}: missing hlink"));
                        None
                    }
                };
            }
            "cause" => event.cause = child.text().map(str::trim).map(str::to_string),
            "description" => {
                event.description = child.text().map(str::trim).map(str::to_string);
            }
            // Unknown per-record elements (attribute, note/tag/citation
            // refs, objref, ...) are tolerated (forward compatibility).
            _ => {}
        }
    }

    // A broken date element defers the skip until here, so the event type
    // read after it is captured; the date issue takes precedence over a
    // malformed `change`, which would otherwise warn and skip too.
    if let Some(mut issue) = pending_date_issue {
        issue.event_type = event.event_type.clone();
        warnings.push(format!(
            "skipping event {id}: malformed {} date ({})",
            issue.date_kind, issue.message
        ));
        date_issues.push(issue);
        return Ok(None);
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
    fn reversed_range_warns_and_skips_and_is_reported() {
        let db = parse(
            r#"<database><events>
            <event handle="_e0" id="E0000">
              <daterange start="1918" stop="1914"/>
            </event>
            <event handle="_e1" id="E0001">
              <datespan start="2000-01-01" stop="1999-12-31"/>
            </event>
            <event handle="_e2" id="E0002"><dateval val="1950-01-15"/></event>
            </events></database>"#,
        )
        .unwrap();
        // Reversed endpoints are soft defects now: the damaged events are
        // skipped, the well-formed one is kept, the parse succeeds, and each
        // skip is both warned about and reported structurally.
        assert_eq!(db.events.len(), 1);
        assert_eq!(db.events[0].handle, "_e2");
        assert_eq!(db.date_issues.len(), 2);
        assert_eq!(db.date_issues[0].event_handle, "_e0");
        assert_eq!(db.date_issues[0].event_id, "E0000");
        assert_eq!(db.date_issues[0].date_kind, "daterange");
        assert!(
            db.date_issues[0].message.contains("1914"),
            "message: {}",
            db.date_issues[0].message
        );
        assert_eq!(db.date_issues[1].event_handle, "_e1");
        assert_eq!(db.date_issues[1].event_id, "E0001");
        assert_eq!(db.date_issues[1].date_kind, "datespan");
        assert_eq!(db.warnings.len(), 2);
        assert!(
            db.warnings[0].contains("E0000"),
            "warning: {}",
            db.warnings[0]
        );
        assert!(
            db.warnings[1].contains("E0001"),
            "warning: {}",
            db.warnings[1]
        );
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
        assert_eq!(db.date_issues.len(), 1);
        assert_eq!(db.date_issues[0].event_id, "E0000");
        assert_eq!(db.date_issues[0].date_kind, "dateval");
        assert!(
            db.date_issues[0].message.contains("month 13"),
            "message: {}",
            db.date_issues[0].message
        );
    }

    #[test]
    fn mixed_reversed_range_and_invalid_value_are_all_reported() {
        let db = parse(
            r#"<database><events>
            <event handle="_e0" id="E0000">
              <daterange start="1918" stop="1914"/>
            </event>
            <event handle="_e1" id="E0001"><dateval val="1822-13"/></event>
            <event handle="_e2" id="E0002"><dateval val="1900-01-01"/></event>
            </events></database>"#,
        )
        .unwrap();
        assert_eq!(db.events.len(), 1, "both damaged events must be skipped");
        assert_eq!(db.events[0].handle, "_e2");
        assert_eq!(db.warnings.len(), 2);
        assert_eq!(db.date_issues.len(), 2);
        assert_eq!(db.date_issues[0].event_id, "E0000");
        assert_eq!(db.date_issues[0].date_kind, "daterange");
        assert_eq!(db.date_issues[1].event_id, "E0001");
        assert_eq!(db.date_issues[1].date_kind, "dateval");
    }

    #[test]
    fn event_type_is_captured_when_type_follows_the_broken_date() {
        let db = parse(
            r#"<database><events>
            <event handle="_e0" id="E0000">
              <daterange start="1918" stop="1914"/>
              <type>Death</type>
            </event>
            </events></database>"#,
        )
        .unwrap();
        assert!(db.events.is_empty());
        assert_eq!(db.date_issues.len(), 1);
        assert_eq!(db.date_issues[0].event_id, "E0000");
        assert_eq!(db.date_issues[0].event_type, "Death");
    }

    #[test]
    fn date_issue_takes_precedence_over_malformed_change() {
        let db = parse(
            r#"<database><events>
            <event handle="_e0" id="E0000" change="not-a-number">
              <daterange start="1918" stop="1914"/>
            </event>
            </events></database>"#,
        )
        .unwrap();
        // Skipped for the date only: one warning naming the date, one
        // structured issue, and no second warning about the change.
        assert!(db.events.is_empty());
        assert_eq!(db.warnings.len(), 1);
        assert!(
            db.warnings[0].contains("date"),
            "warning: {}",
            db.warnings[0]
        );
        assert_eq!(db.date_issues.len(), 1);
        assert_eq!(db.date_issues[0].event_id, "E0000");
        assert_eq!(db.date_issues[0].date_kind, "daterange");
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

    // ---------------------------------------------------------------------
    // Full-record parsing (plan step 5): names, eventref roles, family
    // members, place hierarchy, privacy flags, header/tags.
    // ---------------------------------------------------------------------

    const FULL: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<database xmlns="http://gramps-project.org/xml/1.7.1/">
  <header>
    <created date="2026-09-29" version="5.1.6"/>
    <researcher><resname>Dr. Whiskers &amp; Sons</resname></researcher>
  </header>
  <tags>
    <tag handle="_t0000" name="ToDo" color="#fb9408" priority="0" change="10"/>
    <tag handle="_t0001" name="Research" color="#2e76b6" priority="1" change="11"/>
  </tags>
  <events>
    <event handle="_e0000" change="100" id="E0000" priv="1">
      <type>Birth</type>
      <dateval val="2000-03-03"/>
      <place hlink="_p0000"/>
      <cause>custom cause</cause>
      <description>firstborn</description>
    </event>
    <event handle="_e0001" change="101" id="E0001">
      <type>Marriage</type>
      <dateval val="1955-06-12"/>
    </event>
  </events>
  <people>
    <person handle="_pe000" change="200" id="I0000" priv="1">
      <gender>M</gender>
      <name type="Birth Name">
        <first>Harry</first>
        <surname prefix="van der" prim="1">Meowser</surname>
      </name>
      <name type="Married Name" alt="1">
        <first>Harry</first>
        <surname prim="0">Hairball</surname>
        <surname prim="1">Meowser</surname>
      </name>
      <eventref hlink="_e0000" role="Primary"/>
      <eventref hlink="_e0001" role="Witness"/>
      <childof hlink="_f0000"/>
      <parentin hlink="_f0001"/>
    </person>
    <person handle="_pe001" change="201" id="I0001">
      <gender>F</gender>
      <name type="Birth Name">
        <first>Sally</first>
        <surname>Furball</surname>
      </name>
      <eventref hlink="_e0001" role="Primary"/>
    </person>
  </people>
  <families>
    <family handle="_f0000" change="300" id="F0000">
      <rel type="Marriage"/>
      <father hlink="_pe000"/>
      <mother hlink="_pe001"/>
      <childref hlink="_pe002"/>
      <eventref hlink="_e0001" role="Family"/>
    </family>
    <family handle="_f0001" change="301" id="F0001">
      <rel type="Unknown"/>
    </family>
  </families>
  <places>
    <placeobj handle="_p0000" change="400" id="P0000" type="City">
      <pname value="Uppsala"/>
      <placeref hlink="_p0001"/>
    </placeobj>
    <placeobj handle="_p0001" change="401" id="P0001" type="Country" priv="1">
      <pname value="Sweden"/>
    </placeobj>
  </places>
</database>"##;

    #[test]
    fn person_names_parse_multiple_names_with_surname_prefix_and_prim() {
        let db = parse(FULL).unwrap();
        let person = &db.people[0];
        assert_eq!(person.names.len(), 2, "primary + married name");
        let primary = &person.names[0];
        assert_eq!(primary.name_type, "Birth Name");
        assert!(!primary.alt);
        assert_eq!(primary.first.as_deref(), Some("Harry"));
        assert_eq!(primary.surnames.len(), 1);
        assert_eq!(primary.surnames[0].value, "Meowser");
        assert_eq!(primary.surnames[0].prefix.as_deref(), Some("van der"));
        assert!(primary.surnames[0].prim);
        assert_eq!(primary.suffix, None);
        let married = &person.names[1];
        assert_eq!(married.name_type, "Married Name");
        assert!(married.alt);
        assert_eq!(married.surnames.len(), 2);
        assert!(!married.surnames[0].prim);
        assert!(married.surnames[1].prim);
    }

    #[test]
    fn person_without_names_stays_empty() {
        let db = parse(
            r#"<database><people>
            <person handle="_p0" id="I0000"><gender>U</gender></person>
            </people></database>"#,
        )
        .unwrap();
        assert!(db.people[0].names.is_empty());
        assert!(db.warnings.is_empty());
    }

    #[test]
    fn eventref_roles_parse_on_people_and_families() {
        let db = parse(FULL).unwrap();
        let person = &db.people[0];
        assert_eq!(person.event_refs.len(), 2);
        assert_eq!(person.event_refs[0].event_handle, "_e0000");
        assert_eq!(person.event_refs[0].role, "Primary");
        assert_eq!(person.event_refs[1].role, "Witness");
        // second person: single Primary link
        assert_eq!(db.people[1].event_refs.len(), 1);
        assert_eq!(db.people[1].event_refs[0].role, "Primary");
        // marriage event hangs off the family, role "Family"
        let family = &db.families[0];
        assert_eq!(family.event_refs.len(), 1);
        assert_eq!(family.event_refs[0].event_handle, "_e0001");
        assert_eq!(family.event_refs[0].role, "Family");
    }

    #[test]
    fn family_members_and_children_parse() {
        let db = parse(FULL).unwrap();
        let family = &db.families[0];
        assert_eq!(family.rel.as_deref(), Some("Marriage"));
        assert_eq!(family.father.as_deref(), Some("_pe000"));
        assert_eq!(family.mother.as_deref(), Some("_pe001"));
        assert_eq!(family.children, vec!["_pe002"]);
        // family with no spouses keeps None
        assert_eq!(db.families[1].father, None);
        assert_eq!(db.families[1].mother, None);
        assert!(db.families[1].children.is_empty());
    }

    #[test]
    fn person_childof_parentin_links_parse() {
        let db = parse(FULL).unwrap();
        let person = &db.people[0];
        assert_eq!(person.child_of, vec!["_f0000"]);
        assert_eq!(person.parent_in, vec!["_f0001"]);
        assert!(db.people[1].child_of.is_empty());
        assert!(db.people[1].parent_in.is_empty());
    }

    #[test]
    fn gender_markers_parse() {
        let db = parse(FULL).unwrap();
        assert_eq!(db.people[0].gender, crate::Gender::Male);
        assert_eq!(db.people[1].gender, crate::Gender::Female);
    }

    #[test]
    fn missing_gender_defaults_to_unknown() {
        let db = parse(
            r#"<database><people><person handle="_p0" id="I0000">
            <name type="Birth Name"><first>Quiet</first><surname>Case</surname></name>
            </person></people></database>"#,
        )
        .unwrap();
        assert_eq!(db.people[0].gender, crate::Gender::Unknown);
        assert!(db.warnings.is_empty());
    }

    #[test]
    fn malformed_gender_warns_and_skips_the_person() {
        let db = parse(
            r#"<database><people>
            <person handle="_p0" id="I0000"><gender>X</gender></person>
            <person handle="_p1" id="I0001"><gender>M</gender>
              <name type="Birth Name"><first>Ok</first><surname>Person</surname></name>
            </person>
            </people></database>"#,
        )
        .unwrap();
        assert_eq!(db.people.len(), 1);
        assert_eq!(db.people[0].handle, "_p1");
        assert_eq!(db.warnings.len(), 1);
        assert!(
            db.warnings[0].contains("I0000"),
            "warning: {}",
            db.warnings[0]
        );
        assert!(
            db.warnings[0].contains("gender"),
            "warning: {}",
            db.warnings[0]
        );
    }

    #[test]
    fn place_hierarchy_parses_via_placeref() {
        let db = parse(FULL).unwrap();
        let city = &db.places[0];
        assert_eq!(city.name, "Uppsala");
        assert_eq!(city.place_type.as_deref(), Some("City"));
        assert_eq!(city.parent_handle.as_deref(), Some("_p0001"));
        // place without placeref keeps None
        assert_eq!(db.places[1].parent_handle, None);
    }

    #[test]
    fn privacy_flags_parse_on_primary_records() {
        let db = parse(FULL).unwrap();
        // event
        assert!(db.events[0].private);
        assert!(!db.events[1].private);
        // person
        assert!(db.people[0].private);
        assert!(!db.people[1].private);
        // family
        assert!(!db.families[0].private);
        // place
        assert!(!db.places[0].private);
        assert!(db.places[1].private);
    }

    #[test]
    fn event_cause_description_and_private_parse() {
        let db = parse(FULL).unwrap();
        assert_eq!(db.events[0].cause.as_deref(), Some("custom cause"));
        assert_eq!(db.events[0].description.as_deref(), Some("firstborn"));
        assert!(db.events[0].private);
        // sibling event without cause/description keeps None
        assert_eq!(db.events[1].cause, None);
        assert_eq!(db.events[1].description, None);
    }

    #[test]
    fn header_researcher_name_parses() {
        let db = parse(FULL).unwrap();
        assert_eq!(db.header.created_date.as_deref(), Some("2026-09-29"));
        assert_eq!(db.header.version.as_deref(), Some("5.1.6"));
        // &amp; is decoded by the XML parser
        assert_eq!(
            db.header.researcher_name.as_deref(),
            Some("Dr. Whiskers & Sons")
        );
    }

    #[test]
    fn header_without_researcher_keeps_none() {
        let db =
            parse("<database><header><created date=\"2026-09-29\"/></header></database>").unwrap();
        assert_eq!(db.header.researcher_name, None);
    }

    #[test]
    fn tags_section_parses_into_the_database() {
        let db = parse(FULL).unwrap();
        assert_eq!(db.tags.len(), 2);
        assert_eq!(db.tags[0].handle, "_t0000");
        assert_eq!(db.tags[0].name, "ToDo");
        assert_eq!(db.tags[0].color, "#fb9408");
        assert_eq!(db.tags[0].priority, 0);
        assert_eq!(db.tags[0].change, 10);
        assert_eq!(db.tags[1].priority, 1);
    }

    #[test]
    fn tag_handles_participate_in_duplicate_detection() {
        let err = parse(
            r##"<database><tags>
            <tag handle="_t0" name="A" color="#000" priority="0" change="1"/>
            <tag handle="_t0" name="B" color="#fff" priority="1" change="2"/>
            </tags></database>"##,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            GrampsXmlError::DuplicateHandle { ref handle, .. } if handle == "_t0"
        ));
    }

    #[test]
    fn malformed_tag_priority_warns_and_skips_the_tag() {
        let db = parse(
            r##"<database><tags>
            <tag handle="_t0" name="Broken" color="#000" priority="high" change="1"/>
            <tag handle="_t1" name="Fine" color="#fff" priority="0" change="2"/>
            </tags></database>"##,
        )
        .unwrap();
        assert_eq!(db.tags.len(), 1);
        assert_eq!(db.tags[0].name, "Fine");
        assert_eq!(db.warnings.len(), 1);
        assert!(
            db.warnings[0].contains("_t0"),
            "warning: {}",
            db.warnings[0]
        );
    }

    #[test]
    fn tag_without_handle_is_a_hard_error() {
        let err = parse(r##"<database><tags><tag name="X" color="#000" priority="0" change="1"/></tags></database>"##).unwrap_err();
        assert!(matches!(
            err,
            GrampsXmlError::MissingHandle {
                record: "tag",
                position: 0
            }
        ));
    }

    #[test]
    fn unknown_elements_and_attributes_inside_full_records_are_tolerated() {
        let db = parse(
            r##"<database>
            <tags>
              <tag handle="_t0" name="Mine" color="#fff" priority="0" change="1" future_attr="x"/>
              <future-tag handle="_t1" name="?" color="#000" priority="9" change="9"/>
            </tags>
            <people>
              <person handle="_p0" id="I0000" future_attr="y">
                <gender>M</gender>
                <name type="Birth Name"><first>Frank</first><surname>Future</surname></name>
                <future-meta><x/></future-meta>
              </person>
              <person handle="_p1" id="I0001">
                <gender>F</gender>
                <name type="Birth Name"><first>Joan</first></name>
              </person>
            </people>
            <families>
              <family handle="_f0" id="F0000" future_attr="z">
                <future-meta/>
                <father hlink="_p0"/>
              </family>
            </families>
            <places>
              <placeobj handle="_pl0" id="P0000" future_attr="w">
                <pname value="Narnia"/>
                <future-place-child/>
              </placeobj>
            </places>
            <events>
              <event handle="_e0" id="E0000">
                <type>Birth</type>
                <future-event-child/>
              </event>
            </events>
            </database>"##,
        )
        .unwrap();
        assert_eq!(db.tags.len(), 1);
        assert_eq!(db.people.len(), 2);
        assert_eq!(db.families[0].father.as_deref(), Some("_p0"));
        assert_eq!(db.places[0].name, "Narnia");
        assert_eq!(db.events[0].event_type, "Birth");
        assert!(db.warnings.is_empty());
    }

    #[test]
    fn missing_hlink_on_eventref_warns_and_keeps_the_record() {
        let db = parse(
            r#"<database><people><person handle="_p0" id="I0000">
              <gender>M</gender>
              <eventref role="Primary"/>
              <eventref hlink="_e0" role="Primary"/>
            </person></people></database>"#,
        )
        .unwrap();
        assert_eq!(db.people[0].event_refs.len(), 1);
        assert_eq!(db.people[0].event_refs[0].event_handle, "_e0");
        assert_eq!(db.warnings.len(), 1);
        assert!(
            db.warnings[0].contains("eventref"),
            "warning: {}",
            db.warnings[0]
        );
    }

    #[test]
    fn missing_hlink_on_childref_warns_and_keeps_the_family() {
        let db = parse(
            r#"<database><families><family handle="_f0" id="F0000">
              <childref/>
              <childref hlink="_p0"/>
            </family></families></database>"#,
        )
        .unwrap();
        assert_eq!(db.families[0].children, vec!["_p0"]);
        assert_eq!(db.warnings.len(), 1);
        assert!(
            db.warnings[0].contains("childref"),
            "warning: {}",
            db.warnings[0]
        );
    }

    #[test]
    fn missing_place_hlink_warns_and_keeps_the_event() {
        let db = parse(
            r#"<database><events>
            <event handle="_e0" id="E0000"><type>Birth</type><place/></event>
            </events></database>"#,
        )
        .unwrap();
        assert_eq!(db.events[0].place_handle, None);
        assert_eq!(db.warnings.len(), 1);
        assert!(
            db.warnings[0].contains("place"),
            "warning: {}",
            db.warnings[0]
        );
    }

    #[test]
    fn real_fixture_parses_full_people_families_and_places() {
        let db = parse(include_str!("../../../tests/fixtures/data.gramps")).unwrap();
        // Harry Meowser: gender, birth name, Primary eventref, childof link
        let harry = &db.people[0];
        assert_eq!(harry.gender, crate::Gender::Male);
        assert_eq!(harry.names.len(), 1);
        assert_eq!(harry.names[0].first.as_deref(), Some("Harry"));
        assert_eq!(harry.names[0].surnames[0].value, "Meowser");
        assert_eq!(harry.event_refs.len(), 1);
        assert_eq!(harry.event_refs[0].role, "Primary");
        assert!(!harry.private);
        assert_eq!(harry.child_of.len(), 1);
        // Abraham has two Primary eventrefs (birth + death)
        let abraham = &db.people[4];
        assert_eq!(abraham.event_refs.len(), 2);
        assert_eq!(abraham.parent_in.len(), 1);
        // F0000: father George, mother Sally, child Harry, no family events
        let family = &db.families[0];
        assert_eq!(family.rel.as_deref(), Some("Unknown"));
        assert_eq!(
            family.father.as_deref(),
            Some("_103e39a531a86a1b01d276ee0cf8")
        );
        assert_eq!(
            family.mother.as_deref(),
            Some("_103e39ab2114249d489d0884174")
        );
        assert_eq!(family.children.len(), 1);
        assert!(family.event_refs.is_empty());
        // places in this fixture carry no parent chain
        assert!(db.places.iter().all(|p| p.parent_handle.is_none()));
        // events are all public, none carry cause/description
        assert!(db.events.iter().all(|e| !e.private));
        assert!(
            db.events
                .iter()
                .all(|e| e.cause.is_none() && e.description.is_none())
        );
        assert!(db.tags.is_empty());
        assert!(db.warnings.is_empty());
    }

    #[test]
    fn families_fixture_parses_couple_children_and_family_events() {
        let db = parse(include_str!("../../../tests/fixtures/families.gramps")).unwrap();
        // family F0000 holds Marriage / Marriage-Alternative / Divorce
        let family = &db.families[0];
        assert_eq!(family.rel.as_deref(), Some("Marriage"));
        assert_eq!(family.father.as_deref(), Some("_fp000"));
        assert_eq!(family.mother.as_deref(), Some("_fp001"));
        assert_eq!(family.children, vec!["_fp002"]);
        assert_eq!(family.event_refs.len(), 3);
        assert_eq!(family.event_refs[0].event_handle, "_f0000");
        assert_eq!(family.event_refs[0].role, "Family");
        // Adam: Primary on the dedup case + the ordination, Witness on baptism
        let adam = &db.people[0];
        assert_eq!(adam.gender, crate::Gender::Male);
        assert_eq!(adam.event_refs.len(), 3);
        assert_eq!(adam.event_refs[0].role, "Primary");
        assert_eq!(adam.event_refs[2].role, "Witness");
        // Eve: Primary + Witness (multi-role on one event)
        let eve = &db.people[1];
        assert_eq!(eve.event_refs.len(), 2);
        assert_eq!(eve.event_refs[1].role, "Witness");
        assert!(db.warnings.is_empty());
    }

    #[test]
    fn edge_case_fixture_parses_private_records_and_hostile_names() {
        let db = parse(include_str!("../../../tests/fixtures/edge-cases.gramps")).unwrap();
        // private event and private person
        assert!(db.events[0].private);
        assert!(!db.events[1].private);
        assert!(db.people[0].private);
        assert!(!db.people[1].private);
        // hostile text is decoded faithfully: entities unescaped, markup literal
        let ava = &db.people[0];
        assert_eq!(
            ava.names[0].first.as_deref(),
            Some("Ava <script>alert(\"gotcha\")</script>")
        );
        assert_eq!(ava.names[0].surnames[0].value, "Pryvat & Sons");
        assert_eq!(ava.event_refs.len(), 1);
        assert_eq!(ava.event_refs[0].event_handle, "_e0000");
        assert_eq!(db.header.version, None, "unversioned header stays None");
        assert!(db.warnings.is_empty());
    }
}
