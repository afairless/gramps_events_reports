//! Error type for `.gramps` container decoding and XML parsing.

use std::io;

use thiserror::Error;

/// Every way parsing a `.gramps` file can fail.
///
/// Container errors (`EmptyInput`, `UnknownContainer`, the gzip/zip decode
/// failures) are raised before any XML work; `Xml`/`MissingRoot` cover
/// malformed document structure; `MissingHandle`/`DuplicateHandle` are the
/// parse-boundary handle-index validations (plan §7.1). Record-level defects
/// — a bad attribute value, a malformed date element — are recoverable: the
/// parser warns and skips that record so the rest of the file stays
/// readable, and each malformed-date skip is additionally captured
/// structurally in
/// [`Database::date_issues`](crate::model::Database::date_issues)
/// ([`DateIssue`](crate::model::DateIssue)) alongside its human message in
/// [`Database::warnings`](crate::model::Database::warnings).
#[derive(Debug, Error)]
pub enum GrampsXmlError {
    /// No bytes at all.
    #[error("empty input: expected a plain XML, gzip or zip document")]
    EmptyInput,

    /// The leading bytes match none of the three supported containers.
    #[error(
        "unsupported container: expected plain XML (leading `<`), gzip (magic 1f 8b) or zip \
         (magic PK); found {magic}"
    )]
    UnknownContainer { magic: String },

    /// The gzip member could not be decompressed (e.g. truncated stream).
    #[error("failed to decode gzip container: {0}")]
    GzipDecode(#[from] io::Error),

    /// The zip archive could not be read: missing/corrupt central directory,
    /// truncated members, unsupported features, ...
    #[error("failed to read zip container: {0}")]
    ZipDecode(#[from] zip::result::ZipError),

    /// The zip archive is intact but contains no `data.gramps` member —
    /// Gramps "saved tree" archives name their XML payload that.
    #[error("zip container has no data.gramps member (found: {0}); not a Gramps saved tree?")]
    ZipMissingMember(String),

    /// The decoded XML payload is not UTF-8.
    #[error("decoded XML is not valid UTF-8")]
    InvalidUtf8,

    /// The payload is not well-formed XML.
    #[error("malformed XML: {0}")]
    Xml(#[from] roxmltree::Error),

    /// The document parses as XML but has no `<database>` root element.
    #[error("XML does not describe a Gramps database: no <database> root element")]
    MissingRoot,

    /// A record carries no `handle` attribute, so the parse-boundary index
    /// cannot key it (plan §7.1 requires hard failure, naming the
    /// record type and its position within the section).
    #[error("{record} record at position {position} is missing its handle")]
    MissingHandle {
        record: &'static str,
        position: usize,
    },

    /// Two records share one `handle`, violating the per-document `ID`
    /// uniqueness the Gramps DTD demands (plan §7.1 hard failure).
    #[error(
        "duplicate handle {handle:?}: first seen on a {first} record, then on a {second} record"
    )]
    DuplicateHandle {
        handle: String,
        first: &'static str,
        second: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::GrampsXmlError;

    /// Errors remain `Send + Sync` so `anyhow`/axum can carry them across
    /// threads in the CLI and web layers.
    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GrampsXmlError>();
    }
}
