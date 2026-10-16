//! API errors, rendered as `{ "error": "<code>", "message": "<text>" }`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("missing or invalid zone secret")]
    Unauthorized,
    #[error("zone '{0}' is not on the world map")]
    UnknownZone(String),
    #[error("zone instance {0} is not registered or has stopped sending heartbeats")]
    UnknownInstance(uuid::Uuid),
    #[error("this zone instance does not own character '{0}'")]
    NotOwner(String),
    #[error("zone '{to}' is not connected to zone '{from}'")]
    NotAdjacent { from: String, to: String },
    #[error("no live instance of zone '{0}' has room")]
    NoCapacity(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("database error")]
    Database(#[from] sqlx::Error),
}

impl ApiError {
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            ApiError::UnknownZone(_) => (StatusCode::NOT_FOUND, "unknown_zone"),
            ApiError::UnknownInstance(_) => (StatusCode::NOT_FOUND, "unknown_instance"),
            ApiError::NotOwner(_) => (StatusCode::CONFLICT, "not_owner"),
            ApiError::NotAdjacent { .. } => (StatusCode::CONFLICT, "not_adjacent"),
            ApiError::NoCapacity(_) => (StatusCode::SERVICE_UNAVAILABLE, "no_capacity"),
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            ApiError::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        if let ApiError::Database(e) = &self {
            tracing::error!(error = %e, "database error");
        }
        (status, Json(json!({ "error": code, "message": self.to_string() }))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
