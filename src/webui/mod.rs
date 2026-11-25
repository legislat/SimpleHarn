//! Control page (operator mockup): Pico CSS + vanilla
//! JavaScript, assets embedded with `include_str!` — the page only renders `/har/*`
//! JSON and every action is a headless API call. The header is persistent (the
//! merged slot/tab strip and the memory bars never scroll away, compacted
//! 2026-11-01); the model table lives below it.

use crate::api::ApiState;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::sync::Arc;

const INDEX_HTML: &str = include_str!("index.html");
const STYLE_CSS: &str = include_str!("style.css");
const APP_JS: &str = include_str!("app.js");
const PICO_CSS: &str = include_str!("pico.min.css");
/// The operator's tri-color Browser_Tab icon (asset 2026-11-05), served as-is.
const FAVICON_PNG: &[u8] = include_bytes!("favicon.png");

async fn index() -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        INDEX_HTML,
    )
        .into_response()
}

async fn stylesheet() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        STYLE_CSS,
    )
        .into_response()
}

async fn pico() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        PICO_CSS,
    )
        .into_response()
}

async fn script() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        APP_JS,
    )
        .into_response()
}

async fn favicon() -> Response {
    ([(header::CONTENT_TYPE, "image/png")], FAVICON_PNG).into_response()
}

async fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

/// The page routes, merged into the control listener (Step 11: `/har/*` and the
/// page live on the designated control listener only).
pub fn router(_state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/", get(index))
        // The guard's PUBLIC_PATHS already names this, and the README advertises it, so
        // a bookmarked `.../index.html` should answer rather than 404 on a path that is
        // documented as public.
        .route("/index.html", get(index))
        .route("/style.css", get(stylesheet))
        .route("/app.js", get(script))
        .route("/pico.min.css", get(pico))
        .route("/favicon.png", get(favicon))
        // Browsers probe `/favicon.ico` whether or not the page links an icon; the
        // same PNG bytes under that name answer the probe instead of a 404 (the
        // sniffing decoders read a PNG served as `.ico`).
        .route("/favicon.ico", get(favicon))
        .fallback(not_found)
}

#[cfg(test)]
mod tests {
    use super::{APP_JS, FAVICON_PNG};
    use crate::engine::llama::OFFLOAD_ENGINES;

    /// List-drift falsifier (C of the `cpu_offload` scoping ruling): the served
    /// page's gate list — the exact `include_str!` bytes `/app.js` answers, so an
    /// `include_str!`-stale exe cannot hide a page bug (GOTCHAS [ui]) — must equal
    /// the Rust `OFFLOAD_ENGINES` const itself, not a second copy of it
    /// (GOTCHAS [test]). This is one of the two source pins the test wave kept:
    /// the DOM stub has no access to the Rust const, so the const comparison is
    /// not DOM-expressible. Nature, declared: a source-content assertion, not a
    /// render test. Every rendered behavior this function used to carry — the
    /// gated checkbox, the one-per-row count, the dash on non-offload rows — is a
    /// DOM leg in `tests/webui/webui_dom_test.mjs` (per-row checkbox counts and
    /// cell text, engine-scoped).
    ///
    /// Falsifier: adding an engine to the Rust const without restating the page
    /// list, or restating the page list wrongly, fails the equality.
    #[test]
    fn served_app_js_gates_the_cpu_control_on_the_offload_engine_table() {
        const LIST: &str = "const OFFLOAD_ENGINES = [";
        let at = APP_JS
            .find(LIST)
            .expect("the page must carry the OFFLOAD_ENGINES gate list");
        let tail = &APP_JS[at + LIST.len()..];
        let end = tail.find(']').expect("gate list unterminated");
        let listed: Vec<&str> = tail[..end]
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(|token| token.trim_matches(['"', '\'']))
            .collect();
        assert_eq!(
            listed, OFFLOAD_ENGINES,
            "the page's gate list must equal the Rust OFFLOAD_ENGINES const"
        );
    }

    /// A unit-handler state: the favicon handlers read no state at all, so the
    /// dummy seam `api`'s unit pins use is the honest here too.
    fn page_state() -> std::sync::Arc<crate::api::ApiState> {
        std::sync::Arc::new(crate::api::ApiState::new(
            crate::config::AppConfig::default(),
            crate::scheduler::SchedulerHandle::test_dummy(),
            crate::listeners::ListenerRegistry::new(),
            std::sync::Arc::new(crate::budget::LedgerOnlyWitness),
            crate::api::BootTimings::default(),
        ))
    }

    /// The favicon (operator asset 2026-11-05): both names answer 200 `image/png`
    /// with exactly the embedded PNG bytes — the `.ico` alias exists so the
    /// browser's default probe lands on the logo instead of the 404 fallback.
    ///
    /// Nature, declared: an in-process route test over `router()` itself, the page
    /// surface without the guard layer (`control_router` composes it in `api`);
    /// the guard's public-path ledger is doctrine-locked and untouched here.
    ///
    /// Falsifier: drop a route and that leg fails with the fallback 404; serve
    /// anything other than `FAVICON_PNG`, or a content-type other than
    /// `image/png`, and that leg's assert fails.
    #[tokio::test]
    async fn the_favicon_routes_answer_the_embedded_png() {
        use axum::body::Body;
        use axum::http::{header, Request, StatusCode};
        use tower::ServiceExt;
        for path in ["/favicon.png", "/favicon.ico"] {
            let response = super::router(page_state())
                .oneshot(
                    Request::get(path)
                        .body(Body::empty())
                        .expect("favicon request"),
                )
                .await
                .expect("in-process favicon read");
            assert_eq!(response.status(), StatusCode::OK, "{path} must answer");
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .expect("favicon content-type"),
                "image/png",
                "{path} must answer as a PNG"
            );
            let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .expect("favicon body");
            assert_eq!(
                &bytes[..],
                FAVICON_PNG,
                "{path} must answer the embedded PNG bytes"
            );
        }
    }

    /// Flash-deadline hygiene — the second kept source pin: a new flash must
    /// clear the previous deadline before arming its own, or an earlier timer
    /// wipes a later banner. Nature, declared: a source-content assertion over
    /// the exact `include_str!` bytes `/app.js` answers. The DOM stub has no
    /// timer driver, so a leaked prior deadline cannot be rendered there; the
    /// honest alternative is a fake-timer harness, which this wave did not
    /// build. Everything else this test used to carry is DOM-side in
    /// `tests/webui/webui_dom_test.mjs`: the banner's place in the document and
    /// the no-fixed rule are static legs there, the opaque/edged/hidden rules
    /// are cascade legs there, and the ledger dedup, the once-per-reason
    /// wording and the absent last-fault headline are rendered legs there.
    /// The name now states the one behavior this pin keeps: the served app
    /// clears the previous flash timer before arming a new deadline.
    ///
    /// Falsifier: delete the clear-before-set line in `flash` and the assert
    /// fails.
    #[test]
    fn served_app_clears_the_previous_flash_timer() {
        assert!(
            APP_JS.contains("if (flashTimer !== null) clearTimeout(flashTimer);"),
            "arming a new flash deadline must clear the previous one"
        );
    }
}
