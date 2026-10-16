//! End-to-end tests of the HTTP API against a real Postgres database.
//! Each test gets a fresh database (sqlx::test). Requires DATABASE_URL pointing at a
//! Postgres server where the user may create databases.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use baro_orchestrator::api::{router, AppState};
use baro_orchestrator::store::{Store, Timing};
use baro_orchestrator::world::WorldFile;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;

const SECRET: &str = "test-secret-0123456789";

/// loc-1 (entry) — conn-1-2 — loc-2, and loc-3 off on its own.
const WORLD: &str = r#"{
  "zones": [
    {"key": "loc-1", "name": "Outpost One", "entry": true},
    {"key": "conn-1-2", "name": "Route 1-2"},
    {"key": "loc-2", "name": "Outpost Two"},
    {"key": "loc-3", "name": "Far Away"}
  ],
  "links": [["loc-1", "conn-1-2"], ["conn-1-2", "loc-2"]]
}"#;

struct Harness {
    app: axum::Router,
    pool: PgPool,
}

impl Harness {
    async fn new(pool: PgPool) -> Self {
        WorldFile::parse(WORLD).unwrap().sync_to_db(&pool).await.unwrap();
        let state = Arc::new(AppState {
            store: Store::new(pool.clone(), Timing { dead_after_secs: 30.0, transfer_ttl_secs: 300.0 }),
            zone_secret: SECRET.into(),
            heartbeat_interval_secs: 10,
        });
        Harness { app: router(state), pool }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {SECRET}"));
        let body = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = self.app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
        (status, json)
    }

    async fn register(&self, zone: &str, max_players: i32) -> String {
        let (s, v) = self
            .call(
                "POST",
                "/v1/zones/register",
                Some(json!({
                    "zone_key": zone,
                    "address": format!("10.0.0.1:{}", 27000 + max_players),
                    "server_name": format!("{zone} server"),
                    "max_players": max_players
                })),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v["instance_id"].as_str().unwrap().to_owned()
    }

    async fn claim(&self, account: &str, instance: &str) -> (StatusCode, Value) {
        self.call("POST", &format!("/v1/players/{account}/claim"), Some(json!({ "instance_id": instance })))
            .await
    }

    async fn save(&self, account: &str, instance: &str, xml: &str) -> StatusCode {
        self.call(
            "PUT",
            &format!("/v1/players/{account}/character"),
            Some(json!({ "instance_id": instance, "character_xml": xml })),
        )
        .await
        .0
    }

    async fn transfer(&self, from: &str, dest: &str, crew: &[(&str, &str)]) -> (StatusCode, Value) {
        let players: Vec<Value> =
            crew.iter().map(|(a, x)| json!({ "account_id": a, "character_xml": x })).collect();
        self.call(
            "POST",
            "/v1/transfers",
            Some(json!({ "instance_id": from, "destination_zone_key": dest, "players": players })),
        )
        .await
    }

    /// Makes an instance look dead by backdating its last heartbeat.
    async fn kill(&self, instance: &str) {
        sqlx::query("UPDATE zone_instances SET last_heartbeat = now() - interval '1 hour' WHERE instance_id = $1::uuid")
            .bind(instance)
            .execute(&self.pool)
            .await
            .unwrap();
    }

    /// Creates a player at an entry instance and gives them a saved character.
    async fn new_player(&self, account: &str, entry_instance: &str, xml: &str) {
        let (s, v) = self.claim(account, entry_instance).await;
        assert_eq!((s, v["status"].as_str()), (StatusCode::OK, Some("new")), "{v}");
        assert_eq!(self.save(account, entry_instance, xml).await, StatusCode::NO_CONTENT);
    }
}

const P1: &str = "STEAM_1:1:1001";
const P2: &str = "STEAM_1:0:2002";
const XML1: &str = r#"<CharacterCampaignData name="Ada"><inventory/></CharacterCampaignData>"#;
const XML2: &str = r#"<CharacterCampaignData name="Bo"><inventory/></CharacterCampaignData>"#;

#[sqlx::test(migrator = "baro_orchestrator::MIGRATOR")]
async fn rejects_missing_or_wrong_secret(pool: PgPool) {
    let h = Harness::new(pool).await;
    let no_auth = Request::builder().uri("/v1/world").body(Body::empty()).unwrap();
    assert_eq!(h.app.clone().oneshot(no_auth).await.unwrap().status(), StatusCode::UNAUTHORIZED);
    let wrong = Request::builder()
        .uri("/v1/world")
        .header("authorization", "Bearer nope")
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.app.clone().oneshot(wrong).await.unwrap().status(), StatusCode::UNAUTHORIZED);
    let health = Request::builder().uri("/healthz").body(Body::empty()).unwrap();
    assert_eq!(h.app.clone().oneshot(health).await.unwrap().status(), StatusCode::OK);
}

