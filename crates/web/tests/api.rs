//! Integration tests for the web API + skeleton UI (plan §11 web tests,
//! milestone 13): the router is driven exactly as an HTTP client would
//! be — Tower `ServiceExt::oneshot` — plus one real-listener test that
//! proves the server answers over loopback TCP.
//!
//! Coverage: landing page + vendored htmx; the load lifecycle (JSON and
//! HTMX responses, the 200 MB cap → 413, generated temp names, parse
//! failures, reset cleanup); options/events/events.json/export routes;
//! the not-loaded 409s; the unsupported-view 400 of this milestone's
//! scope; and the escaping regression — hostile `<script>`-bearing
//! names/types/dates render `&lt;script&gt;`-escaped in the view
//! fragment (plan §11: "an escaping test asserts that `<script>`-bearing
//! names/dates render escaped in every view fragment").

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

/// A minimal Gramps file carrying hostile markup in a person name, an
/// event type and a text date — the escape-regression fixture: the
/// decoded strings contain real `<script>` tags that must never reach the
/// fragment raw.
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
async fn unsupported_views_are_400_until_milestone_14() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, _) = load(&app, DATA_GRAMPS, "data.gramps").await;
    assert_eq!(status, StatusCode::OK);
    for view in ["calendar", "timeline", "yrcal"] {
        let (status, _, _) = call(
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
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "view {view} not in this milestone"
        );
    }
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
async fn events_fragment_escapes_hostile_names_types_and_dates() {
    let (app, _state) = setup(UPLOAD_CAP_BYTES);
    let (status, body) = load(&app, HOSTILE_GRAMPS.as_bytes(), "escaped.gramps").await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let (status, _, body) = call(
        &app,
        request(Method::GET, "/api/events?view=list", None, &[], Vec::new()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body.to_vec()).unwrap();

    // The hostile strings travelled through XML entities, the model, the
    // view builder and the row contract as literal `<script>` … and must
    // come out entity-escaped. Askama 0.16's default HTML escape emits
    // decimal numeric character references (`&#60;`/`&#62;` for
    // `<`/`>`) — the regression test locks the resulting bytes in.
    assert!(
        text.contains("&#60;script&#62;alert(3)&#60;/script&#62;"),
        "hostile name escaped: {text}"
    );
    assert!(
        text.contains("&#60;script&#62;alert(1)&#60;/script&#62;"),
        "hostile type escaped: {text}"
    );
    assert!(
        text.contains("&#60;img src=x onerror=alert(2)&#62;"),
        "hostile date text escaped: {text}"
    );
    assert!(
        !text.contains("<script>"),
        "no raw <script> markup may reach the fragment: {text}"
    );
    assert!(
        !text.contains("<img "),
        "no raw attribute-bearing element may reach the fragment: {text}"
    );
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
