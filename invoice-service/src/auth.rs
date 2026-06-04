use axum::{
    extract::{FromRef, FromRequestParts},
    http::request::Parts,
};
use sqlx::PgPool;
use uuid::Uuid;
use crate::error::AppError;

#[derive(Debug, Clone)]
pub struct AuthenticatedBusiness {
    pub id: Uuid,
    pub name: String,
}

// Authenticates requests by hashing the Bearer token and looking up the business in the database.
#[axum::async_trait]
impl<S> FromRequestParts<S> for AuthenticatedBusiness
where
    PgPool: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get("Authorization")
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| AppError::Unauthorized("Missing Authorization header".to_string()))?;

        if !auth_header.starts_with("Bearer ") {
            return Err(AppError::Unauthorized("Invalid Authorization header format".to_string()));
        }

        let token = &auth_header[7..];

        // Hash token with SHA-256
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let hash_bytes = hasher.finalize();
        let hash_hex = hex::encode(hash_bytes);

        let db = PgPool::from_ref(state);
        let business = sqlx::query!(
            "SELECT id, name FROM businesses WHERE api_key_hash = $1",
            hash_hex
        )
        .fetch_optional(&db)
        .await
        .map_err(|e| AppError::Internal(format!("Database error: {}", e)))?
        .ok_or_else(|| AppError::Unauthorized("Invalid API Key".to_string()))?;

        Ok(AuthenticatedBusiness {
            id: business.id,
            name: business.name,
        })
    }
}
