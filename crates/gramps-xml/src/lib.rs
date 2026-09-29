//! gramps-xml — `.gramps` container detection + XML → typed model.
//!
//! A `.gramps` file ships in any of three container forms (plan §3.1), each
//! recognizable by its leading bytes:
//!
//! | Form | Detection | Handling |
//! | --- | --- | --- |
//! | Plain XML | starts with `<` | parsed directly |
//! | Gzip-compressed XML | magic `1f 8b` | decompressed via `flate2` |
//! | Zip archive ("saved tree") | magic `PK` | member `data.gramps` read via `zip` |
//!
//! [`parse_database`] is the single entry point: it detects the container,
//! decodes it to UTF-8 XML text ([`decode_container`]) and maps the
//! document onto the typed [`Database`] model (see [`crate::parse`] for the
//! XML mapping rules and [`crate::error`] for the failure modes).
//!
//! The model mirrors the `grampsxml.dtd`: every primary record carries a
//! unique `handle` (validated by a parse-time handle index), and records
//! carry handles, not resolved links — cross-reference *resolution* lives in
//! `event-core`. Full records parse for the sections v1 consumes: person
//! names (multiple names, surname prefix/`prim`), `eventref` roles, family
//! members, the place hierarchy, privacy flags on every primary record, and
//! the `header`/`tags` sections; unknown elements and attributes are
//! tolerated (forward compatibility). Dates are fully wired: the four
//! interchangeable date elements (`dateval`, `daterange`, `datespan`,
//! `datestr`) parse into `gramps-dates`' [`GrampsDate`].

pub mod error;
pub mod model;
pub mod parse;

pub use error::GrampsXmlError;
pub use model::{
    Database, Event, EventRef, Family, Gender, Header, Person, PersonName, Place, Surname, Tag,
};
pub use parse::parse_database;

use std::io::{Cursor, Read};

use flate2::read::GzDecoder;
use zip::ZipArchive;

/// The three container forms a `.gramps` file ships in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// Plain, uncompressed XML (starts with `<`).
    Xml,
    /// Gzip-compressed XML (magic `1f 8b`).
    Gzip,
    /// Zip archive holding a `data.gramps` member (magic `PK`) — Gramps
    /// "saved tree" archives.
    Zip,
}

/// Canonical crate name — smoke tests (and later workspace integration
/// tests) use this to assert crate linkage.
pub const CRATE_NAME: &str = "gramps-xml";

/// Detect the container form from the leading bytes of a `.gramps` file.
///
/// Returns `None` for empty input or unrecognized magic (anything that is
/// not XML, gzip or zip).
pub fn detect_container(bytes: &[u8]) -> Option<Container> {
    match bytes {
        [0x1f, 0x8b, ..] => Some(Container::Gzip),
        // Explicit `..` slice patterns: a byte-string literal pattern like
        // `b"PK\x03\x04"` only matches slices of *exactly* that length, so
        // it would reject every real archive — the magic must match as a
        // prefix instead.
        [b'P', b'K', 0x03, 0x04, ..]
        | [b'P', b'K', 0x05, 0x06, ..]
        | [b'P', b'K', 0x07, 0x08, ..] => Some(Container::Zip),
        [b'<', ..] => Some(Container::Xml),
        _ => None,
    }
}

/// Decode a container into UTF-8 XML text.
///
/// Plain XML is passed through; gzip is inflated; zip archives are scanned
/// for their `data.gramps` member. Container-granularity failures (empty
/// input, unknown magic, truncated/foreign archives, non-UTF-8 payload)
/// raise [`GrampsXmlError`] here; XML structure errors surface only once
/// the document is parsed by [`parse_database`].
pub fn decode_container(bytes: &[u8]) -> Result<String, GrampsXmlError> {
    let kind = detect_container(bytes).ok_or_else(|| {
        if bytes.is_empty() {
            GrampsXmlError::EmptyInput
        } else {
            GrampsXmlError::UnknownContainer {
                magic: preview(bytes),
            }
        }
    })?;

    let xml: Vec<u8> = match kind {
        Container::Xml => bytes.to_vec(),
        Container::Gzip => {
            let mut decoder = GzDecoder::new(bytes);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out)?;
            out
        }
        Container::Zip => {
            let mut archive = ZipArchive::new(Cursor::new(bytes))?;
            let mut payload = None;
            for i in 0..archive.len() {
                let mut member = archive.by_index(i)?;
                if member.name() == "data.gramps" {
                    let mut out = Vec::new();
                    member
                        .read_to_end(&mut out)
                        .map_err(zip::result::ZipError::Io)?;
                    payload = Some(out);
                    break;
                }
            }
            payload.ok_or_else(|| {
                let found = (0..archive.len())
                    .filter_map(|i| archive.by_index(i).ok().map(|m| m.name().to_string()))
                    .collect::<Vec<_>>()
                    .join(", ");
                GrampsXmlError::ZipMissingMember(found)
            })?
        }
    };

    String::from_utf8(xml).map_err(|_| GrampsXmlError::InvalidUtf8)
}

