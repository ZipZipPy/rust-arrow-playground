//! JSON error responses: `{"error": "..."}`.

use std::error::Error;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tracing::error;

use crate::encode::EncodeError;
use crate::query::ParamError;

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    NotFound,
    MethodNotAllowed,
    /// The client went away; nobody reads this response.
    Cancelled,
    /// Database or encoding failure; already logged when created.
    Internal(String),
}

impl ApiError {
    pub fn internal(err: &(dyn Error + 'static)) -> Self {
        let message = describe(err);
        error!(error = %message, "request failed");
        Self::Internal(message)
    }
}

/// The error and its sources, skipping sources whose text the outer
/// message already includes (e.g. `tokio_postgres::Error` says only
/// "db error" and keeps the server's message in its source).
fn describe(err: &(dyn Error + 'static)) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        if !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = cause.source();
    }
    message
}

impl From<ParamError> for ApiError {
    fn from(err: ParamError) -> Self {
        Self::BadRequest(err.to_string())
    }
}

macro_rules! internal_from {
    ($($ty:ty),+) => {
        $(impl From<$ty> for ApiError {
            fn from(err: $ty) -> Self {
                Self::internal(&err)
            }
        })+
    };
}

internal_from!(
    tokio_postgres::Error,
    deadpool_postgres::PoolError,
    tokio::task::JoinError
);

impl From<EncodeError> for ApiError {
    fn from(err: EncodeError) -> Self {
        match err {
            EncodeError::Cancelled(_) => Self::Cancelled,
            err => Self::internal(&err),
        }
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message.as_str()),
            Self::NotFound => (StatusCode::NOT_FOUND, "not found"),
            Self::MethodNotAllowed => (StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
            // nginx's "client closed request".
            Self::Cancelled => (
                StatusCode::from_u16(499).expect("a valid status code"),
                "cancelled",
            ),
            Self::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message.as_str()),
        };
        (status, Json(ErrorBody { error: message })).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("outer")]
    struct Outer(#[source] Inner);

    #[derive(Debug, thiserror::Error)]
    #[error("inner detail")]
    struct Inner;

    #[derive(Debug, thiserror::Error)]
    #[error("wrapped: {0}")]
    struct Wrapped(#[source] Inner);

    #[test]
    fn describes_the_whole_chain_once() {
        assert_eq!(describe(&Outer(Inner)), "outer: inner detail");
        assert_eq!(describe(&Wrapped(Inner)), "wrapped: inner detail");
    }
}