#[sqlx::test(migrator = "baro_orchestrator::MIGRATOR")]
async fn register_validates_zone_and_input(pool: PgPool) {
    let h = Harness::new(pool).await;
    let (s, v) = h
        .call(
            "POST",
            "/v1/zones/register",
            Some(json!({"zone_key": "loc-99", "address": "x:1", "server_name": "x", "max_players": 32})),
        )
        .await;
    assert_eq!((s, v["error"].as_str()), (StatusCode::NOT_FOUND, Some("unknown_zone")));
    let (s, _) = h
        .call(
            "POST",
            "/v1/zones/register",
            Some(json!({"zone_key": "loc-1", "address": "x:1", "server_name": "x", "max_players": 0})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    h.register("loc-1", 32).await;
}

#[sqlx::test(migrator = "baro_orchestrator::MIGRATOR")]
async fn new_players_start_only_in_entry_zones(pool: PgPool) {
    let h = Harness::new(pool).await;
    let entry = h.register("loc-1", 32).await;
    let far = h.register("loc-2", 32).await;

    // Joining a non-entry zone with no character sends you to an entry zone.
    let (s, v) = h.claim(P1, &far).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["status"], "redirect");
    assert_eq!(v["instance_id"], entry.as_str());

    let (_, v) = h.claim(P1, &entry).await;
    assert_eq!(v["status"], "new");
    // Claiming again before the first save is idempotent.
    let (_, v) = h.claim(P1, &entry).await;
    assert_eq!(v["status"], "new");
}

#[sqlx::test(migrator = "baro_orchestrator::MIGRATOR")]
async fn crew_transfers_between_zones_with_their_character(pool: PgPool) {
    let h = Harness::new(pool).await;
    let a = h.register("loc-1", 32).await;
    let b = h.register("conn-1-2", 32).await;
    h.new_player(P1, &a, XML1).await;
    h.new_player(P2, &a, XML2).await;

    let xml1_after = r#"<CharacterCampaignData name="Ada"><inventory><Item name="diving suit"/></inventory></CharacterCampaignData>"#;
    let (s, v) = h.transfer(&a, "conn-1-2", &[(P1, xml1_after), (P2, XML2)]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["destination"]["instance_id"], b.as_str());
    assert_eq!(v["expires_in_secs"], 300);

    // The source zone no longer owns them.
    assert_eq!(h.save(P1, &a, XML1).await, StatusCode::CONFLICT);

    // Arriving at the destination returns the character exactly as it left.
    let (_, v) = h.claim(P1, &b).await;
    assert_eq!(v["status"], "arrive");
    assert_eq!(v["character_xml"], xml1_after);
    let (_, v) = h.claim(P2, &b).await;
    assert_eq!(v["status"], "arrive");

    // Reconnecting later is a resume, not a second arrival.
    let (_, v) = h.claim(P1, &b).await;
    assert_eq!(v["status"], "resume");

    // Joining the old zone again redirects to where the character actually is,
    // both while zone B holds them and after B has released them.
    let (_, v) = h.claim(P1, &a).await;
    assert_eq!(v["status"], "redirect");
    assert_eq!(v["instance_id"], b.as_str());
    let (s, _) = h
        .call(
            "POST",
            &format!("/v1/players/{P1}/release"),
            Some(json!({ "instance_id": b, "character_xml": xml1_after })),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, v) = h.claim(P1, &a).await;
    assert_eq!(v["status"], "redirect");
    assert_eq!(v["instance_id"], b.as_str());
}

#[sqlx::test(migrator = "baro_orchestrator::MIGRATOR")]
async fn a_character_cannot_be_live_in_two_instances(pool: PgPool) {
    let h = Harness::new(pool).await;
    let a1 = h.register("loc-1", 32).await;
    let a2 = h.register("loc-1", 31).await;
    h.new_player(P1, &a1, XML1).await;

    // Joining a second copy of the zone while the first still holds the character sends
    // the player back to the first, and the second copy cannot write the character.
    let (_, v) = h.claim(P1, &a2).await;
    assert_eq!(v["status"], "redirect");
    assert_eq!(v["instance_id"], a1.as_str());
    assert_eq!(h.save(P1, &a2, XML2).await, StatusCode::CONFLICT);

    // Once the first instance dies, its lease can be taken over.
    h.kill(&a1).await;
    let (_, v) = h.claim(P1, &a2).await;
    assert_eq!(v["status"], "resume");
    assert_eq!(v["character_xml"], XML1);
    // The dead instance can no longer write over it.
    assert_eq!(h.save(P1, &a1, XML2).await, StatusCode::CONFLICT);
}

#[sqlx::test(migrator = "baro_orchestrator::MIGRATOR")]
async fn release_keeps_the_character_where_it_is(pool: PgPool) {
    let h = Harness::new(pool).await;
    let a = h.register("loc-1", 32).await;
    h.new_player(P1, &a, XML1).await;
    let (s, _) = h
        .call(
            "POST",
            &format!("/v1/players/{P1}/release"),
            Some(json!({ "instance_id": a, "character_xml": XML2 })),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    // Released: another instance of the same zone may pick them up immediately.
    let a2 = h.register("loc-1", 31).await;
    let (_, v) = h.claim(P1, &a2).await;
    assert_eq!(v["status"], "resume");
    assert_eq!(v["character_xml"], XML2);
}

#[sqlx::test(migrator = "baro_orchestrator::MIGRATOR")]
async fn transfers_are_all_or_nothing(pool: PgPool) {
    let h = Harness::new(pool).await;
    let a = h.register("loc-1", 32).await;
    let a2 = h.register("loc-1", 31).await;
    h.register("conn-1-2", 32).await;
    h.new_player(P1, &a, XML1).await;
    h.new_player(P2, &a2, XML2).await; // owned by a different instance

    let (s, v) = h.transfer(&a, "conn-1-2", &[(P1, XML1), (P2, XML2)]).await;
    assert_eq!((s, v["error"].as_str()), (StatusCode::CONFLICT, Some("not_owner")));

    // Nothing moved: P1 is still owned by `a` and still in loc-1.
    assert_eq!(h.save(P1, &a, XML1).await, StatusCode::NO_CONTENT);
    let (_, v) = h.claim(P1, &a).await;
    assert_eq!(v["status"], "resume");
}

#[sqlx::test(migrator = "baro_orchestrator::MIGRATOR")]
async fn transfers_must_follow_the_map(pool: PgPool) {
    let h = Harness::new(pool).await;
    let a = h.register("loc-1", 32).await;
    h.register("loc-3", 32).await;
    h.new_player(P1, &a, XML1).await;

    let (s, v) = h.transfer(&a, "loc-3", &[(P1, XML1)]).await;
    assert_eq!((s, v["error"].as_str()), (StatusCode::CONFLICT, Some("not_adjacent")));
    let (s, v) = h.transfer(&a, "loc-404", &[(P1, XML1)]).await;
    assert_eq!((s, v["error"].as_str()), (StatusCode::NOT_FOUND, Some("unknown_zone")));
}

#[sqlx::test(migrator = "baro_orchestrator::MIGRATOR")]
async fn capacity_counts_crews_still_in_transit(pool: PgPool) {
    let h = Harness::new(pool).await;
    let a = h.register("loc-1", 32).await;
    h.register("conn-1-2", 2).await;
    h.new_player(P1, &a, XML1).await;
    h.new_player(P2, &a, XML2).await;
    h.new_player("STEAM_1:1:3003", &a, XML1).await;

    // A crew of 3 doesn't fit in 2 slots.
    let (s, v) = h.transfer(&a, "conn-1-2", &[(P1, XML1), (P2, XML2), ("STEAM_1:1:3003", XML1)]).await;
    assert_eq!((s, v["error"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("no_capacity")));

    // 2 fit, and while they are in transit the zone counts as full.
    let (s, _) = h.transfer(&a, "conn-1-2", &[(P1, XML1), (P2, XML2)]).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = h.transfer(&a, "conn-1-2", &[("STEAM_1:1:3003", XML1)]).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
}

#[sqlx::test(migrator = "baro_orchestrator::MIGRATOR")]
async fn dead_destinations_receive_no_one(pool: PgPool) {
    let h = Harness::new(pool).await;
    let a = h.register("loc-1", 32).await;
    let b = h.register("conn-1-2", 32).await;
    h.new_player(P1, &a, XML1).await;
    h.kill(&b).await;
    let (s, _) = h.transfer(&a, "conn-1-2", &[(P1, XML1)]).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);

    // A heartbeat brings it back.
    let (s, _) = h
        .call("POST", &format!("/v1/zones/{b}/heartbeat"), Some(json!({ "player_count": 0 })))
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = h.transfer(&a, "conn-1-2", &[(P1, XML1)]).await;
    assert_eq!(s, StatusCode::OK);
}

#[sqlx::test(migrator = "baro_orchestrator::MIGRATOR")]
async fn world_lists_zones_links_and_live_instances(pool: PgPool) {
    let h = Harness::new(pool).await;
    let a = h.register("loc-1", 32).await;
    let dead = h.register("loc-2", 32).await;
    h.kill(&dead).await;
    let (s, v) = h.call("GET", "/v1/world", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["zones"].as_array().unwrap().len(), 4);
    assert_eq!(v["links"].as_array().unwrap().len(), 2);
    let instances = v["instances"].as_array().unwrap();
    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0]["instance_id"], a.as_str());
}

