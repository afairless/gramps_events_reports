//! Integration tests for the web API and the full four-view UI (plan §11
//! web tests, milestone 13 + 14): the router is driven exactly as an HTTP
//! client would be — Tower `ServiceExt::oneshot` — plus one real-listener
//! test that proves the server answers over loopback TCP.
//!
//! Coverage: landing page + vendored htmx; the load lifecycle (JSON and
//! HTMX responses, the 200 MB cap → 413, generated temp names, parse
//! failures, reset cleanup); options/events/events.json/export routes;
//! the milestone-14 four-view UI (all four view tokens render fragments
//! from the shared event-core view builders, event-type checkboxes with
//! counts, orphan/privacy/leap-day/living-only toggles, export buttons,
//! CSS range bars and the stylesheet); the options form (rule-14
//! exclusion from the checked types, reference year, toggle round-trips);
//! the not-loaded 409s and the unknown-view 400; and the escaping
//! regression — hostile `<script>`-bearing names/types/dates render
//! `&lt;script&gt;`-escaped in **every** view fragment (plan §11).

use std::io::{Read as _, Write as _};
use std::net::Ipv4Addr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use bytes::Bytes;
use http_body_util::BodyExt;
use tower::ServiceExt;
use web::router;
use web::state::{AppState, UPLOAD_CAP_BYTES};

// The canonical workspace fixture: 5 people, 6 events (plan §11).
const DATA_GRAMPS: &[u8] = include_bytes!("../../../tests/fixtures/data.gramps");

// The crafted date-form fixture: modifiers, partial dates, BC dates,
// non-Gregorian calendars, daterange / datespan / datestr forms — the
// ranges and text-only dates the range-bar and anchor tests assert on
// (plan §11).
const DATES_GRAMPS: &[u8] = include_bytes!("../../../tests/fixtures/dates.gramps");

/// A minimal Gramps file carrying hostile markup in person names, event
/// types and a text date — the escape-regression fixture: the decoded
/// strings contain real `<script>` tags that must never reach any view
/// fragment raw. `_e1` is undated (list + timeline only); `_e2` carries
/// a dated 1999-01-01 event with hostile name and type, so the three
/// grid views (calendar, yrcal) and the timeline exercise the escaping
/// too.
const HOSTILE_GRAMPS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE database PUBLIC "-//Gramps//DTD Gramps XML 1.7.1//EN"
"http://gramps-project.org/xml/1.7.1/grampsxml.dtd">
<database xmlns="http://gramps-project.org/xml/1.7.1/">
  <header>
    <created date="2026-01-01" version="5.1.6"/>
    <researcher/>
  </header>
  <events>
    <event handle="_e1" change="1" id="E0000">
      <type>&lt;script&gt;alert(1)&lt;/script&gt;</type>
      <datestr val="&lt;img src=x onerror=alert(2)&gt;"/>
    </event>
    <event handle="_e2" change="2" id="E0001">
      <type>&lt;script&gt;alert(4)&lt;/script&gt;</type>
      <dateval val="1999-01-01"/>
    </event>
  </events>
  <people>
    <person handle="_p1" change="1" id="I0000">
      <gender>M</gender>
      <name type="Birth Name">
        <first>&lt;script&gt;alert(3)&lt;/script&gt;</first>
        <surname>Meowser</surname>
      </name>
      <eventref hlink="_e1" role="Primary"/>
    </person>
    <person handle="_p2" change="2" id="I0001">
      <gender>M</gender>
      <name type="Birth Name">
        <first>&lt;script&gt;alert(5)&lt;/script&gt;</first>
        <surname>Hostile</surname>
      </name>
      <eventref hlink="_e2" role="Primary"/>
    </person>
  </people>
</database>
"#;

/// A fresh router + state; `cap` lets tests shrink the upload cap so the
/// 413 branch is reachable cheaply.
fn setup(cap: usize) -> (Router, Arc<AppState>) {
    let state = Arc::new(AppState::with_cap(cap).unwrap());
    (router(state.clone()), state)
}

/// Build a request. `headers` is `(name, value)` pairs; `content_type`
/// is the common case.
fn request(
    method: Method,
    uri: &str,
    content_type: Option<&str>,
    extra: &[(&str, &str)],
    body: Vec<u8>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(ct) = content_type {
        builder = builder.header(header::CONTENT_TYPE, ct);
    }
    for (name, value) in extra {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::from(body)).unwrap()
}

