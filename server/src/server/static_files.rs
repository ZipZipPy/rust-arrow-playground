//! The built frontend, served for every path outside `/api`.

use std::path::Path;

use axum::Router;
use axum::http::{StatusCode, Uri};
use tower_http::services::{ServeDir, ServeFile};
use tracing::warn;

const MISSING_FRONTEND: &str = "Rust Arrow Playground API server\n\n\
The frontend is not built, so only /api is available.\n\
Build it with `npm ci && npm run build` in web/ and point STATIC_DIR at web/dist,\n\
or run the UI with `npm run dev` (Vite proxies /api to this server).\n";

/// Adds the static file fallback to `router`: files from `dir`, and
/// `index.html` for unknown paths so client-side routes work. Without the
/// directory, `/` explains how to build the frontend.
pub fn with_static_files(router: Router, dir: &Path) -> Router {
    if dir.is_dir() {
        let index = ServeFile::new(dir.join("index.html"));
        router.fallback_service(ServeDir::new(dir).fallback(index))
    } else {
        warn!(dir = %dir.display(), "static directory not found; serving the API only");
        router.fallback(missing_frontend)
    }
}

async fn missing_frontend(uri: Uri) -> (StatusCode, &'static str) {
    let status = if uri.path() == "/" {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    (status, MISSING_FRONTEND)
}
