//! All database logic. Every operation that touches more than one row runs in a single
//! transaction with row locks, so concurrent zone servers cannot duplicate or lose a character.

use serde::Serialize;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};

#[derive(Debug, Clone)]
pub struct Timing {
    /// An instance with no heartbeat for this long is treated as dead.
    pub dead_after_secs: f64,
    /// How long a crew has to arrive after a transfer before it is no longer counted
    /// against the destination's capacity.
    pub transfer_ttl_secs: f64,
}

#[derive(Debug, Clone, FromRow, Serialize, PartialEq)]
pub struct Instance {
    pub instance_id: Uuid,
    pub zone_key: String,
    pub address: String,
    pub server_name: String,
    pub max_players: i32,
    pub player_count: i32,
}

/// Where a zone should send a player.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Destination {
    pub instance_id: Uuid,
    pub address: String,
    pub server_name: String,
}

impl From<Instance> for Destination {
    fn from(i: Instance) -> Self {
        Destination { instance_id: i.instance_id, address: i.address, server_name: i.server_name }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ClaimOutcome {
    /// Pending transfer into this zone. Lease granted.
    Arrive { character_xml: String },
    /// The character already lives in this zone. Lease granted.
    Resume { character_xml: String },
    /// No saved character yet; start the normal new-character flow. Lease granted.
    New,
    /// The player belongs somewhere else: the zone their character is in, the live instance
    /// still holding their character, or an entry zone for brand-new players.
    Redirect(Destination),
}

#[derive(Debug, Clone)]
pub struct TransferPlayer {
    pub account_id: String,
    pub character_xml: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TransferResult {
    pub transfer_id: Uuid,
    pub destination: Destination,
    pub expires_in_secs: i64,
}

const LIVE: &str = "last_heartbeat > now() - make_interval(secs => $LIVE)";

/// SQL for live instances of a zone with at least `need` free slots, most free first.
/// Free slots subtract crews that have been sent there but have not arrived yet.
const PICK_INSTANCE: &str = "
    SELECT instance_id, zone_key, address, server_name, max_players, player_count FROM (
        SELECT i.*,
               i.max_players - i.player_count - COALESCE((
                   SELECT count(*) FROM transfer_players tp JOIN transfers t USING (transfer_id)
                   WHERE t.to_instance = i.instance_id AND tp.arrived_at IS NULL AND t.expires_at > now()
               ), 0) AS free
        FROM zone_instances i
        WHERE i.zone_key = $1 AND i.last_heartbeat > now() - make_interval(secs => $2)
    ) s
    WHERE free >= $3
    ORDER BY free DESC, instance_id
    LIMIT 1";

fn live(n: usize) -> String {
    LIVE.replace("$LIVE", &format!("${n}"))
}

pub struct Store {
    pub pool: PgPool,
    pub timing: Timing,
}

impl Store {
    pub fn new(pool: PgPool, timing: Timing) -> Self {
        Store { pool, timing }
    }

    async fn zone_exists(tx: &mut Transaction<'_, Postgres>, zone_key: &str) -> ApiResult<bool> {
        Ok(sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM zones WHERE zone_key = $1)")
            .bind(zone_key)
            .fetch_one(&mut **tx)
            .await?)
    }

    async fn live_instance(&self, tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<Instance> {
        let sql = format!(
            "SELECT instance_id, zone_key, address, server_name, max_players, player_count
             FROM zone_instances WHERE instance_id = $1 AND {}",
            live(2)
        );
        sqlx::query_as::<_, Instance>(&sql)
            .bind(id)
            .bind(self.timing.dead_after_secs)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(ApiError::UnknownInstance(id))
    }

    async fn pick_instance(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        zone_key: &str,
        need: i64,
    ) -> ApiResult<Option<Instance>> {
        Ok(sqlx::query_as::<_, Instance>(PICK_INSTANCE)
            .bind(zone_key)
            .bind(self.timing.dead_after_secs)
            .bind(need)
            .fetch_optional(&mut **tx)
            .await?)
    }

    pub async fn register(
        &self,
        zone_key: &str,
        address: &str,
        server_name: &str,
        max_players: i32,
    ) -> ApiResult<Uuid> {
        let mut tx = self.pool.begin().await?;
        if !Self::zone_exists(&mut tx, zone_key).await? {
            return Err(ApiError::UnknownZone(zone_key.to_owned()));
        }
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO zone_instances (instance_id, zone_key, address, server_name, max_players)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(zone_key)
        .bind(address)
        .bind(server_name)
        .bind(max_players)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn heartbeat(&self, id: Uuid, player_count: i32) -> ApiResult<()> {
        let updated = sqlx::query(
            "UPDATE zone_instances SET player_count = $2, last_heartbeat = now() WHERE instance_id = $1",
        )
        .bind(id)
        .bind(player_count)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(ApiError::UnknownInstance(id));
        }
        Ok(())
    }

    /// Decides what a newly connected, Steam-authenticated player should do in this instance.
    pub async fn claim(&self, account_id: &str, instance_id: Uuid) -> ApiResult<ClaimOutcome> {
        // A retry is only needed when two zones race to create the same brand-new character;
        // the second attempt then sees the row the first one created.
        for _ in 0..3 {
            if let Some(outcome) = self.claim_once(account_id, instance_id).await? {
                return Ok(outcome);
            }
        }
        Err(ApiError::BadRequest("could not claim character due to concurrent updates; retry".into()))
    }

    /// Returns `None` if a concurrent claim created the character first.
    async fn claim_once(&self, account_id: &str, instance_id: Uuid) -> ApiResult<Option<ClaimOutcome>> {
        let mut tx = self.pool.begin().await?;
        let inst = self.live_instance(&mut tx, instance_id).await?;

        let row: Option<(String, Option<String>, Option<Uuid>, bool)> = sqlx::query_as(&format!(
            "SELECT c.zone_key, c.character_xml, c.owner_instance,
                    COALESCE(o.{}, FALSE) AS owner_live
             FROM characters c LEFT JOIN zone_instances o ON o.instance_id = c.owner_instance
             WHERE c.account_id = $1
             FOR UPDATE OF c",
            live(2)
        ))
        .bind(account_id)
        .bind(self.timing.dead_after_secs)
        .fetch_optional(&mut *tx)
        .await?;

        let Some((zone_key, xml, owner, owner_live)) = row else {
            // Brand-new player: only entry zones may create characters.
            let is_entry: bool = sqlx::query_scalar("SELECT is_entry FROM zones WHERE zone_key = $1")
                .bind(&inst.zone_key)
                .fetch_one(&mut *tx)
                .await?;
            if is_entry {
                let created = sqlx::query(
                    "INSERT INTO characters (account_id, zone_key, owner_instance) VALUES ($1, $2, $3)
                     ON CONFLICT (account_id) DO NOTHING",
                )
                .bind(account_id)
                .bind(&inst.zone_key)
                .bind(instance_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                if created == 0 {
                    return Ok(None);
                }
                tx.commit().await?;
                return Ok(Some(ClaimOutcome::New));
            }
            let entry = self.pick_entry_instance(&mut tx).await?;
            return Ok(Some(ClaimOutcome::Redirect(entry.into())));
        };

        if let Some(owner) = owner {
            if owner != instance_id && owner_live {
                // Another live instance still holds the character, usually because the player
                // dropped and that zone hasn't noticed yet. Send them back to it.
                let holder = self.live_instance(&mut tx, owner).await?;
                return Ok(Some(ClaimOutcome::Redirect(holder.into())));
            }
        }

        if zone_key != inst.zone_key {
            let target = self
                .pick_instance(&mut tx, &zone_key, 1)
                .await?
                .ok_or_else(|| ApiError::NoCapacity(zone_key.clone()))?;
            return Ok(Some(ClaimOutcome::Redirect(target.into())));
        }

        // Character belongs in this zone: take the lease and close out any pending arrival.
        let arrived = sqlx::query(
            "UPDATE transfer_players tp SET arrived_at = now()
             FROM transfers t
             WHERE tp.transfer_id = t.transfer_id AND tp.account_id = $1
               AND tp.arrived_at IS NULL AND t.to_zone_key = $2",
        )
        .bind(account_id)
        .bind(&inst.zone_key)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;

        sqlx::query("UPDATE characters SET owner_instance = $2, updated_at = now() WHERE account_id = $1")
            .bind(account_id)
            .bind(instance_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        Ok(Some(match xml {
            None => ClaimOutcome::New,
            Some(character_xml) if arrived => ClaimOutcome::Arrive { character_xml },
            Some(character_xml) => ClaimOutcome::Resume { character_xml },
        }))
    }

    async fn pick_entry_instance(&self, tx: &mut Transaction<'_, Postgres>) -> ApiResult<Instance> {
        let entries: Vec<String> =
            sqlx::query_scalar("SELECT zone_key FROM zones WHERE is_entry ORDER BY zone_key")
                .fetch_all(&mut **tx)
                .await?;
        for z in &entries {
            if let Some(i) = self.pick_instance(tx, z, 1).await? {
                return Ok(i);
            }
        }
        Err(ApiError::NoCapacity(entries.join(", ")))
    }

    /// Saves a character. Only the lease holder may save.
    pub async fn save(&self, account_id: &str, instance_id: Uuid, xml: &str, release: bool) -> ApiResult<()> {
        let sql = if release {
            "UPDATE characters SET character_xml = $3, owner_instance = NULL, updated_at = now()
             WHERE account_id = $1 AND owner_instance = $2"
        } else {
            "UPDATE characters SET character_xml = $3, updated_at = now()
             WHERE account_id = $1 AND owner_instance = $2"
        };
        let updated = sqlx::query(sql)
            .bind(account_id)
            .bind(instance_id)
            .bind(xml)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if updated == 0 {
            return Err(ApiError::NotOwner(account_id.to_owned()));
        }
        Ok(())
    }

    /// Moves a crew from the calling instance to an adjacent zone, atomically.
    pub async fn transfer(
        &self,
        instance_id: Uuid,
        destination_zone_key: &str,
        players: &[TransferPlayer],
    ) -> ApiResult<TransferResult> {
        let mut tx = self.pool.begin().await?;
        let source = self.live_instance(&mut tx, instance_id).await?;

        if !Self::zone_exists(&mut tx, destination_zone_key).await? {
            return Err(ApiError::UnknownZone(destination_zone_key.to_owned()));
        }
        let adjacent: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM zone_links
                            WHERE zone_a = LEAST($1, $2) AND zone_b = GREATEST($1, $2))",
        )
        .bind(&source.zone_key)
        .bind(destination_zone_key)
        .fetch_one(&mut *tx)
        .await?;
        if !adjacent {
            return Err(ApiError::NotAdjacent {
                from: source.zone_key,
                to: destination_zone_key.to_owned(),
            });
        }

        let ids: Vec<String> = players.iter().map(|p| p.account_id.clone()).collect();
        let owned: Vec<(String, Option<Uuid>)> = sqlx::query_as(
            "SELECT account_id, owner_instance FROM characters
             WHERE account_id = ANY($1) ORDER BY account_id FOR UPDATE",
        )
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await?;
        for id in &ids {
            match owned.iter().find(|(a, _)| a == id) {
                Some((_, Some(owner))) if *owner == instance_id => {}
                _ => return Err(ApiError::NotOwner(id.clone())),
            }
        }

        let dest = self
            .pick_instance(&mut tx, destination_zone_key, players.len() as i64)
            .await?
            .ok_or_else(|| ApiError::NoCapacity(destination_zone_key.to_owned()))?;

        // A newer transfer supersedes any arrival these players never completed.
        sqlx::query("DELETE FROM transfer_players WHERE account_id = ANY($1) AND arrived_at IS NULL")
            .bind(&ids)
            .execute(&mut *tx)
            .await?;

        for p in players {
            sqlx::query(
                "UPDATE characters
                 SET character_xml = $2, zone_key = $3, owner_instance = NULL, updated_at = now()
                 WHERE account_id = $1",
            )
            .bind(&p.account_id)
            .bind(&p.character_xml)
            .bind(destination_zone_key)
            .execute(&mut *tx)
            .await?;
        }

        let transfer_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO transfers (transfer_id, from_instance, to_instance, to_zone_key, expires_at)
             VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))",
        )
        .bind(transfer_id)
        .bind(instance_id)
        .bind(dest.instance_id)
        .bind(destination_zone_key)
        .bind(self.timing.transfer_ttl_secs)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO transfer_players (transfer_id, account_id) SELECT $1, unnest($2::text[])",
        )
        .bind(transfer_id)
        .bind(&ids)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(TransferResult {
            transfer_id,
            destination: dest.into(),
            expires_in_secs: self.timing.transfer_ttl_secs as i64,
        })
    }

    pub async fn world(&self) -> ApiResult<serde_json::Value> {
        let zones: Vec<(String, String, bool)> =
            sqlx::query_as("SELECT zone_key, display_name, is_entry FROM zones ORDER BY zone_key")
                .fetch_all(&self.pool)
                .await?;
        let links: Vec<(String, String)> =
            sqlx::query_as("SELECT zone_a, zone_b FROM zone_links ORDER BY zone_a, zone_b")
                .fetch_all(&self.pool)
                .await?;
        let instances: Vec<Instance> = sqlx::query_as(&format!(
            "SELECT instance_id, zone_key, address, server_name, max_players, player_count
             FROM zone_instances WHERE {} ORDER BY zone_key, instance_id",
            live(1)
        ))
        .bind(self.timing.dead_after_secs)
        .fetch_all(&self.pool)
        .await?;

        Ok(serde_json::json!({
            "zones": zones.into_iter().map(|(key, name, entry)| serde_json::json!({
                "key": key, "name": name, "entry": entry
            })).collect::<Vec<_>>(),
            "links": links.into_iter().map(|(a, b)| [a, b]).collect::<Vec<_>>(),
            "instances": instances,
        }))
    }
}