/// Drive the router with `oneshot` and return (status, headers, body).
async fn call(app: &Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Bytes) {
    let response = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = response.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (parts.status, parts.headers, bytes)
}

/// A one-file multipart body for the upload route; returns the
/// `Content-Type` to send alongside it.
fn multipart_body(field: &str, filename: &str, content: &[u8]) -> (String, Vec<u8>) {
    let boundary = "test-boundary-7d1ca5e9";
    let mut body = Vec::new();
    write!(
        body,
        "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; \
         filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .unwrap();
    body.extend_from_slice(content);
    write!(body, "\r\n--{boundary}--\r\n").unwrap();
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn load(app: &Router, content: &[u8], filename: &str) -> (StatusCode, Bytes) {
    let (ct, body) = multipart_body("gramps", filename, content);
    let (status, _, bytes) = call(
        app,
        request(Method::POST, "/api/load", Some(&ct), &[], body),
    )
    .await;
    (status, bytes)
}

// ---------------------------------------------------------------- routes

#[tokio::test]
async fn landing_page_renders_and_includes_vendored_htmx() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, headers, body) =
        call(&app, request(Method::GET, "/", None, &[], Vec::new())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("gramps-events"));
    assert!(
        text.contains("src=\"/static/htmx.min.js\""),
        "htmx vendored script is linked"
    );
}

#[tokio::test]
async fn vendored_htmx_is_served_from_the_binary() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, headers, body) = call(
        &app,
        request(Method::GET, "/static/htmx.min.js", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/javascript")
    );
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(!text.is_empty());
    assert!(
        text.starts_with("var htmx="),
        "the vendored 2.x distribution's opening line"
    );
}

// ------------------------------------------------------------ load route

#[tokio::test]
async fn load_returns_the_json_summary() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, body) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // {event_types: [{type, count}], people_count, …} (plan §9)
    assert_eq!(value["people_count"], 5);
    assert_eq!(value["event_count"], 6);
    assert_eq!(value["file_name"], "data.gramps");
    let types = value["event_types"].as_array().unwrap();
    assert!(!types.is_empty());
    assert!(
        types
            .iter()
            .all(|t| t["type"].is_string() && t["count"].is_u64())
    );
    assert!(types.iter().any(|t| t["type"] == "Birth"));
}

