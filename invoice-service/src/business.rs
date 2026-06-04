use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::AppState;
use crate::error::AppError;
use tracing::info;

#[derive(Deserialize)]
pub struct CreateBusinessInput {
    pub name: String,
}

#[derive(Serialize)]
pub struct CreateBusinessResponse {
    pub id: Uuid,
    pub name: String,
    pub api_key: String,
}

// POST /businesses (Public - helper to generate keys)
pub async fn create_business(
    State(state): State<AppState>,
    Json(input): Json<CreateBusinessInput>,
) -> Result<Json<CreateBusinessResponse>, AppError> {
    info!("Creating business: {}", input.name);
    let api_key = format!(
        "dodo_sk_live_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    );

    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(api_key.as_bytes());
    let hash_hex = hex::encode(hasher.finalize());

    let row = sqlx::query!(
        "INSERT INTO businesses (name, api_key_hash) VALUES ($1, $2) RETURNING id",
        input.name,
        hash_hex
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(CreateBusinessResponse {
        id: row.id,
        name: input.name,
        api_key,
    }))
}