#[sqlx::test(migrator = "baro_orchestrator::MIGRATOR")]
async fn rejects_bad_payloads(pool: PgPool) {
    let h = Harness::new(pool).await;
    let a = h.register("loc-1", 32).await;
    h.new_player(P1, &a, XML1).await;
    assert_eq!(h.save(P1, &a, "not xml").await, StatusCode::BAD_REQUEST);
    let (s, _) = h.transfer(&a, "conn-1-2", &[]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = h.transfer(&a, "conn-1-2", &[(P1, XML1), (P1, XML1)]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrator = "baro_orchestrator::MIGRATOR")]
async fn simultaneous_first_joins_create_one_character(pool: PgPool) {
    let h = Harness::new(pool).await;
    let a1 = h.register("loc-1", 32).await;
    let a2 = h.register("loc-1", 31).await;
    for i in 0..20 {
        let account = format!("STEAM_1:0:9{i:03}");
        let (r1, r2) = tokio::join!(h.claim(&account, &a1), h.claim(&account, &a2));
        assert_eq!(r1.0, StatusCode::OK, "{}", r1.1);
        assert_eq!(r2.0, StatusCode::OK, "{}", r2.1);
        // Exactly one instance gets the new character; the other is sent to it.
        let statuses = [r1.1["status"].as_str().unwrap(), r2.1["status"].as_str().unwrap()];
        let mut sorted = statuses;
        sorted.sort();
        assert_eq!(sorted, ["new", "redirect"], "{statuses:?}");
    }
}
