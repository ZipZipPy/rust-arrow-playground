//! The built frontend, served for every path outside `/api`.

use std::path::Path;

use axum::Router;
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderValue, Response, StatusCode, Uri};
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::warn;

const MISSING_FRONTEND: &str = "Rust Arrow Playground API server\n\n\
The frontend is not built, so only /api is available.\n\
Build it with `npm ci && npm run build` in web/ and point STATIC_DIR at web/dist,\n\
or run the UI with `npm run dev` (Vite proxies /api to this server).\n";

/// Vite's content-hashed build output: a file's name changes with its content.
const IMMUTABLE: HeaderValue = HeaderValue::from_static("public, max-age=31536000, immutable");
/// Everything else (`index.html` above all) is revalidated on every load, so
/// a redeploy is picked up at once instead of a stale page asking for hashed
/// chunks that no longer exist.
const NO_CACHE: HeaderValue = HeaderValue::from_static("no-cache");

/// Adds the static files to `router`: `/assets/*` from `dir/assets` (a
/// missing one is a plain 404, never the HTML page), other files from
/// `dir`, and `index.html` for unknown paths so client-side routes work.
/// Without the directory, `/` explains how to build the frontend.
pub fn with_static_files(router: Router, dir: &Path) -> Router {
    if !dir.is_dir() {
        warn!(dir = %dir.display(), "static directory not found; serving the API only");
        return router.fallback(missing_frontend);
    }
    let assets = ServiceBuilder::new()
        .layer(CompressionLayer::new())
        .layer(SetResponseHeaderLayer::overriding(
            CACHE_CONTROL,
            immutable_if_found,
        ))
        .service(ServeDir::new(dir.join("assets")));
    let pages = ServiceBuilder::new()
        .layer(CompressionLayer::new())
        .layer(SetResponseHeaderLayer::overriding(CACHE_CONTROL, NO_CACHE))
        .service(ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html"))));
    router
        .nest_service("/assets", assets)
        .fallback_service(pages)
}

/// A 404 must not be cached for a year.
fn immutable_if_found<B>(response: &Response<B>) -> Option<HeaderValue> {
    response.status().is_success().then_some(IMMUTABLE)
}

async fn missing_frontend(uri: Uri) -> (StatusCode, &'static str) {
    let status = if uri.path() == "/" {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    (status, MISSING_FRONTEND)
}
