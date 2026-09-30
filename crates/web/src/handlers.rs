//! The route handlers (plan §9 route table) and the small DTOs they
//! serve. Every handler keeps the 200 MB cap, the generated temp names
//! and the escape-by-default templates honest; none of them await while
//! holding a state lock.

use std::collections::HashSet;
use std::io::Write as _;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Multipart, Query, State};
use axum::http::{HeaderMap, header};
use axum::response::{Html, IntoResponse, Response};
use event_core::{LeapDayPolicy, ReportOptions, ViewKind, build_view, collect_events, rows};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use writers::{
    CsvWriter, EventWriter, JsonWriter, ParquetWriter, PdfBackend, TypstPdf, build_pdf_document,
};

use crate::error::WebError;
use crate::state::{AppState, LoadedDatabase, LoadedSummary, count_event_types};
use crate::templates::{EventsFragmentTemplate, FragmentRow, IndexTemplate, MainTemplate};
use gramps_xml::Database;

/// `GET /` — the server-rendered landing page (plan §9): the upload form
/// when nothing is loaded, the loaded-file summary + HTMX view/export
/// controls otherwise. The page survives refresh because the summary is
/// rendered server-side from the state.
pub async fn root(State(state): State<Arc<AppState>>) -> Result<Response, WebError> {
    let summary = summary_of(&state);
    render(IndexTemplate {
        loaded: summary.as_ref(),
    })
}

