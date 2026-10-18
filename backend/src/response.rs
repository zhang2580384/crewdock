use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct ApiEnvelope<T> {
    pub data: T,
}

pub fn ok<T: Serialize>(data: T) -> Json<ApiEnvelope<T>> {
    Json(ApiEnvelope { data })
}
