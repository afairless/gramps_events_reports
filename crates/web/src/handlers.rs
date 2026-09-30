//! The route handlers (plan §9 route table) and the small DTOs they
//! serve. Every handler keeps the 200 MB cap, the generated temp names
//! and the escape-by-default templates honest; none of them await while
//! holding a state lock.
//!
//! Milestone 14 (the full four-view UI) adds the `PUT /api/options`
//! route — the HTMX options form posts the checked event types, the
//! toggles and the reference year; the handler derives the new
//! [`ReportOptions`] (exclusion = every database type not checked, rule
//! 14) and answers with the *current* view's events fragment so HTMX
//! swaps in fresh content — and extends `GET /api/events` to all four
//! view tokens (`list`, `calendar`, `timeline`, `yrcal`), each fragment
//! rendered by the same event-core view builders the CLI and the writers
//! use (plan §5).

use std::collections::HashSet;
use std::io::Write as _;
use std::sync::Arc;

use axum::Json;
use axum::extract::{FromRequest, Multipart, Query, Request, State};
use axum::http::{HeaderMap, header};
use axum::response::{Html, IntoResponse, Response};
use event_core::{LeapDayPolicy, ReportOptions, View, ViewKind, build_view, collect_events, rows};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use writers::{
    CsvWriter, EventWriter, JsonWriter, ParquetWriter, PdfBackend, TypstPdf, build_pdf_document,
};

use crate::error::WebError;
use crate::state::{AppState, LoadedDatabase, LoadedSummary, count_event_types};
use crate::templates::{
    CalendarFragmentTemplate, EventsFragmentTemplate, FragmentRow, IndexTemplate, MainTemplate,
    TimelineFragmentTemplate, UiContext, YrcalFragmentTemplate, ui_context,
};
use gramps_xml::Database;

/// `GET /` — the server-rendered landing page (plan §9): the upload form
/// when nothing is loaded, the loaded-file summary + the full options
/// form, export button group and four view tabs otherwise. The page
/// survives refresh because the summary and options are rendered
/// server-side from the state.
pub async fn root(State(state): State<Arc<AppState>>) -> Result<Response, WebError> {
    render_index(&state)
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
        render_main(&state)
    } else {
        Ok(Json(summary).into_response())
    }
}

/// `GET /api/options` — the current run options as JSON (plan §9): the
/// reference year, the type include/exclude sets (rule 14), living-only,
/// privacy, the leap-day policy (D6) and the orphan toggle.
pub async fn options(State(state): State<Arc<AppState>>) -> Result<Json<OptionsDto>, WebError> {
    let guard = state.opts.read().map_err(|_| WebError::Lock)?;
    Ok(Json(OptionsDto::of(&guard)))
}

/// `PUT /api/options` — the HTMX options-form target (plan §9). The
/// form-encoded body carries the reference year, the checked event-type
/// names, the orphan/privacy/living-only/leap-day/dedupe toggles and the
/// active view. The new options are derived deterministically: unchecked
/// types of the loaded database go into `exclude_types` (exclusion wins,
/// rule 14), everything else rides through. The response is the current
/// view's events fragment, so a single request re-renders the tab HX
/// swapped it from.
pub async fn save_options(
    State(state): State<Arc<AppState>>,
    input: OptionsForm,
) -> Result<Response, WebError> {
    // The type universe comes from the loaded database; a 409 keeps the
    // options form honest until a file is loaded.
    let db = loaded_db(&state)?;
    let mut opts = state.opts.read().map_err(|_| WebError::Lock)?.clone();
    opts.reference_year = input.reference_year;
    opts.leap_day = match input.leap_day.as_deref() {
        Some("keep") => LeapDayPolicy::Keep,
        _ => LeapDayPolicy::FoldToFeb28,
    };
    opts.living_only = input.living_only;
    opts.include_private = input.include_private;
    opts.show_orphans = input.show_orphans;
    opts.dedupe_same = input.dedupe_same;
    let checked: HashSet<String> = input.types.into_iter().collect();
    opts.exclude_types = exclude_unchecked(&db, &checked);
    opts.include_types = None;
    {
        let mut guard = state.opts.write().map_err(|_| WebError::Lock)?;
        *guard = opts;
    }
    let kind = view_kind(input.view.as_deref())?;
    render_events_fragment(&state, kind)
}

