//! The web API's error type — `WebError` — and its HTTP mapping (plan
//! §9). Every failure mode of the routes (not-loaded, upload cap, parse,
//! unknown view/format, template, IO) renders as a JSON `{"error":
//! "…"}` body with an appropriate status instead of a bare 500.

use axum::Json;
use axum::extract::multipart::MultipartError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// An API failure. Each variant maps to a status code in
/// [`IntoResponse`] below; the JSON body always carries a human-readable
/// `error` message.
#[derive(Debug, thiserror::Error)]
pub enum WebError {
    /// Most `/api/*` routes require a loaded database.
    #[error("no Gramps file loaded — POST /api/load first")]
    NotLoaded,
    /// The upload exceeded the hard cap (200 MB per plan §9, or the
    /// state's test cap).
    #[error("upload exceeds the {cap} byte cap")]
    TooLarge { cap: usize },
    /// The multipart request carried no file field.
    #[error("no file part in the upload")]
    NoFile,
    /// The uploaded bytes could not be parsed as a Gramps database.
    #[error("not a readable Gramps file: {0}")]
    Parse(#[from] gramps_xml::GrampsXmlError),
    /// `GET /api/events` got a view this milestone does not render yet.
    #[error("unsupported view {view:?} — this milestone implements only {supported}")]
    UnsupportedView {
        /// The requested view token.
        view: String,
        /// The views that are implemented.
        supported: &'static str,
    },
    /// `GET /api/export` got an unknown format token.
    #[error("unknown export format {format:?} — expected csv, json, parquet or pdf")]
    UnknownFormat {
        /// The offending token.
        format: String,
    },
    /// The options form body was malformed or missing a required field.
    #[error("malformed options form: {detail}")]
    BadForm {
        /// What went wrong.
        detail: String,
    },
    /// The multipart body itself failed to stream.
    #[error("upload stream failed: {0}")]
    Multipart(#[from] MultipartError),
    /// The server state's lock was poisoned.
    #[error("server state lock poisoned")]
    Lock,
    /// An Askama template failed to render.
    #[error("template rendering failed: {0}")]
    Template(#[from] askama::Error),
    /// A writer failed while producing an export.
    #[error(transparent)]
    Writer(#[from] writers::WriterError),
    /// Underlying IO (temp files, uploads, exports).
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The JSON error body.
#[derive(Debug, Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let status = match self {
            WebError::NotLoaded => StatusCode::CONFLICT,
            WebError::TooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            WebError::NoFile
            | WebError::UnsupportedView { .. }
            | WebError::UnknownFormat { .. } => StatusCode::BAD_REQUEST,
            WebError::Parse(_) | WebError::Multipart(_) | WebError::BadForm { .. } => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            WebError::Lock | WebError::Template(_) | WebError::Writer(_) | WebError::Io(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        let message = self.to_string();
        (status, Json(ErrorBody { error: &message })).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_loaded_maps_to_409() {
        let resp = WebError::NotLoaded.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn too_large_maps_to_413_with_the_cap() {
        let resp = WebError::TooLarge { cap: 1024 }.into_response();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn parse_error_maps_to_422() {
        let resp = WebError::Parse(gramps_xml::GrampsXmlError::EmptyInput).into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn unknown_format_and_view_map_to_400() {
        let resp = WebError::UnknownFormat {
            format: "xlsx".into(),
        }
        .into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let resp = WebError::UnsupportedView {
            view: "timeline".into(),
            supported: "list",
        }
        .into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn error_response_is_json_with_a_message() {
        use http_body_util::BodyExt;

        let resp = WebError::NotLoaded.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        // content-type is application/json and the body carries the
        // human-readable error message.
        assert!(
            resp.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("\"error\""));
        assert!(text.contains("no Gramps file loaded"));
    }
}
