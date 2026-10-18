use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("{0} is busy; retry")]
    Busy(&'static str),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("too many requests; retry later")]
    RateLimited,
    #[error("too many requests; retry later")]
    RateLimitedFor(u64),
    #[error("{0} is unavailable")]
    Unavailable(&'static str),
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
    #[error("database error: {0}")]
    Sqlx(sqlx::Error),
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let retry_after = match &self {
            ApiError::RateLimitedFor(seconds) => Some(*seconds),
            _ => None,
        };
        let internal = matches!(
            &self,
            ApiError::Internal(_) | ApiError::Sqlx(_) | ApiError::Redis(_)
        );
        let error_message = if internal {
            tracing::error!(error = %self, "request failed with an internal error");
            "internal error".to_string()
        } else {
            self.to_string()
        };
        let code = match &self {
            ApiError::Busy(_) => "busy",
            ApiError::Conflict(_) => "conflict",
            ApiError::RateLimited | ApiError::RateLimitedFor(_) => "rate_limited",
            ApiError::Unavailable(_) => "unavailable",
            ApiError::BadRequest(_) => "bad_request",
            ApiError::Unauthorized => "unauthorized",
            ApiError::Forbidden => "forbidden",
            ApiError::NotFound => "not_found",
            ApiError::Internal(_) | ApiError::Sqlx(_) | ApiError::Redis(_) => "internal_error",
        };
        let retryable = matches!(
            self,
            ApiError::Busy(_)
                | ApiError::RateLimited
                | ApiError::RateLimitedFor(_)
                | ApiError::Unavailable(_)
        );
        let status = match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden => StatusCode::FORBIDDEN,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::Busy(_) => StatusCode::CONFLICT,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::RateLimited | ApiError::RateLimitedFor(_) => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Internal(_) | ApiError::Sqlx(_) | ApiError::Redis(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        let body = Json(json!({
            "error": error_message,
            "code": code,
            "retryable": retryable,
        }));
        let mut response = (status, body).into_response();
        if let Some(seconds) = retry_after {
            response.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                seconds
                    .to_string()
                    .parse()
                    .expect("numeric Retry-After header"),
            );
        }
        response
    }
}

fn is_lock_not_available_code(code: Option<&str>) -> bool {
    code == Some("55P03")
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        if is_lock_not_available_code(
            error
                .as_database_error()
                .and_then(|database_error| database_error.code())
                .as_deref(),
        ) {
            Self::Busy("database resource")
        } else {
            Self::Sqlx(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiError, is_lock_not_available_code};
    use axum::{body::to_bytes, response::IntoResponse};
    use serde_json::Value;

    #[tokio::test]
    async fn login_rate_limit_returns_retry_after_without_changing_error_contract() {
        let response = ApiError::RateLimitedFor(89).into_response();
        assert_eq!(response.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "89");
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "rate_limited");
        assert_eq!(body["retryable"], true);
    }

    #[tokio::test]
    async fn busy_error_has_stable_conflict_contract() {
        let response = ApiError::Busy("user").into_response();
        assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "busy");
        assert_eq!(body["retryable"], true);
        assert_eq!(body["error"], "user is busy; retry");
    }

    #[tokio::test]
    async fn internal_error_details_are_not_exposed() {
        let response =
            ApiError::Internal(anyhow::anyhow!("database password leaked")).into_response();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "internal_error");
        assert_eq!(body["error"], "internal error");
        assert!(!body.to_string().contains("database password leaked"));
    }

    #[test]
    fn postgres_lock_not_available_is_retryable_busy() {
        assert!(is_lock_not_available_code(Some("55P03")));
        assert!(!is_lock_not_available_code(Some("23505")));
        assert!(!is_lock_not_available_code(None));
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
