//! HTTP routes. See docs/mmo/PROTOCOL.md for the contract.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::store::{ClaimOutcome, Store, TransferPlayer, TransferResult};

pub struct AppState {
    pub store: Store,
    pub zone_secret: String,
    pub heartbeat_interval_secs: u64,
    /// Load testing only: accept "UNAUTHENTICATED_<name>" account IDs, which anyone can spoof.
    pub allow_unauthenticated: bool,
}

pub type Shared = Arc<AppState>;

pub fn router(state: Shared) -> Router {
    let v1 = Router::new()
        .route("/zones/register", post(register))
        .route("/zones/{instance_id}/heartbeat", post(heartbeat))
        .route("/players/{account_id}/claim", post(claim))
        .route("/players/{account_id}/character", put(save_character))
        .route("/players/{account_id}/release", post(release))
        .route("/transfers", post(transfer))
        .route("/world", get(world))
        .layer(middleware::from_fn_with_state(state.clone(), require_zone_secret));

    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .nest("/v1", v1)
        .with_state(state)
}

/// Constant-time comparison so the secret can't be guessed byte by byte from timings.
fn secrets_match(given: &[u8], expected: &[u8]) -> bool {
    if given.len() != expected.len() {
        return false;
    }
    given.iter().zip(expected).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

async fn require_zone_secret(State(state): State<Shared>, req: Request, next: Next) -> Result<Response, ApiError> {
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|token| secrets_match(token.as_bytes(), state.zone_secret.as_bytes()));
    if !ok {
        return Err(ApiError::Unauthorized);
    }
    Ok(next.run(req).await)
}

fn check_account_id(state: &AppState, id: &str) -> ApiResult<()> {
    if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
        return Err(ApiError::BadRequest("invalid account_id".into()));
    }
    if id.starts_with("UNAUTHENTICATED_") && !state.allow_unauthenticated {
        return Err(ApiError::BadRequest(
            "unauthenticated accounts are not allowed (set ALLOW_UNAUTHENTICATED only for load tests)".into(),
        ));
    }
    Ok(())
}

fn check_xml(xml: &str) -> ApiResult<()> {
    if !xml.trim_start().starts_with('<') {
        return Err(ApiError::BadRequest("character_xml must be an XML element".into()));
    }
    Ok(())
}

#[derive(Deserialize)]
struct RegisterBody {
    zone_key: String,
    address: String,
    server_name: String,
    max_players: i32,
}

async fn register(State(s): State<Shared>, Json(b): Json<RegisterBody>) -> ApiResult<Json<Value>> {
    if b.address.trim().is_empty() {
        return Err(ApiError::BadRequest("address is required".into()));
    }
    if !(1..=255).contains(&b.max_players) {
        return Err(ApiError::BadRequest("max_players must be 1-255".into()));
    }
    let id = s.store.register(&b.zone_key, &b.address, &b.server_name, b.max_players).await?;
    tracing::info!(zone = %b.zone_key, instance = %id, address = %b.address, "zone registered");
    Ok(Json(json!({ "instance_id": id, "heartbeat_interval_secs": s.heartbeat_interval_secs })))
}

#[derive(Deserialize)]
struct HeartbeatBody {
    player_count: i32,
}

async fn heartbeat(
    State(s): State<Shared>,
    Path(instance_id): Path<Uuid>,
    Json(b): Json<HeartbeatBody>,
) -> ApiResult<StatusCode> {
    if b.player_count < 0 {
        return Err(ApiError::BadRequest("player_count must be >= 0".into()));
    }
    s.store.heartbeat(instance_id, b.player_count).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct InstanceBody {
    instance_id: Uuid,
}

async fn claim(
    State(s): State<Shared>,
    Path(account_id): Path<String>,
    Json(b): Json<InstanceBody>,
) -> ApiResult<Json<ClaimOutcome>> {
    check_account_id(&s, &account_id)?;
    let outcome = s.store.claim(&account_id, b.instance_id).await?;
    let status = match &outcome {
        ClaimOutcome::Arrive { .. } => "arrive",
        ClaimOutcome::Resume { .. } => "resume",
        ClaimOutcome::New => "new",
        ClaimOutcome::Redirect(_) => "redirect",
    };
    tracing::info!(account = %account_id, instance = %b.instance_id, status, "claim");
    Ok(Json(outcome))
}

#[derive(Deserialize)]
struct SaveBody {
    instance_id: Uuid,
    character_xml: String,
}

async fn save_character(
    State(s): State<Shared>,
    Path(account_id): Path<String>,
    Json(b): Json<SaveBody>,
) -> ApiResult<StatusCode> {
    check_account_id(&s, &account_id)?;
    check_xml(&b.character_xml)?;
    s.store.save(&account_id, b.instance_id, Some(&b.character_xml), false).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ReleaseBody {
    instance_id: Uuid,
    /// Omitted or null: keep the last saved character.
    #[serde(default)]
    character_xml: Option<String>,
}

async fn release(
    State(s): State<Shared>,
    Path(account_id): Path<String>,
    Json(b): Json<ReleaseBody>,
) -> ApiResult<StatusCode> {
    check_account_id(&s, &account_id)?;
    if let Some(xml) = &b.character_xml {
        check_xml(xml)?;
    }
    s.store.save(&account_id, b.instance_id, b.character_xml.as_deref(), true).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct TransferPlayerBody {
    account_id: String,
    character_xml: String,
}

#[derive(Deserialize)]
struct TransferBody {
    instance_id: Uuid,
    destination_zone_key: String,
    players: Vec<TransferPlayerBody>,
}

async fn transfer(State(s): State<Shared>, Json(b): Json<TransferBody>) -> ApiResult<Json<TransferResult>> {
    if b.players.is_empty() {
        return Err(ApiError::BadRequest("players must not be empty".into()));
    }
    let mut seen = HashSet::new();
    for p in &b.players {
        check_account_id(&s, &p.account_id)?;
        check_xml(&p.character_xml)?;
        if !seen.insert(p.account_id.as_str()) {
            return Err(ApiError::BadRequest(format!("duplicate account_id {}", p.account_id)));
        }
    }
    let players: Vec<TransferPlayer> = b
        .players
        .into_iter()
        .map(|p| TransferPlayer { account_id: p.account_id, character_xml: p.character_xml })
        .collect();
    let result = s.store.transfer(b.instance_id, &b.destination_zone_key, &players).await?;
    tracing::info!(
        transfer = %result.transfer_id,
        from = %b.instance_id,
        to = %result.destination.instance_id,
        crew = players.len(),
        "transfer"
    );
    Ok(Json(result))
}

async fn world(State(s): State<Shared>) -> ApiResult<Json<Value>> {
    Ok(Json(s.store.world().await?))
}

#[cfg(test)]
mod tests {
    use super::secrets_match;

    #[test]
    fn secret_comparison() {
        assert!(secrets_match(b"abc", b"abc"));
        assert!(!secrets_match(b"abd", b"abc"));
        assert!(!secrets_match(b"ab", b"abc"));
    }
}
