use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::errors::{Problem, Refusal};

impl Problem {
    pub fn status(&self) -> StatusCode {
        match self {
            Problem::Refused(Refusal::Db(_)) | Problem::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            Problem::Refused(_) | Problem::Launch(_) | Problem::Repo(_) => StatusCode::BAD_REQUEST,
            Problem::NoLogin => StatusCode::NOT_FOUND,
        }
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        (self.status(), Json(self)).into_response()
    }
}