/// Render the leading bytes of an unrecognized input for error messages —
/// printable ASCII is shown verbatim, everything else is dotted.
fn preview(bytes: &[u8]) -> String {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(16)]);
    if bytes.len() > 16 {
        format!("{head:?}…")
    } else {
        format!("{head:?}")
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use zip::write::SimpleFileOptions;

    use super::{Container, decode_container, detect_container, parse_database};
    use crate::GrampsXmlError;

    /// Real fixtures committed at the repository root (step 1). `include_bytes!`
    /// bakes them in at compile time, so the tests run from any cwd.
    const DATA_GRAMPS: &[u8] = include_bytes!("../../../tests/fixtures/data.gramps");
    const GZIP_CONTAINER: &[u8] =
        include_bytes!("../../../tests/fixtures/containers/data.gramps.gz");
    const ZIP_CONTAINER: &[u8] =
        include_bytes!("../../../tests/fixtures/containers/tree.gramps.zip");

    #[test]
    fn detects_all_three_containers_from_magic() {
        assert_eq!(detect_container(DATA_GRAMPS), Some(Container::Xml));
        assert_eq!(detect_container(GZIP_CONTAINER), Some(Container::Gzip));
        assert_eq!(detect_container(ZIP_CONTAINER), Some(Container::Zip));
        assert_eq!(detect_container(b""), None);
        assert_eq!(detect_container(b"MZ\x90\x00"), None);
        assert_eq!(detect_container(b"PK only"), None);
        // Leading `<` is enough — `<?xml` and unadorned roots both detect.
        assert_eq!(
            detect_container(b"<?xml version=\"1.0\"?>"),
            Some(Container::Xml)
        );
    }

    #[test]
    fn plain_xml_container_parses_directly() {
        let db = parse_database(DATA_GRAMPS).unwrap();
        assert_eq!(db.events.len(), 6);
        assert_eq!(db.people.len(), 5);
        assert_eq!(db.families.len(), 3);
        assert_eq!(db.places.len(), 4);
        assert!(db.warnings.is_empty());
        assert_eq!(db.header.version.as_deref(), Some("5.1.6"));
    }

    #[test]
    fn gzip_container_parses_to_the_same_database() {
        assert_eq!(detect_container(GZIP_CONTAINER), Some(Container::Gzip));
        let db = parse_database(GZIP_CONTAINER).unwrap();
        assert_eq!(db.events.len(), 6);
        assert_eq!(db.events[0].handle, "_103e399ca54970ad6794623655bc");
        assert_eq!(db.events[0].date.as_ref().unwrap().ymd, (2000, 3, 3));
        assert!(db.warnings.is_empty());
    }

    #[test]
    fn zip_container_parses_its_data_gramps_member() {
        assert_eq!(detect_container(ZIP_CONTAINER), Some(Container::Zip));
        let db = parse_database(ZIP_CONTAINER).unwrap();
        assert_eq!(db.events.len(), 6);
        assert_eq!(db.people.len(), 5);
        assert_eq!(db.families.len(), 3);
        assert_eq!(db.places.len(), 4);
        assert!(db.warnings.is_empty());
    }

    #[test]
    fn all_three_containers_produce_identical_databases() {
        let plain = parse_database(DATA_GRAMPS).unwrap();
        let gzip = parse_database(GZIP_CONTAINER).unwrap();
        let zip = parse_database(ZIP_CONTAINER).unwrap();
        assert_eq!(plain, gzip);
        assert_eq!(plain, zip);
    }

    #[test]
    fn empty_input_is_rejected() {
        assert!(matches!(
            parse_database(b""),
            Err(GrampsXmlError::EmptyInput)
        ));
    }

    #[test]
    fn unknown_magic_is_rejected_with_a_preview() {
        let err = parse_database(b"MZ\x90\x00\x03\x00\x00\x00").unwrap_err();
        assert!(matches!(err, GrampsXmlError::UnknownContainer { .. }));
        assert!(err.to_string().contains("MZ"), "message: {err}");
    }

    #[test]
    fn truncated_gzip_raises_a_decode_error() {
        let truncated = &GZIP_CONTAINER[..GZIP_CONTAINER.len() / 2];
        assert!(matches!(
            parse_database(truncated),
            Err(GrampsXmlError::GzipDecode(_))
        ));
    }

    #[test]
    fn gzip_of_non_xml_raises_a_parse_error() {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"definitely not xml").unwrap();
        let bytes = encoder.finish().unwrap();
        assert!(matches!(
            parse_database(&bytes),
            Err(GrampsXmlError::Xml(_))
        ));
    }

    #[test]
    fn zip_without_data_gramps_member_is_rejected() {
        let bytes = make_zip(&[("other.txt", b"hello")]);
        let err = parse_database(&bytes).unwrap_err();
        assert!(matches!(err, GrampsXmlError::ZipMissingMember(_)));
        assert!(err.to_string().contains("other.txt"), "message: {err}");
    }

    #[test]
    fn zip_with_corrupt_data_gramps_content_raises_a_parse_error() {
        let bytes = make_zip(&[("data.gramps", b"this is not xml")]);
        assert!(matches!(
            parse_database(&bytes),
            Err(GrampsXmlError::Xml(_))
        ));
    }

    #[test]
    fn truncated_zip_raises_a_decode_error() {
        let truncated = &ZIP_CONTAINER[..ZIP_CONTAINER.len() / 2];
        assert!(matches!(
            parse_database(truncated),
            Err(GrampsXmlError::ZipDecode(_))
        ));
    }

    #[test]
    fn decode_container_round_trips_plain_xml() {
        assert_eq!(
            decode_container(DATA_GRAMPS).unwrap(),
            std::str::from_utf8(DATA_GRAMPS).unwrap()
        );
        assert_eq!(
            decode_container(GZIP_CONTAINER).unwrap(),
            std::str::from_utf8(DATA_GRAMPS).unwrap()
        );
        assert_eq!(
            decode_container(ZIP_CONTAINER).unwrap(),
            std::str::from_utf8(DATA_GRAMPS).unwrap()
        );
    }

    /// Build a small in-memory zip with stored (uncompressed) members.
    fn make_zip(members: &[(&str, &[u8])]) -> Vec<u8> {
        use zip::ZipWriter;
        let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, content) in members {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
}