/// `POST /api/load` — the multipart upload route (plan §9). The file
/// field is streamed to a **generated** temp name (the browser filename
/// is display-only), the hard cap is enforced while streaming (413
/// above the cap), the bytes are parsed and the database + upload are
/// stored in the state. Answers with the JSON summary `{event_types,
/// people_count, …}`; HTMX requests (the landing page) get the same data
/// as the `main.html` fragment to swap in instead.
pub async fn load(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Response, WebError> {
    // Stream the first file part to a generated temp name, enforcing the
    // cap before any parse reads the bytes into memory.
    let mut upload: Option<(NamedTempFile, String, u64)> = None;
    while let Some(mut field) = multipart.next_field().await? {
        if field.file_name().is_none() {
            continue; // non-file form fields are skipped
        }
        let file_name = field.file_name().unwrap_or_default().to_string();
        let mut tmp = NamedTempFile::new_in(state.temp_dir.path())?;
        let mut total: u64 = 0;
        while let Some(chunk) = field.chunk().await? {
            total += chunk.len() as u64;
            if total > state.upload_cap as u64 {
                return Err(WebError::TooLarge {
                    cap: state.upload_cap,
                });
            }
            tmp.write_all(&chunk)?;
        }
        upload = Some((tmp, file_name, total));
        break; // the first file field is the database
    }

    let (tmp, file_name, bytes) = upload.ok_or(WebError::NoFile)?;
    let data = std::fs::read(tmp.path())?;
    let db = gramps_xml::parse_database(&data)?; // parse failure drops `tmp`
    let event_types = count_event_types(&db);
    let loaded = LoadedDatabase {
        file_name,
        upload: tmp,
        db,
        event_types,
        bytes,
    };

    {
        // Replacing a previous file drops the old entry and its upload.
        let mut guard = state.db.write().map_err(|_| WebError::Lock)?;
        *guard = Some(loaded);
    }
    let summary = summary_of(&state).ok_or(WebError::Lock)?;
    if is_hx(&headers) {
        render(MainTemplate {
            loaded: Some(&summary),
        })
    } else {
        Ok(Json(summary).into_response())
    }
}

/// `GET /api/options` — the current run options as JSON (plan §9): the
/// reference year, the type include/exclude sets (rule 14), living-only,
/// privacy, the leap-day policy (D6) and the orphan toggle. The PUT
/// update route arrives with the full UI (milestone 14).
pub async fn options(State(state): State<Arc<AppState>>) -> Result<Json<OptionsDto>, WebError> {
    let guard = state.opts.read().map_err(|_| WebError::Lock)?;
    Ok(Json(OptionsDto::of(&guard)))
}

/// `GET /api/events?view=list` — the rendered view as an HTML fragment
/// (HTMX target swap, plan §9). Milestone 13 implements the list view;
/// the calendar / timeline / calendar-with-years fragments arrive with
/// the full UI (milestone 14).
pub async fn events(
    State(state): State<Arc<AppState>>,
    Query(query): Query<EventsQuery>,
) -> Result<Response, WebError> {
    match query.view.as_deref() {
        None | Some("list") => {}
        Some(other) => {
            return Err(WebError::UnsupportedView {
                view: other.to_string(),
                supported: "list",
            });
        }
    }
    let opts = state.opts.read().map_err(|_| WebError::Lock)?.clone();
    let db = loaded_db(&state)?;
    let built = build_view(&collect_events(&db, &opts), ViewKind::List, &opts);
    let frag_rows: Vec<FragmentRow> = rows(&built).iter().map(FragmentRow::of).collect();
    render(EventsFragmentTemplate {
        rows: frag_rows,
        reference_year: opts.reference_year,
    })
}

/// `GET /api/events.json` — the same data as the `EventRow` JSON, for
/// debug / inspection (plan §9): the list view's flat rows serialized
/// exactly as the JSON writer emits them.
pub async fn events_json(State(state): State<Arc<AppState>>) -> Result<Response, WebError> {
    let opts = state.opts.read().map_err(|_| WebError::Lock)?.clone();
    let db = loaded_db(&state)?;
    let built = build_view(&collect_events(&db, &opts), ViewKind::List, &opts);
    let flat = rows(&built);
    Ok(([(header::CONTENT_TYPE, "application/json")], Json(flat)).into_response())
}

/// `GET /api/export?format=csv|json|parquet|pdf` — one format per request;
/// the generated file streams as a `Content-Disposition: attachment`
/// download built from the current options (plan §9). The file is
/// rendered into a throwaway temp dir (the writers are atomic — temp +
/// rename) whose drop cleans it up; the browser-supplied names never
/// reach the filesystem here either.
pub async fn export(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, WebError> {
    let format = match query.format.as_deref() {
        Some("csv") => ExportFormat::Csv,
        Some("json") => ExportFormat::Json,
        Some("parquet") => ExportFormat::Parquet,
        Some("pdf") => ExportFormat::Pdf,
        Some(other) => {
            return Err(WebError::UnknownFormat {
                format: other.to_string(),
            });
        }
        None => {
            return Err(WebError::UnknownFormat {
                format: "(missing)".to_string(),
            });
        }
    };

    let opts = state.opts.read().map_err(|_| WebError::Lock)?.clone();
    let db = loaded_db(&state)?;
    let built = build_view(&collect_events(&db, &opts), ViewKind::List, &opts);

    let dir = tempfile::tempdir()?;
    let dest = dir
        .path()
        .join(format!("gramps-events.{}", format.extension()));
    match format {
        ExportFormat::Csv => CsvWriter.write(&rows(&built), &dest)?,
        ExportFormat::Json => JsonWriter.write(&rows(&built), &dest)?,
        ExportFormat::Parquet => ParquetWriter.write(&rows(&built), &dest)?,
        ExportFormat::Pdf => {
            let doc = build_pdf_document(&built, &opts);
            TypstPdf.render(&doc, &dest)?;
        }
    }
    let bytes = std::fs::read(&dest)?;
    let disposition = format!(
        "attachment; filename=\"gramps-events.{}\"",
        format.extension()
    );
    Ok((
        [
            (header::CONTENT_TYPE, format.content_type().to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    )
        .into_response())
}

/// `POST /api/reset` — unload the current file: the database entry and
/// its uploaded temp file are dropped (plan §9), the landing page (HTMX)
/// gets the upload form back, API consumers get `{"ok": true}`.
/// Idempotent — resetting an empty state is a no-op success.
pub async fn reset(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    {
        let mut guard = state.db.write().map_err(|_| WebError::Lock)?;
        *guard = None; // dropping LoadedDatabase deletes the upload
    }
    if is_hx(&headers) {
        render(MainTemplate { loaded: None })
    } else {
        Ok((
            [(header::CONTENT_TYPE, "application/json")],
            Json(json_ok()),
        )
            .into_response())
    }
}

/// `GET /static/htmx.min.js` — the vendored HTMX 2.x single file (plan
/// §6.4): compiled into the binary, so the server needs no static files
/// on disk. See `static/README.md` for provenance and license.
pub async fn htmx() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/htmx.min.js"),
    )
}

/// The `view` query parameter of `GET /api/events`.
#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    /// The view token (`list` is the only implemented view in this
    /// milestone).
    pub view: Option<String>,
}

/// The `format` query parameter of `GET /api/export`.
#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    /// The export format token: `csv`, `json`, `parquet` or `pdf`.
    pub format: Option<String>,
}

