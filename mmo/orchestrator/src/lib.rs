//! Orchestrator for the Barotrauma MMO mod (a modified version of Barotrauma; see
//! docs/mmo/DESIGN.md). Owns the world map, the registry of running zone servers, character
//! ownership leases and crew transfers between zones.

pub mod api;
pub mod error;
pub mod store;
pub mod world;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
