//! The shared server state and the loaded-database lifecycle.
//!
//! Plan §9: `Arc<AppState>` holds the loaded `Database`, the current
//! `ReportOptions`, and the server temp dir. Uploads land in that dir
//! under **generated** names — the browser-supplied filename is never
//! used as a path or trusted — and are deleted on `POST /api/reset` (the
//! entry is dropped, deleting the `NamedTempFile`) and at process exit
//! (the `TempDir` itself drops with the state).

use std::sync::RwLock;

use event_core::ReportOptions;
use gramps_xml::Database;
use serde::Serialize;
use tempfile::{NamedTempFile, TempDir};

/// The plan's hard upload cap: 200 MB (plan §9). `AppState::new` pins it;
/// tests build states with tiny caps to exercise the 413 path cheaply.
pub const UPLOAD_CAP_BYTES: usize = 200 * 1024 * 1024;

/// The shared server state (plan §9).
#[derive(Debug)]
pub struct AppState {
    /// The loaded database, or `None` before `POST /api/load` / after
    /// `POST /api/reset`.
    pub db: RwLock<Option<LoadedDatabase>>,
    /// The current run options every view and export is filtered by.
    pub opts: RwLock<ReportOptions>,
    /// The per-server temp dir holding the upload(s) and any export
    /// scratch — removed when the state drops (process exit).
    pub temp_dir: TempDir,
    /// The hard upload cap in bytes (plan §9's 200 MB by default).
    pub upload_cap: usize,
}

impl AppState {
    /// A fresh state: plan defaults (200 MB cap, current-year options)
    /// and its own temp dir.
    pub fn new() -> std::io::Result<AppState> {
        AppState::with_cap(UPLOAD_CAP_BYTES)
    }

    /// A state with an explicit upload cap — tests use tiny caps to reach
    /// the 413 branch without allocating hundreds of megabytes.
    pub fn with_cap(cap: usize) -> std::io::Result<AppState> {
        Ok(AppState {
            db: RwLock::new(None),
            opts: RwLock::new(ReportOptions::default()),
            temp_dir: tempfile::tempdir()?,
            upload_cap: cap,
        })
    }
}

/// A loaded database together with its on-disk upload.
///
/// Dropping the struct (reset, reload, process exit) deletes the uploaded
/// temp file — the `NamedTempFile` owns cleanup.
#[derive(Debug)]
pub struct LoadedDatabase {
    /// The display name the browser supplied — used for display only,
    /// never as a filesystem path (plan §9).
    pub file_name: String,
    /// The uploaded bytes under their generated temp name.
    pub upload: NamedTempFile,
    /// The parsed database.
    pub db: Database,
    /// The event-type enumeration (count desc, then type name — the same
    /// order the CLI `inspect` prints, so the GUI's checkbox list and the
    /// CLI agree; plan §9).
    pub event_types: Vec<EventTypeCount>,
    /// Uploaded byte count.
    pub bytes: u64,
}

impl LoadedDatabase {
    /// The generated temp path the upload was streamed into.
    pub fn temp_path(&self) -> &std::path::Path {
        self.upload.path()
    }

    /// The [`LoadedSummary`] this loaded file presents to the API and the
    /// templates.
    pub fn summary(&self) -> LoadedSummary {
        LoadedSummary {
            file_name: self.file_name.clone(),
            bytes: self.bytes,
            event_count: self.db.events.len(),
            people_count: self.db.people.len(),
            event_types: self.event_types.clone(),
            date_errors: self
                .db
                .date_issues
                .iter()
                .map(|issue| DateIssueDto {
                    event_id: issue.event_id.clone(),
                    event_handle: issue.event_handle.clone(),
                    event_type: issue.event_type.clone(),
                    date_kind: issue.date_kind.clone(),
                    message: issue.message.clone(),
                })
                .collect(),
        }
    }
}

/// One entry of the event-type enumeration: the verbatim `<type>` string
/// and how many events carry it. Serializes as `{"type": …, "count": …}`
/// — the shape `POST /api/load` returns (plan §9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventTypeCount {
    /// The event `<type>` verbatim, e.g. `"Birth"` or any custom type.
    #[serde(rename = "type")]
    pub event_type: String,
    /// How many events in the loaded database carry this type.
    pub count: usize,
}