/// The exported formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportFormat {
    Csv,
    Json,
    Parquet,
    Pdf,
}

impl ExportFormat {
    /// The file extension for `Content-Disposition` and the scratch name.
    fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
            ExportFormat::Parquet => "parquet",
            ExportFormat::Pdf => "pdf",
        }
    }

    /// The `Content-Type` of the download.
    fn content_type(self) -> &'static str {
        match self {
            ExportFormat::Csv => "text/csv; charset=utf-8",
            ExportFormat::Json => "application/json",
            ExportFormat::Parquet => "application/octet-stream",
            ExportFormat::Pdf => "application/pdf",
        }
    }
}

/// The `GET /api/options` JSON — a deterministic projection of
/// [`ReportOptions`]: hash sets are sorted, `None` stays `null`, and the
/// leap-day policy serializes as a stable string token.
#[derive(Debug, Serialize)]
pub struct OptionsDto {
    /// The report year every elapsed value is measured against (rule 4).
    pub reference_year: i32,
    /// `null` = every type; else the whitelist, sorted.
    pub include_types: Option<Vec<String>>,
    /// The exclusion list, sorted (rule 14: exclusion wins).
    pub exclude_types: Vec<String>,
    /// `--living-only` equivalent (rule 15).
    pub living_only: bool,
    /// `--include-private` equivalent (plan §8.7).
    pub include_private: bool,
    /// The leap-day policy: `"fold-to-feb-28"` (D6) or `"keep"`.
    pub leap_day: &'static str,
    /// Orphans toggle (D7).
    pub show_orphans: bool,
    /// Deduplicate identical (type, subject, month-day) rows (rule 6).
    pub dedupe_same: bool,
}

impl OptionsDto {
    /// Project the options deterministically (rule 12).
    fn of(opts: &ReportOptions) -> OptionsDto {
        OptionsDto {
            reference_year: opts.reference_year,
            include_types: opts.include_types.as_ref().map(sorted_vec),
            exclude_types: sorted_vec(&opts.exclude_types),
            living_only: opts.living_only,
            include_private: opts.include_private,
            leap_day: match opts.leap_day {
                LeapDayPolicy::FoldToFeb28 => "fold-to-feb-28",
                LeapDayPolicy::Keep => "keep",
            },
            show_orphans: opts.show_orphans,
            dedupe_same: opts.dedupe_same,
        }
    }
}

/// Sort a hash set into the deterministic Vec form (rule 12).
fn sorted_vec(set: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = set.iter().cloned().collect();
    out.sort();
    out
}

/// The current loaded file's summary, if any (a snapshot — the state
/// locks are released before returning).
fn summary_of(state: &AppState) -> Option<LoadedSummary> {
    state.db.read().ok()?.as_ref().map(LoadedDatabase::summary)
}

/// The parsed database (a clone) or a 409 when nothing is loaded.
fn loaded_db(state: &AppState) -> Result<Database, WebError> {
    let guard = state.db.read().map_err(|_| WebError::Lock)?;
    guard
        .as_ref()
        .map(|l| l.db.clone())
        .ok_or(WebError::NotLoaded)
}

/// Did an HTMX request (as opposed to plain fetch / curl) send this?
fn is_hx(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

/// The shared `{"ok": true}` body.
fn json_ok() -> serde_json::Value {
    serde_json::json!({ "ok": true })
}

/// Render an Askama template, mapping render failures to a 500 HTML/JSON
/// error response.
fn render(t: impl askama::Template) -> Result<Response, WebError> {
    let html = t.render()?;
    Ok(Html(html).into_response())
}