/// `GET /api/events?view=list|calendar|timeline|yrcal` — the rendered
/// view as an HTML fragment (HTMX target swap, plan §9). Every fragment
/// is built from the shared event-core view builders, so the GUI renders
/// exactly the data the CLI and the file writers do. `list` is the
/// default.
pub async fn events(
    State(state): State<Arc<AppState>>,
    Query(query): Query<EventsQuery>,
) -> Result<Response, WebError> {
    let kind = view_kind(query.view.as_deref())?;
    render_events_fragment(&state, kind)
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
        render_main(&state)
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

/// `GET /static/style.css` — the authored UI stylesheet (plan §9 /
/// milestone 14): the four-view layout, the option controls and the
/// timeline range bars. Compiled into the binary like htmx.
pub async fn style() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../static/style.css"),
    )
}

/// The `view` query parameter of `GET /api/events`.
#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    /// The view token: `list` (default), `calendar`, `timeline` or
    /// `yrcal` (calendar-with-years).
    pub view: Option<String>,
}

/// The `format` query parameter of `GET /api/export`.
#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    /// The export format token: `csv`, `json`, `parquet` or `pdf`.
    pub format: Option<String>,
}

/// The parsed `PUT /api/options` form body. Parsed manually rather than
/// with serde_urlencoded's `Form<T>` because the checkbox list repeats the
/// `type` key: URL form encoders emit a single `type=Birth` for one
/// checked box and `type=a&type=b` for many, and serde_urlencoded 0.7
/// cannot map that shape onto a `Vec` field. See [`OptionsForm::from_request`].
#[derive(Debug, Default)]
pub struct OptionsForm {
    /// The report year every elapsed value is measured against (rule 4).
    pub reference_year: i32,
    /// The leap-day policy token: `"keep"` = keep Feb 29 (no fold); any
    /// other value (or absence) = the default fold policy (D6).
    pub leap_day: Option<String>,
    /// True when the "living only" toggle is checked (rule 15).
    pub living_only: bool,
    /// True when "include private records" is checked (§8.7).
    pub include_private: bool,
    /// True when "show orphans" is checked (D7 — checked by default).
    pub show_orphans: bool,
    /// True when "deduplicate identical rows" is checked (rule 6).
    pub dedupe_same: bool,
    /// The checked event-type names — every other type of the loaded
    /// database is excluded (rule 14).
    pub types: Vec<String>,
    /// The active view token, so the options update re-renders the same
    /// tab: `list` (default), `calendar`, `timeline` or `yrcal`.
    pub view: Option<String>,
}

impl<S> FromRequest<S> for OptionsForm
where
    S: Send + Sync,
{
    type Rejection = WebError;

    /// Split the `application/x-www-form-urlencoded` body on `&`/`=`,
    /// percent-decode each value and collect the repeated `type` keys
    /// into the allowed list. Present-but-empty controls (`living_only`,
    /// …) mean "checked" — the form emits the key with a `1` value for
    /// a checked box and omits it entirely for an unchecked one.
    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let content_type = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if !content_type
            .to_ascii_lowercase()
            .starts_with("application/x-www-form-urlencoded")
        {
            return Err(WebError::BadForm {
                detail: "expected application/x-www-form-urlencoded".into(),
            });
        }
        let body = String::from_request(req, state)
            .await
            .map_err(|_| WebError::BadForm {
                detail: "unreadable options form body".into(),
            })?;

        let mut form = OptionsForm::default();
        let mut reference_year: Option<i32> = None;
        for pair in body.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.split_once('=') {
                Some((key, value)) => (key, percent_decode(value)),
                None => (pair, String::new()),
            };
            match key {
                "reference_year" => {
                    reference_year = Some(value.parse::<i32>().map_err(|_| WebError::BadForm {
                        detail: "reference_year must be an integer".into(),
                    })?);
                }
                "leap_day" => form.leap_day = Some(value),
                "living_only" => form.living_only = true,
                "include_private" => form.include_private = true,
                "show_orphans" => form.show_orphans = true,
                "dedupe_same" => form.dedupe_same = true,
                "type" => form.types.push(value),
                "view" => form.view = Some(value),
                // Unknown controls are skipped — the server ignores what
                // the current form revision does not know.
                _ => {}
            }
        }
        form.reference_year = reference_year.ok_or(WebError::BadForm {
            detail: "missing reference_year".into(),
        })?;
        Ok(form)
    }
}

