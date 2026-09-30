//! web — the local HTTP server and server-rendered UI (plan §6.4 / §9).
//!
//! An axum 0.8 server bound to **127.0.0.1** (decision D1): the
//! upload/load/options/events/export/reset API plus the Askama + HTMX
//! landing page. Milestone 13 ships the API + skeleton UI — every route
//! of plan §9's table, the 200 MB upload cap with generated temp names
//! and lifecycle cleanup, the vendored htmx file, and the
//! escaping-regression test (plan §11). The full four-view UI (tabs,
//! checkboxes, export button group) lands in milestone 14.
//!
//! ## Routes (plan §9 route table)
//!
//! | Route | Purpose |
//! | --- | --- |
//! | `GET /` | Landing page: upload form, or the loaded file's summary + HTMX view/export controls |
//! | `POST /api/load` | multipart upload (200 MB cap) → generated temp name → parse → store; returns `{event_types, people_count, …}` (HTML fragment for HTMX) |
//! | `GET /api/options` | the current `ReportOptions` as deterministic JSON |
//! | `GET /api/events?view=list` | the rendered view as an HTML fragment (HTMX swap) |
//! | `GET /api/events.json` | the same data as `EventRow` JSON |
//! | `GET /api/export?format=…` | streams one csv/json/parquet/pdf download |
//! | `POST /api/reset` | unload the current file; deletes its upload |
//! | `GET /static/htmx.min.js` | the vendored HTMX 2.x single file |
//!
//! The server is a local single-user tool handling family PII (plan §9):
//! nothing binds an external interface without an explicit future `--host`
//! flag.

pub mod error;
pub mod handlers;
pub mod state;
pub mod templates;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{MethodFilter, get, post};

use crate::state::AppState;

/// The plan's hard upload cap: 200 MB (plan §9), enforced while the
/// upload streams to disk — before any parse reads the bytes into memory.
pub const UPLOAD_CAP_BYTES: usize = 200 * 1024 * 1024;

/// The plan's default serve port (plan §6.5 sketch: `serve [--port 8380]`).
pub const DEFAULT_PORT: u16 = 8380;

/// Canonical crate name — smoke tests (and later workspace integration
/// tests) use this to assert crate linkage.
pub const CRATE_NAME: &str = "web";

/// The address the server binds: loopback only (plan §9).
pub const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;

/// Server configuration for [`serve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServeConfig {
    /// The bind address. The plan fixes loopback unless an explicit
    /// `--host` flag is ever added (plan §9); [`ServeConfig::local`] is
    /// the only constructor ship.
    pub host: IpAddr,
    /// The TCP port (default [`DEFAULT_PORT`]).
    pub port: u16,
}

impl ServeConfig {
    /// `127.0.0.1:PORT` — the default local configuration.
    pub fn local(port: u16) -> ServeConfig {
        ServeConfig {
            host: IpAddr::V4(LOOPBACK),
            port,
        }
    }
}

/// Build the router over `state` — used by the integration tests
/// (Tower `oneshot`) and by [`serve`].
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(handlers::root))
        .route(
            "/api/load",
            post(handlers::load).layer(DefaultBodyLimit::max(UPLOAD_CAP_BYTES)),
        )
        .route(
            "/api/options",
            get(handlers::options).on(MethodFilter::PUT, handlers::save_options),
        )
        .route("/api/events", get(handlers::events))
        .route("/api/events.json", get(handlers::events_json))
        .route("/api/export", get(handlers::export))
        .route("/api/reset", post(handlers::reset))
        .route("/static/htmx.min.js", get(handlers::htmx))
        .route("/static/style.css", get(handlers::style))
        .with_state(state)
}

/// Bind the loopback listener and serve until Ctrl-C — the `serve`
/// subcommand's engine (plan §6.5 / §9). Uploads and the temp dir are
/// cleaned up when the state drops at exit.
pub async fn serve(config: ServeConfig) -> Result<(), ServeError> {
    let listener = tokio::net::TcpListener::bind((config.host, config.port))
        .await
        .map_err(|source| ServeError::Bind {
            host: config.host,
            port: config.port,
            source,
        })?;
    let addr = listener.local_addr()?;
    println!("gramps-events web UI on http://{addr}/");
    let state = Arc::new(AppState::new().map_err(ServeError::Temp)?);
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Startup / shutdown failures of the [`serve`] entry point.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The loopback bind failed (port in use, ...).
    #[error("cannot bind {host}:{port}: {source}")]
    Bind {
        /// The bind address.
        host: IpAddr,
        /// The bind port.
        port: u16,
        /// The underlying IO error.
        source: std::io::Error,
    },
    /// The upload temp dir could not be created.
    #[error("cannot create the upload temp dir: {0}")]
    Temp(std::io::Error),
    /// The server itself failed after binding.
    #[error("server failed: {0}")]
    Serve(#[from] std::io::Error),
}

/// Wait for Ctrl-C — the graceful-shutdown trigger (plan §9: cleanup
/// happens when the state drops). Ignores a missing handler; the process
/// just keeps serving.
async fn shutdown_signal() {
    if let Err(err) = tokio::signal::ctrl_c().await {
        eprintln!("failed to install the Ctrl-C handler: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_cap_is_plan_200_mb() {
        assert_eq!(UPLOAD_CAP_BYTES, 200 * 1024 * 1024);
    }

    #[test]
    fn local_config_binds_loopback_and_default_port() {
        let config = ServeConfig::local(DEFAULT_PORT);
        assert_eq!(config.host, IpAddr::V4(LOOPBACK));
        assert_eq!(config.port, 8380);
        assert!(
            config.host.is_loopback(),
            "never bind an external interface"
        );
    }

    #[test]
    fn smoke_crate_builds() {
        assert_eq!(crate::CRATE_NAME, "web");
    }
}
