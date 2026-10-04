//! Serializer for the malformed-date error report (plan §3.2).
//!
//! `write_date_issues` turns the ingestion side's structured
//! [`DateIssue`]s into the deterministic JSON report the CLI writes as
//! `{prefix}.errors.json`:
//!
//! ```json
//! {
//!   "error_count": 2,
//!   "errors": [
//!     {
//!       "event_id": "E0005",
//!       "event_handle": "_abc",
//!       "event_type": "Death",
//!       "date_kind": "daterange",
//!       "message": "daterange/datespan stop \"1914\" sorts before start \"1918\""
//!     }
//!   ]
//! }
//! ```
//!
//! The report is a single object — not an array — with the count above the
//! entries. It contains no clock values, entries appear in document (slice)
//! order, and the serializer sorts nothing else (plan §6 determinism).

use std::io::Write as _;
use std::path::Path;

use event_core::DateIssue;
use serde::Serialize;

use crate::atomic::write_atomically;
use crate::error::WriterError;

/// One `errors` array entry, mirroring [`DateIssue`] with the report's
/// fixed field order (`event_id` first, then the rest) and borrowing
/// instead of cloning. Every field is a plain string — nothing is `null`.
#[derive(Serialize)]
struct ErrorEntry<'a> {
    event_id: &'a str,
    event_handle: &'a str,
    event_type: &'a str,
    date_kind: &'a str,
    message: &'a str,
}

impl<'a> From<&'a DateIssue> for ErrorEntry<'a> {
    fn from(issue: &'a DateIssue) -> Self {
        Self {
            event_id: &issue.event_id,
            event_handle: &issue.event_handle,
            event_type: &issue.event_type,
            date_kind: &issue.date_kind,
            message: &issue.message,
        }
    }
}

/// The report envelope: the count plus the ordered `errors` array.
#[derive(Serialize)]
struct DateIssueReport<'a> {
    error_count: usize,
    errors: Vec<ErrorEntry<'a>>,
}

/// Serialize `issues` as an atomic, deterministic error report at `dest`.
///
/// The report is `{ "error_count": N, "errors": [...] }` with entries in
/// slice (document) order and every field a serde-escaped string, so
/// quotes, backslashes and newlines in the message round-trip intact.
/// Writing is atomic ([`write_atomically`]): `dest` is either absent or
/// complete, never partial.
///
/// An empty slice still produces a well-formed report (`error_count: 0`
/// with an empty `errors` array) so callers may pass whatever they hold;
/// deciding whether the file is needed at all belongs to the caller (the
/// CLI writes it only when at least one issue exists).
pub fn write_date_issues(issues: &[DateIssue], dest: &Path) -> Result<(), WriterError> {
    write_atomically(dest, |mut file| {
        serde_json::to_writer(
            &mut file,
            &DateIssueReport {
                error_count: issues.len(),
                errors: issues.iter().map(ErrorEntry::from).collect(),
            },
        )?;
        file.flush().map_err(|e| WriterError::io(dest, e))?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::write_date_issues;
    use event_core::DateIssue;
    use serde_json::Value;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn issue(event_id: &str, message: &str) -> DateIssue {
        DateIssue {
            event_handle: format!("_h_{event_id}"),
            event_id: event_id.to_string(),
            event_type: "Death".to_string(),
            date_kind: "daterange".to_string(),
            message: message.to_string(),
        }
    }

    fn read_report(dest: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(dest).unwrap()).unwrap()
    }

    fn temp_leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn report_is_an_object_with_an_errors_array_in_document_order() {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("out.errors.json");
        let issues = vec![issue("E0005", "first"), issue("E0002", "second")];

        write_date_issues(&issues, &dest).unwrap();

        let root = read_report(&dest);
        assert!(root.is_object(), "report must be an object: {root}");
        assert_eq!(root["error_count"], 2);
        let errors = root["errors"].as_array().expect("errors must be an array");
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0]["event_id"], "E0005");
        assert_eq!(errors[1]["event_id"], "E0002");
    }

    #[test]
    fn every_entry_field_is_a_string_never_null() {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("out.errors.json");
        let issues = vec![
            issue("E0005", "stop \"1914\" sorts before start \"1918\""),
            issue("E0009", "unrecognized month value"),
        ];

        write_date_issues(&issues, &dest).unwrap();

        let raw = fs::read_to_string(&dest).unwrap();
        assert!(
            !raw.contains(":null") && !raw.contains("null,"),
            "no null field values in the report: {raw}"
        );
        let fields = [
            "event_id",
            "event_handle",
            "event_type",
            "date_kind",
            "message",
        ];
        for entry in read_report(&dest)["errors"].as_array().unwrap() {
            for field in fields {
                let value = &entry[field];
                assert!(value.is_string(), "{field} must be a string, got {value:?}");
            }
        }
    }

    #[test]
    fn empty_input_yields_a_well_formed_empty_report() {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("out.errors.json");

        write_date_issues(&[], &dest).unwrap();

        let root = read_report(&dest);
        assert_eq!(root["error_count"], 0);
        assert_eq!(
            root["errors"].as_array().map(Vec::len),
            Some(0),
            "empty errors array"
        );
    }

    #[test]
    fn a_second_write_replaces_the_first_atomically() {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("out.errors.json");
        let first = vec![issue("E0005", "old message")];
        let second = vec![issue("E0007", "new message")];

        write_date_issues(&first, &dest).unwrap();
        assert_eq!(read_report(&dest)["errors"][0]["message"], "old message");

        write_date_issues(&second, &dest).unwrap();
        // rename(2) is atomic-over-replace on POSIX: only the new content
        // is ever visible at the destination, no temp file lingers.
        assert_eq!(read_report(&dest)["errors"][0]["message"], "new message");
        assert_eq!(read_report(&dest)["error_count"], 1);
        assert!(
            temp_leftovers(td.path()).is_empty(),
            "temp files left behind"
        );
    }

    #[test]
    fn message_with_quotes_backslashes_and_newlines_round_trips() {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("out.errors.json");
        let message = "he said \"hi\\there\"\nline two\r\n\ttabbed";
        let issues = vec![issue("E0005", message)];

        write_date_issues(&issues, &dest).unwrap();

        // serde_json escapes the hostile bytes on disk…
        let raw = fs::read_to_string(&dest).unwrap();
        assert!(raw.contains(r#"\""#), "quotes escaped: {raw}");
        assert!(raw.contains(r"\\"), "backslashes escaped: {raw}");
        assert!(raw.contains(r"\n"), "newlines escaped: {raw}");
        assert!(
            !raw.contains('\n'),
            "raw newline bytes must not reach the file: {raw:?}"
        );
        // …and decoding restores the exact original string.
        assert_eq!(
            read_report(&dest)["errors"][0]["message"],
            message,
            "round-trip must preserve the message byte-for-byte"
        );
    }
}