#[tokio::test]
async fn load_replaces_a_previous_file_and_deletes_its_upload() {
    let (app, state) = setup(UPLOAD_CAP_BYTES);
    let (status, body) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    // counts after the first load
    let first: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(first["people_count"], 5);
    let files_before = std::fs::read_dir(state.temp_dir.path()).unwrap().count();
    assert_eq!(files_before, 1);

    // reload the same bytes under a different name
    let (status, _) = load(&app, DATA_GRAMPS, "again.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let files_after = std::fs::read_dir(state.temp_dir.path()).unwrap().count();
    assert_eq!(files_after, 1, "the replaced upload was deleted");
}

#[tokio::test]
async fn load_with_hx_request_returns_the_main_fragment() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (ct, body) = multipart_body("gramps", "data.gramps", DATA_GRAMPS);
    let (status, headers, bytes) = call(
        &app,
        request(
            Method::POST,
            "/api/load",
            Some(&ct),
            &[("hx-request", "true")],
            body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("data.gramps"));
    assert!(text.contains("Event types"), "fragment lists the types");
}

#[tokio::test]
async fn load_enforces_the_upload_cap_before_reading_into_memory() {
    // A tiny cap (64 KB) reaches the 413 branch without allocating
    // hundreds of megabytes; the fixture is repeated to exceed it.
    let (app, state) = setup(64 * 1024);
    let mut huge = Vec::new();
    for _ in 0..20 {
        huge.extend_from_slice(DATA_GRAMPS); // ~138 KB total
    }
    let (status, body) = load(&app, &huge, "data.gramps").await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("cap"), "the error names the cap: {text}");
    // nothing was stored, no temp file leaked
    assert_eq!(std::fs::read_dir(state.temp_dir.path()).unwrap().count(), 0);
    let (status, _, _) = call(
        &app,
        request(Method::GET, "/api/events", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "still nothing loaded");
}

#[tokio::test]
async fn load_without_a_file_field_is_400() {
    let (app, state) = setup(UPLOAD_CAP_BYTES);
    let boundary = "test-boundary-no-file";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhello\r\n--{boundary}--\r\n"
    );
    let (status, _, _) = call(
        &app,
        request(
            Method::POST,
            "/api/load",
            Some(&format!("multipart/form-data; boundary={boundary}")),
            &[],
            body.into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(std::fs::read_dir(state.temp_dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn load_rejects_unparseable_bytes_with_422() {
    let (app, state) = setup(UPLOAD_CAP_BYTES);
    let (status, body) = load(&app, b"definitely not a gramps file", "junk.gramps").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("Gramps"));
    // the failed upload was cleaned up
    assert_eq!(std::fs::read_dir(state.temp_dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn uploads_use_generated_temp_names_and_reset_cleans_up() {
    let (app, state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);

    // exactly one file in the temp dir, under a generated name — never
    // the browser-supplied filename (plan §9).
    let mut entries = std::fs::read_dir(state.temp_dir.path()).unwrap();
    let entry = entries.next().unwrap().unwrap();
    assert!(entries.next().is_none(), "exactly one upload file");
    assert_ne!(
        entry.file_name().to_str(),
        Some("data.gramps"),
        "the browser filename must never become a path"
    );
    assert!(entry.path().exists());

    // reset unloads and deletes the upload
    let (status, _, _) = call(
        &app,
        request(Method::POST, "/api/reset", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(std::fs::read_dir(state.temp_dir.path()).unwrap().count(), 0);
    let (status, _, _) = call(
        &app,
        request(Method::GET, "/api/events", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn reset_returns_the_upload_form_fragment_to_htmx() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _, bytes) = call(
        &app,
        request(
            Method::POST,
            "/api/reset",
            None,
            &[("hx-request", "true")],
            Vec::new(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("type=\"file\""), "the upload form comes back");
}

// --------------------------------------------------------- other routes

#[tokio::test]
async fn options_returns_the_current_options_as_json() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, headers, body) = call(
        &app,
        request(Method::GET, "/api/options", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(value["reference_year"].is_i64());
    assert!(value["include_types"].is_null(), "default = all types");
    assert_eq!(value["exclude_types"], serde_json::json!([]));
    assert_eq!(value["living_only"], false);
    assert_eq!(value["include_private"], false);
    assert_eq!(value["leap_day"], "fold-to-feb-28");
    assert_eq!(value["show_orphans"], true);
    assert_eq!(value["dedupe_same"], false);
}

/// Form-encode a `PUT /api/options` body and return the fragment.
async fn put_options(app: &Router, body: &str) -> (StatusCode, axum::http::HeaderMap, Bytes) {
    call(
        app,
        request(
            Method::PUT,
            "/api/options",
            Some("application/x-www-form-urlencoded"),
            &[],
            body.as_bytes().to_vec(),
        ),
    )
    .await
}

#[tokio::test]
async fn options_form_excludes_unchecked_types_rule_14() {
    // Rule 14 from the checkbox list: only the *checked* types survive —
    // sending just `type=Birth` excludes Death (exclusion wins). The
    // response is the re-rendered fragment of the active tab (the HTMX
    // swap target), so the fragment must reflect the new options.
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = put_options(&app, "reference_year=2030&view=list&type=Birth").await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        text.contains("2000-03-03"),
        "the surviving Birth row: {text}"
    );
    assert!(
        !text.contains("2020-12-03"),
        "the unchecked Death row was excluded: {text}"
    );
    assert!(text.contains("Reference year: 2030"));
}

#[tokio::test]
async fn options_form_can_exclude_every_type() {
    // No types checked → the exclusion set is the whole type universe →
    // the empty-state hint renders (rule 14 + checkbox enumeration).
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = put_options(&app, "reference_year=2030&view=list").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        String::from_utf8(body.to_vec())
            .unwrap()
            .contains("No events match the current options.")
    );
}

#[tokio::test]
async fn options_form_re_renders_the_active_tab_and_round_trips() {
    // The hidden `view` field keeps the options update on the active tab:
    // a PUT with view=calendar answers with the calendar fragment. The
    // toggles and reference year survive into `GET /api/options`.
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let body = "reference_year=2030&view=calendar&type=Birth&type=Death&show_orphans=1&include_private=1&leap_day=keep&dedupe_same=1&living_only=1";
    let (status, _, bytes) = put_options(&app, body).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .contains("Anniversary calendar"),
        "the PUT re-renders the active tab's fragment"
    );

    let (status, _, body) = call(
        &app,
        request(Method::GET, "/api/options", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["reference_year"], 2030);
    assert_eq!(value["show_orphans"], true);
    assert_eq!(value["include_private"], true);
    assert_eq!(value["living_only"], true);
    assert_eq!(value["leap_day"], "keep");
    assert_eq!(value["dedupe_same"], true);
    assert_eq!(
        value["exclude_types"],
        serde_json::json!([]),
        "both types checked → nothing excluded"
    );
}

#[tokio::test]
async fn options_form_requires_a_loaded_file() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _, _) = put_options(&app, "reference_year=2030&view=list").await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn events_fragment_renders_the_list_view() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let (status, headers, body) = call(
        &app,
        request(Method::GET, "/api/events?view=list", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("Harry Meowser"));
    assert!(text.contains("Birth"));
    assert!(text.contains("2000-03-03"));
    assert!(text.contains("Reference year:"));
}

#[tokio::test]
async fn events_fragment_defaults_to_the_list_view() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = call(
        &app,
        request(Method::GET, "/api/events", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        String::from_utf8(body.to_vec())
            .unwrap()
            .contains("Harry Meowser")
    );
}

#[tokio::test]
async fn all_four_view_tokens_render_fragments_from_the_shared_builders() {
    // Milestone 14: every view token renders a fragment now — the GUI
    // tabs call exactly these endpoints (plan §9 `GET /api/events?view=…`),
    // each fragment built by the same event-core view builders the CLI
    // and the writers use (plan §5).
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    for (view, marker) in [
        ("list", "Harry Meowser"),
        ("calendar", "Anniversary calendar"),
        ("timeline", "Timeline"),
        ("yrcal", "Calendar with years"),
    ] {
        let (status, headers, body) = call(
            &app,
            request(
                Method::GET,
                &format!("/api/events?view={view}"),
                None,
                &[],
                Vec::new(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "view {view}");
        assert!(
            headers[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html"),
            "view {view} is a fragment"
        );
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains(marker), "view {view} marker: {text}");
        assert!(
            text.contains("id=\"events\""),
            "view {view} is an HTMX swap target"
        );
    }
}

#[tokio::test]
async fn calendar_view_anchors_events_to_their_month_and_day() {
    // data.gramps births anchor on (month, day) per rule 1: 2000-03-03 →
    // the March page, day 3; the elapsed is 2026 − 2000 = 26 years
    // (reference year = the current year, like the CLI).
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = call(
        &app,
        request(
            Method::GET,
            "/api/events?view=calendar",
            None,
            &[],
            Vec::new(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("<h3>March</h3>"), "March page: {text}");
    assert!(
        text.contains("<th scope=\"row\">3</th>"),
        "day 3 cell: {text}"
    );
    assert!(text.contains("Harry Meowser — Birth (2000-03-03)"));
    assert!(text.contains("26 years"), "elapsed vs the reference year");
}

#[tokio::test]
async fn timeline_view_groups_events_under_year_headers() {
    // Rule 10: chronological year groups; data.gramps spans 1938–2020.
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = call(
        &app,
        request(
            Method::GET,
            "/api/events?view=timeline",
            None,
            &[],
            Vec::new(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body.to_vec()).unwrap();
    for year in ["1938", "1946", "1970", "2000", "2020"] {
        assert!(
            text.contains(&format!("<h3>{year}</h3>")),
            "year header {year}: {text}"
        );
    }
    assert!(text.contains("Harry Meowser — Birth (2000-03-03)"));
}

#[tokio::test]
async fn yrcal_view_lays_out_year_pages_with_actual_dates() {
    // Rule 11 / D12: year-by-year pages, events on their actual dates.
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = call(
        &app,
        request(Method::GET, "/api/events?view=yrcal", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("<h3>2000</h3>"), "year page 2000: {text}");
    assert!(text.contains("<h4>March</h4>"), "March cell: {text}");
    assert!(text.contains("Harry Meowser — Birth (2000-03-03)"));
}

#[tokio::test]
async fn unknown_view_tokens_remain_a_400() {
    // Milestone 14 implements exactly the four tokens; anything else is
    // still rejected (the option form can only emit the four tabs).
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    for view in ["bogus", "CALENDAR"] {
        let (status, _, body) = call(
            &app,
            request(
                Method::GET,
                &format!("/api/events?view={view}"),
                None,
                &[],
                Vec::new(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "view {view}");
        assert!(
            String::from_utf8(body.to_vec())
                .unwrap()
                .contains("unsupported view"),
            "view {view} error names the token"
        );
    }
}

// -------------------------------------- range bars & anchor rules (dates.gramps)

#[tokio::test]
async fn timeline_range_events_render_as_css_bars() {
    // The milestone-14 range bars (rule 10 / D11, plan §8): the cross-year
    // Marriage 1914-07-28 → 1918-11-11 spans past its group (`bar-spans`,
    // July-28 start geometry, "→ 1918" note); the Immigration span
    // 1925-06-01 → 1925-08-31 is a same-year bar; the year-only Marriage
    // 1822–1824 is a full-width bar; Graduation's text-only date lands in
    // the terminal "Undated" group (rule 13).
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATES_GRAMPS, "dates.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = call(
        &app,
        request(
            Method::GET,
            "/api/events?view=timeline",
            None,
            &[],
            Vec::new(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        text.contains("bar bar-spans"),
        "cross-year/full-year bars: {text}"
    );
    assert!(
        text.contains("left: 57%; width: 42%;"),
        "July-28 start geometry: {text}"
    );
    assert!(text.contains("→ 1918"), "stop-year note: {text}");
    assert!(text.contains("Immigration (1925-06-01 - 1925-08-31)"));
    assert!(
        text.contains("<h3>Undated</h3>"),
        "rule-13 terminal group: {text}"
    );
    assert!(
        text.contains("Graduation"),
        "text-only date → undated group"
    );
}

#[tokio::test]
async fn calendar_anchors_range_starts_and_skips_year_only_ranges() {
    // Rules 1/2/9/10: a range whose start has a month anchors at the start
    // (day 1 when the day is missing — the Marriage 1822-11 anchors on
    // November 1); year-only ranges (Marriage 1822–1824) and text-only
    // dates never appear in this view.
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATES_GRAMPS, "dates.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = call(
        &app,
        request(
            Method::GET,
            "/api/events?view=calendar",
            None,
            &[],
            Vec::new(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("<h3>November</h3>"), "Nov page: {text}");
    assert!(
        text.contains("<th scope=\"row\">1</th>"),
        "day-1 anchor: {text}"
    );
    assert!(
        text.contains("li class=\"range\""),
        "ranges are flagged: {text}"
    );
    assert!(text.contains("Anniversary calendar"));
}

#[tokio::test]
async fn yrcal_shows_year_only_events_as_full_year_rows() {
    // Rule 11: year-only rows (the 1822–1824 Marriage, the year-only Birth
    // 1822) render as full-year bars across the year page; dated events
    // sit in their month/day cells (Marriage 1914-07-28 → July).
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATES_GRAMPS, "dates.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = call(
        &app,
        request(Method::GET, "/api/events?view=yrcal", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        text.contains("class=\"year-bar\""),
        "full-year rows: {text}"
    );
    assert!(text.contains("<h3>1914</h3>"), "1914 page: {text}");
    assert!(text.contains("<h4>July</h4>"), "July cell: {text}");
}

#[tokio::test]
async fn events_json_returns_event_rows() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let (status, headers, body) = call(
        &app,
        request(Method::GET, "/api/events.json", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let rows = value.as_array().unwrap();
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|r| r["person_name"].is_string()));
    assert!(rows.iter().any(|r| r["event_type"] == "Birth"));
}

/// One export case: (format token, expected content-type prefix, magic
/// check) — factored out so the table stays readable.
type ExportCase = (&'static str, &'static str, &'static dyn Fn(&Bytes) -> bool);

#[tokio::test]
async fn export_streams_every_format_as_a_download() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);

    let cases: &[ExportCase] = &[
        ("csv", "text/csv", &|b: &Bytes| {
            b.starts_with(b"person_id,person_name")
        }),
        ("json", "application/json", &|b: &Bytes| b.starts_with(b"[")),
        ("parquet", "application/octet-stream", &|b: &Bytes| {
            b.starts_with(b"PAR1")
        }),
        ("pdf", "application/pdf", &|b: &Bytes| {
            b.starts_with(b"%PDF-")
        }),
    ];
    for (format, content_type, magic) in cases {
        let (status, headers, body) = call(
            &app,
            request(
                Method::GET,
                &format!("/api/export?format={format}"),
                None,
                &[],
                Vec::new(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "format {format}");
        assert!(
            headers[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with(content_type),
            "format {format} content type"
        );
        let disposition = headers[header::CONTENT_DISPOSITION].to_str().unwrap();
        assert!(
            disposition.starts_with("attachment"),
            "format {format} is a download"
        );
        assert!(
            disposition.contains(&format!("gramps-events.{format}")),
            "format {format} filename"
        );
        assert!(magic(&body), "format {format} magic header");
    }
}

#[tokio::test]
async fn exported_pdf_is_not_blank() {
    // Mandatory web-side lock on the writers-crate content guarantee: the
    // export route must serve a PDF that embeds fonts. Font objects and
    // page resource dictionaries are written uncompressed, so a raw
    // `/Font` byte probe discriminates the blank-PDF regression (0 hits —
    // empty font book, no drawn text) from a real export, while the
    // FlateDecode-compressed content streams make a `Tj`/`TJ` scan
    // useless even on a correct PDF.
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    let (status, headers, body) = call(
        &app,
        request(Method::GET, "/api/export?format=pdf", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/pdf")
    );
    assert!(
        body.to_vec().windows(5).any(|w| w == b"/Font"),
        "the exported PDF must embed font resources (/Font)"
    );
}

#[tokio::test]
async fn export_unknown_format_is_400_and_missing_format_is_400() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    for uri in ["/api/export?format=xlsx", "/api/export"] {
        let (status, _, _) = call(&app, request(Method::GET, uri, None, &[], Vec::new())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
    }
}

#[tokio::test]
async fn data_routes_return_409_before_any_load() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    for uri in ["/api/events", "/api/events.json", "/api/export?format=csv"] {
        let (status, _, body) = call(&app, request(Method::GET, uri, None, &[], Vec::new())).await;
        assert_eq!(status, StatusCode::CONFLICT, "{uri}");
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("no Gramps file loaded"), "{uri}: {text}");
    }
    // options and reset need no file at all.
    let (status, _, _) = call(
        &app,
        request(Method::GET, "/api/options", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = call(
        &app,
        request(Method::POST, "/api/reset", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value, serde_json::json!({"ok": true}));
}

// --------------------------------------------------------- escaping test

#[tokio::test]
async fn every_view_fragment_escapes_hostile_names_types_and_dates() {
    // The escaping regression (plan §11): Askama's default HTML escape
    // emits decimal numeric character references (`&#60;`/`&#62;` for
    // `<`/`>`). The hostile strings travelled through XML entities, the
    // model, the view builders and the row contract as literal
    // `<script>` … and must come out entity-escaped in **every** view
    // fragment — the dated `_e2` row (hostile name + type) appears in all
    // four fragments, so each one is asserted.
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, body) = load(&app, HOSTILE_GRAMPS.as_bytes(), "escaped.gramps").await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    for view in ["list", "calendar", "timeline", "yrcal"] {
        let (status, _, body) = call(
            &app,
            request(
                Method::GET,
                &format!("/api/events?view={view}"),
                None,
                &[],
                Vec::new(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "view {view}");
        let text = String::from_utf8(body.to_vec()).unwrap();

        // The dated hostile row (_e2: 1999-01-01) is in every fragment.
        assert!(
            text.contains("&#60;script&#62;alert(5)&#60;/script&#62; Hostile"),
            "view {view}: hostile name escaped: {text}"
        );
        assert!(
            text.contains("&#60;script&#62;alert(4)&#60;/script&#62;"),
            "view {view}: hostile type escaped: {text}"
        );

        // The undated row (_e1: datestr only) reaches list + timeline
        // (rule 13), never the two calendars — assert it where it is.
        if view == "list" || view == "timeline" {
            assert!(
                text.contains("&#60;script&#62;alert(3)&#60;/script&#62;"),
                "view {view}: hostile name escaped: {text}"
            );
            assert!(
                text.contains("&#60;script&#62;alert(1)&#60;/script&#62;"),
                "view {view}: hostile type escaped: {text}"
            );
            assert!(
                text.contains("&#60;img src=x onerror=alert(2)&#62;"),
                "view {view}: hostile date text escaped: {text}"
            );
        }

        assert!(
            !text.contains("<script>"),
            "view {view}: no raw <script> markup may reach the fragment: {text}"
        );
        assert!(
            !text.contains("<img "),
            "view {view}: no raw attribute-bearing element may reach the fragment: {text}"
        );
    }
}

// -------------------------------------------------- milestone-14 UI chrome

#[tokio::test]
async fn loaded_page_renders_four_tabs_checkboxes_toggles_and_exports() {
    // The milestone-14 UI (plan §9): after a load the HTMX fragment
    // carries the four view tabs, the event-type checkboxes (with the
    // same enumeration `inspect` prints), the orphan/privacy/leap-day/
    // living-only/dedupe toggles, and the export button group — one link
    // per format plus the raw events.json inspection endpoint.
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (ct, body) = multipart_body("gramps", "data.gramps", DATA_GRAMPS);
    let (status, _, bytes) = call(
        &app,
        request(
            Method::POST,
            "/api/load",
            Some(&ct),
            &[("hx-request", "true")],
            body,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(bytes.to_vec()).unwrap();

    // Four tabs, wired to the four fragment endpoints.
    for (view, label) in [
        ("list", "List"),
        ("calendar", "Anniversary calendar"),
        ("timeline", "Timeline"),
        ("yrcal", "Calendar with years"),
    ] {
        assert!(
            text.contains(&format!("data-view=\"{view}\"")),
            "tab {view}: {text}"
        );
        assert!(text.contains(label), "tab label {label}: {text}");
        assert!(
            text.contains(&format!("/api/events?view={view}")),
            "tab {view} hits the fragment endpoint"
        );
    }

    // Event-type checkboxes with counts.
    assert!(text.contains("Event types"));
    assert!(
        text.contains("name=\"type\" value=\"Birth\""),
        "Birth checkbox: {text}"
    );
    assert!(
        text.contains("name=\"type\" value=\"Death\""),
        "Death checkbox: {text}"
    );

    // The five toggles.
    for toggle in [
        "show_orphans",
        "include_private",
        "living_only",
        "leap_day",
        "dedupe_same",
    ] {
        assert!(text.contains(toggle), "toggle {toggle}: {text}");
    }

    // Export button group — one link per format plus events.json.
    for format in ["csv", "json", "parquet", "pdf"] {
        assert!(
            text.contains(&format!("/api/export?format={format}")),
            "export {format}: {text}"
        );
    }
    assert!(text.contains("/api/events.json"));

    // The checkboxes reflect the current options (default = all types
    // admitted + orphans shown): orphan (default) + both fixture types
    // = at least three checked boxes.
    assert!(
        text.matches("checked").count() >= 3,
        "default options pre-check their boxes: {text}"
    );
}

#[tokio::test]
async fn stylesheet_is_served_with_the_four_view_and_range_bar_css() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, headers, body) = call(
        &app,
        request(Method::GET, "/static/style.css", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/css")
    );
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains(".timeline-body .bar"), "the CSS range bars");
    assert!(text.contains(".tab.active"), "tab styling");
    assert!(text.contains(".type .count"), "checkbox count pills");
}

// ----------------------------------------------------- real-listener test

// Multi-thread runtime: the test body synchronously reads the socket
// (`read_to_end`) while the spawned axum server task must keep getting
// polled on another worker thread — a current-thread runtime would
// deadlock.
#[tokio::test(flavor = "multi_thread")]
async fn router_answers_real_http_on_loopback() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let std_listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = std_listener.local_addr().unwrap();
    assert!(
        addr.ip().is_loopback(),
        "the server speaks loopback only (plan §9)"
    );
    std_listener.set_nonblocking(true).unwrap();
    let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();

    let (tx, rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });

    // A raw HTTP/1.1 request over TCP — no test HTTP client involved.
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf);
    assert!(text.starts_with("HTTP/1.1 200"), "raw response: {text}");
    assert!(text.contains("gramps-events"));
    assert!(text.contains("htmx.min.js"));

    let _ = tx.send(());
    server.await.unwrap();
}