/// Count event types in the plan's deterministic order: count descending,
/// type name ascending (plan §8 rule 12 / §9) — independent of any hash
/// iteration order.
pub fn count_event_types(db: &Database) -> Vec<EventTypeCount> {
    let mut counts: Vec<EventTypeCount> = Vec::new();
    for event in &db.events {
        match counts.iter_mut().find(|c| c.event_type == event.event_type) {
            Some(count) => count.count += 1,
            None => counts.push(EventTypeCount {
                event_type: event.event_type.clone(),
                count: 1,
            }),
        }
    }
    counts.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.event_type.cmp(&b.event_type))
    });
    counts
}

/// The summary `POST /api/load` returns as JSON (`{event_types,
/// people_count, …}`) and the landing-page templates render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoadedSummary {
    /// The display name the browser supplied.
    pub file_name: String,
    /// Uploaded byte count.
    pub bytes: u64,
    /// The number of `<event>` records parsed.
    pub event_count: usize,
    /// The number of `<person>` records parsed.
    pub people_count: usize,
    /// The deterministic event-type enumeration.
    pub event_types: Vec<EventTypeCount>,
    /// The malformed-date issues (decision D-h: the web UI shows them
    /// from the load summary; there is no `errors.json` download).
    pub date_errors: Vec<DateIssueDto>,
}

/// One malformed-date issue of the load summary — the web DTO mirrors the
/// CLI's `{prefix}.errors.json` entry fields (plan §3.3). The ingestion
/// field is named `date_issues`; `date_errors` here is the intentional
/// presentation rename.
///
/// Serializes as `{"event_id": …, "event_handle": …, "event_type": …,
/// "date_kind": …, "message": …}` — the same shape `POST /api/load`
/// returns that the file report uses. Every field is a plain string;
/// `message` and `event_type` embed raw text from the parsed file, so the
/// Askama templates HTML-escape each one when rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DateIssueDto {
    /// The Gramps id of the skipped event (`id="E0000"`); falls back to
    /// the handle when the attribute is absent.
    pub event_id: String,
    /// The `<event>` handle.
    pub event_handle: String,
    /// The `<type>` text read before the skip — empty when the file
    /// omits `<type>` entirely.
    pub event_type: String,
    /// The date element tag that failed: `dateval` | `daterange` |
    /// `datespan` | `datestr`.
    pub date_kind: String,
    /// The [`DateError`] display string, e.g.
    /// `daterange/datespan stop "1914" sorts before start "1918"`.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_uses_the_plan_upload_cap() {
        assert_eq!(AppState::new().unwrap().upload_cap, UPLOAD_CAP_BYTES);
    }

    #[test]
    fn default_options_ride_along() {
        let state = AppState::new().unwrap();
        assert_eq!(
            state.opts.read().unwrap().reference_year,
            ReportOptions::default().reference_year
        );
        assert!(state.db.read().unwrap().is_none());
    }

    #[test]
    fn event_types_sort_count_desc_then_name() {
        const XML: &str = r#"<?xml version="1.0"?>
<database xmlns="http://gramps-project.org/xml/1.7.1/">
  <events>
    <event handle="_e1" id="E0"><type>Birth</type></event>
    <event handle="_e2" id="E1"><type>Marriage</type></event>
    <event handle="_e3" id="E2"><type>Birth</type></event>
    <event handle="_e4" id="E3"><type>Death</type></event>
  </events>
</database>"#;
        let db = gramps_xml::parse_database(XML.as_bytes()).unwrap();
        let counts = count_event_types(&db);
        let kinds: Vec<&str> = counts.iter().map(|c| c.event_type.as_str()).collect();
        // Birth ×2 first, then Death/Marriage alphabetically.
        assert_eq!(kinds, vec!["Birth", "Death", "Marriage"]);
        assert_eq!(counts[0].count, 2);
    }
}