/// Decode one URL-form value: `+` → space, `%HH` → the byte it encodes.
/// Invalid `%` sequences pass through literally (they cannot come from the
/// authored form).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match (hex_value(bytes[i + 1]), hex_value(bytes[i + 2]))
            {
                (Some(hi), Some(lo)) => {
                    out.push(hi * 16 + lo);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The digit value of one hex nibble.
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
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

// ---------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------

/// The options form's state, snapshotted from the server state (the
/// locks are released before returning).
fn current_context(state: &AppState) -> UiContext {
    let opts = state
        .opts
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| ReportOptions::default());
    let db = match state.db.read() {
        Ok(guard) => guard.as_ref().map(|loaded| loaded.db.clone()),
        Err(_) => None,
    };
    ui_context(db.as_ref(), &opts)
}

/// Map a view token (`list`, `calendar`, `timeline`, `yrcal`) to its
/// [`ViewKind`]; anything else is an unknown-view 400.
fn view_kind(token: Option<&str>) -> Result<ViewKind, WebError> {
    match token {
        None | Some("") | Some("list") => Ok(ViewKind::List),
        Some("calendar") => Ok(ViewKind::Calendar),
        Some("timeline") => Ok(ViewKind::Timeline),
        Some("yrcal") => Ok(ViewKind::CalendarWithYears),
        Some(other) => Err(WebError::UnsupportedView {
            view: other.to_string(),
            supported: "list, calendar, timeline, yrcal",
        }),
    }
}

/// Derive the exclusion set from the checked boxed types: every event
/// type of the loaded database that is *not* checked is excluded (rule
/// 14 — exclusion wins; the checkbox list covers exactly the present
/// types, so the unchecked set is the full exclusion).
fn exclude_unchecked(db: &Database, checked: &HashSet<String>) -> HashSet<String> {
    let mut exclude: HashSet<String> = HashSet::new();
    for entry in count_event_types(db) {
        if !checked.contains(entry.event_type.as_str()) {
            exclude.insert(entry.event_type.clone());
        }
    }
    exclude
}

/// Render the landing page (`IndexTemplate` — includes `main.html`).
fn render_index(state: &Arc<AppState>) -> Result<Response, WebError> {
    let summary = summary_of(state);
    let context = current_context(state);
    render(IndexTemplate {
        loaded: summary.as_ref(),
        context: &context,
    })
}

/// Render the `main.html` content partial (the HTMX swap target of the
/// load/reset routes).
fn render_main(state: &Arc<AppState>) -> Result<Response, WebError> {
    let summary = summary_of(state);
    let context = current_context(state);
    render(MainTemplate {
        loaded: summary.as_ref(),
        context: &context,
    })
}

/// Build and render the events fragment for `kind` — the shared
/// event-core view builders, shaped by the fragment templates (plan §5:
/// the GUI renders exactly what the CLI prints and the writers dump).
fn render_events_fragment(state: &Arc<AppState>, kind: ViewKind) -> Result<Response, WebError> {
    let opts = state.opts.read().map_err(|_| WebError::Lock)?.clone();
    let db = loaded_db(state)?;
    let built = build_view(&collect_events(&db, &opts), kind, &opts);
    match built {
        View::List(list) => {
            let frag_rows: Vec<FragmentRow> = list.rows.iter().map(FragmentRow::of).collect();
            render(EventsFragmentTemplate {
                rows: frag_rows,
                reference_year: opts.reference_year,
            })
        }
        View::Calendar(calendar) => {
            render(CalendarFragmentTemplate::of(&calendar, opts.reference_year))
        }
        View::Timeline(timeline) => {
            render(TimelineFragmentTemplate::of(&timeline, opts.reference_year))
        }
        View::CalendarWithYears(yrcal) => {
            render(YrcalFragmentTemplate::of(&yrcal, opts.reference_year))
        }
    }
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

/// Sort a hash set into the deterministic Vec form (rule 12).
fn sorted_vec(set: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = set.iter().cloned().collect();
    out.sort();
    out
}

/// Render an Askama template, mapping render failures to a 500 HTML/JSON
/// error response.
fn render(t: impl askama::Template) -> Result<Response, WebError> {
    let html = t.render()?;
    Ok(Html(html).into_response())
}
