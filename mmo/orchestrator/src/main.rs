use std::sync::Arc;

use baro_orchestrator::api::{router, AppState};
use baro_orchestrator::store::{Store, Timing};
use baro_orchestrator::world::WorldFile;
use baro_orchestrator::MIGRATOR;
use tracing_subscriber::EnvFilter;

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("environment variable {name} is required"))
}

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> Result<T, String> {
    match std::env::var(name) {
        Ok(v) => v.parse().map_err(|_| format!("{name} has an invalid value: {v}")),
        Err(_) => Ok(default),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let database_url = env("DATABASE_URL")?;
    let zone_secret = env("ZONE_SECRET")?;
    if zone_secret.len() < 16 {
        return Err("ZONE_SECRET must be at least 16 characters".into());
    }
    let world_path = env_or("WORLD_FILE", "world.json".to_string())?;
    let bind = env_or("BIND_ADDR", "0.0.0.0:8080".to_string())?;
    let heartbeat_interval_secs: u64 = env_or("HEARTBEAT_INTERVAL_SECS", 10)?;
    let transfer_ttl_secs: f64 = env_or("TRANSFER_TTL_SECS", 300.0)?;
    let allow_unauthenticated = env_or("ALLOW_UNAUTHENTICATED", String::new())? == "I_UNDERSTAND_THIS_IS_INSECURE";
    if allow_unauthenticated {
        tracing::warn!("ALLOW_UNAUTHENTICATED is on: players can impersonate each other. Load testing only.");
    }

    let world = WorldFile::parse(&std::fs::read_to_string(&world_path)?)
        .map_err(|e| format!("{world_path}: {e}"))?;

    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(20).connect(&database_url).await?;
    MIGRATOR.run(&pool).await?;
    world.sync_to_db(&pool).await?;
    tracing::info!(zones = world.zones.len(), links = world.links.len(), "world loaded");

    let timing = Timing {
        // Three missed heartbeats and the instance is considered dead.
        dead_after_secs: (heartbeat_interval_secs * 3) as f64,
        transfer_ttl_secs,
    };
    let state = Arc::new(AppState {
        store: Store::new(pool, timing),
        zone_secret,
        heartbeat_interval_secs,
        allow_unauthenticated,
    });

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "orchestrator listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
