use axum::{Json, http::StatusCode, response::{IntoResponse, Response}};
use mm_core::api::ApiError;

/// Messages are deliberately short and carry no request data.
#[derive(Debug)]
pub enum Error {
    BadRequest(&'static str),
    Unauthorized,
    /// The account's storage quota is used up.
    Quota,
    /// A request that did not come from the app's own page.
    Forbidden,
    Internal,
}

impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(kind = ?std::mem::discriminant(&e), "database error");
        Self::Internal
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (code, msg) = match self {
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::Quota => (StatusCode::PAYLOAD_TOO_LARGE, "storage quota"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        (code, Json(ApiError { error: msg.into() })).into_response()
    }
}

pub type Result<T> = std::result::Result<T, Error>;
