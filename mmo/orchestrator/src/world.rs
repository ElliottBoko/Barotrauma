//! The world map: which zones exist, which are entry points, and how they connect.
//!
//! Loaded from a JSON file at startup and synced into the database, so the file is the
//! source of truth and the tables can be queried transactionally.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ZoneDef {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub entry: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldFile {
    pub zones: Vec<ZoneDef>,
    /// Pairs of zone keys that are directly connected.
    pub links: Vec<[String; 2]>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum WorldError {
    #[error("world has no zones")]
    Empty,
    #[error("world has no entry zone; mark at least one zone with \"entry\": true")]
    NoEntry,
    #[error("zone key '{0}' is defined more than once")]
    DuplicateZone(String),
    #[error("zone key '{0}' is invalid; use lowercase letters, digits and '-'")]
    BadKey(String),
    #[error("link refers to unknown zone '{0}'")]
    UnknownZoneInLink(String),
    #[error("zone '{0}' is linked to itself")]
    SelfLink(String),
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl WorldFile {
    pub fn parse(json: &str) -> Result<Self, String> {
        let world: WorldFile = serde_json::from_str(json).map_err(|e| e.to_string())?;
        world.validate().map_err(|e| e.to_string())?;
        Ok(world)
    }

    pub fn validate(&self) -> Result<(), WorldError> {
        if self.zones.is_empty() {
            return Err(WorldError::Empty);
        }
        let mut keys = HashSet::new();
        for z in &self.zones {
            if !valid_key(&z.key) {
                return Err(WorldError::BadKey(z.key.clone()));
            }
            if !keys.insert(z.key.as_str()) {
                return Err(WorldError::DuplicateZone(z.key.clone()));
            }
        }
        if !self.zones.iter().any(|z| z.entry) {
            return Err(WorldError::NoEntry);
        }
        for [a, b] in &self.links {
            for k in [a, b] {
                if !keys.contains(k.as_str()) {
                    return Err(WorldError::UnknownZoneInLink(k.clone()));
                }
            }
            if a == b {
                return Err(WorldError::SelfLink(a.clone()));
            }
        }
        Ok(())
    }

    /// Upserts zones and replaces all links. Zones removed from the file are kept in the
    /// database (characters may still reference them) but lose their links.
    pub async fn sync_to_db(&self, pool: &PgPool) -> Result<(), sqlx::Error> {
        let mut tx = pool.begin().await?;
        for z in &self.zones {
            sqlx::query(
                "INSERT INTO zones (zone_key, display_name, is_entry) VALUES ($1, $2, $3)
                 ON CONFLICT (zone_key) DO UPDATE
                 SET display_name = EXCLUDED.display_name, is_entry = EXCLUDED.is_entry",
            )
            .bind(&z.key)
            .bind(&z.name)
            .bind(z.entry)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("DELETE FROM zone_links").execute(&mut *tx).await?;
        for [a, b] in &self.links {
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            sqlx::query("INSERT INTO zone_links (zone_a, zone_b) VALUES ($1, $2) ON CONFLICT DO NOTHING")
                .bind(lo)
                .bind(hi)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(json: &str) -> Result<WorldFile, String> {
        WorldFile::parse(json)
    }

    #[test]
    fn accepts_valid_world() {
        let w = world(r#"{"zones":[{"key":"loc-1","name":"A","entry":true},{"key":"loc-2","name":"B"}],"links":[["loc-1","loc-2"]]}"#).unwrap();
        assert_eq!(w.zones.len(), 2);
        assert!(!w.zones[1].entry);
    }

    #[test]
    fn rejects_missing_entry() {
        let e = world(r#"{"zones":[{"key":"loc-1","name":"A"}],"links":[]}"#).unwrap_err();
        assert!(e.contains("no entry zone"), "{e}");
    }

    #[test]
    fn rejects_unknown_link_and_self_link() {
        let base = r#"{"zones":[{"key":"loc-1","name":"A","entry":true}],"links":"#;
        assert!(world(&format!(r#"{base}[["loc-1","loc-9"]]}}"#)).unwrap_err().contains("loc-9"));
        assert!(world(&format!(r#"{base}[["loc-1","loc-1"]]}}"#)).unwrap_err().contains("itself"));
    }

    #[test]
    fn rejects_duplicate_and_bad_keys() {
        assert!(world(r#"{"zones":[{"key":"a","name":"A","entry":true},{"key":"a","name":"B"}],"links":[]}"#)
            .unwrap_err()
            .contains("more than once"));
        assert!(world(r#"{"zones":[{"key":"Loc 1","name":"A","entry":true}],"links":[]}"#)
            .unwrap_err()
            .contains("invalid"));
    }
}
