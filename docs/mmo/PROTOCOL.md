# Zone ⇄ Orchestrator Protocol (v1)

> Part of the Barotrauma MMO mod. This is a modified version of Barotrauma; see `DESIGN.md`.

How zone servers (the forked dedicated server) talk to the orchestrator. Players never call
the orchestrator directly in v1: they only ever connect to zone servers, through Steam, as normal.

## 1. What the engine already gives us

Found while reading upstream `1.13.4.0`:

| Need | Existing mechanism | Where |
|---|---|---|
| Send a client to another server | Set `GameMain.Instance.ConnectCommand`; the main loop quits the current client and connects. Already used to rejoin after Workshop downloads. | `BarotraumaClient/ClientSource/GameMain.cs` (connect-command loop), `Steam/BulkDownloader.cs` |
| Serialize a player | `CharacterCampaignData.Save()` → XML: character info, skills, inventory, health, orders, wallet. Keyed by Steam `AccountId`. | `.../GameModes/CharacterCampaignData.cs` (shared + server) |
| Spawn a player mid-round from saved data | `RespawnManager` with `RespawnMode.MidRound` calls `SpawnInventoryItems`, `ApplyHealthData`, `ApplyWalletData`. | `BarotraumaServer/ServerSource/Networking/RespawnManager.cs` |
| Detect a sub at a level exit | `Submarine.AtEndExit` / `AtStartExit`. | `BarotraumaShared/SharedSource/Map/Submarine.cs` |
| Verified identity | Steam auth on connect gives a trusted `AccountId`. | `Client.AccountId` |

Consequence: **the client needs only one new packet** (`ZONE_REDIRECT`). Arrival is recognised
server-side by the player's Steam AccountId, so the client never handles transfer tokens.

## 2. Concepts

- **Zone key**: a stable name for a place on the world map.
  `loc-<n>` for a location, `conn-<a>-<b>` for the level between two locations (a < b).
- **Zone instance**: one running dedicated server hosting one zone key. A busy zone can have
  several instances (overflow copies).
- **Character record**: the latest `CharacterCampaignData` XML for a Steam account, plus where
  that character currently is (`zone_key`).
- **Ownership lease**: at most one zone instance owns a character at a time. Only the owner may
  save it. This is the anti-duplication rule: a character cannot be live in two zones, so items
  and money cannot be cloned by racing two connections.
- **Transfer**: a character moving from one zone to another. Pending until the destination
  claims it, or until it expires.

## 3. Authentication

Zone servers send `Authorization: Bearer <zone secret>`. Secrets are configured on the
orchestrator. v1 uses a single shared secret; per-instance secrets come later.
All endpoints are JSON over HTTP; put TLS in front in production.

## 4. Endpoints

### `POST /v1/zones/register`
A zone server starting up.
```json
{ "zone_key": "loc-3", "address": "203.0.113.5:27015", "server_name": "Europa · Outpost 3", "max_players": 32 }
```
→ `200 { "instance_id": "uuid", "heartbeat_interval_secs": 10 }`
`404 unknown_zone` if the zone key is not on the world map.

### `POST /v1/zones/{instance_id}/heartbeat`
```json
{ "player_count": 17 }
```
→ `204`. Instances missing 3 heartbeats are considered dead: they stop receiving players and
their leases can be taken over.

### `POST /v1/players/{account_id}/claim`
Called by a zone **when a Steam-authenticated client joins**. The orchestrator decides what
that player should do.
```json
{ "instance_id": "uuid" }
```
Responses (`200`), by `status`:

| status | Meaning | Zone server does |
|---|---|---|
| `arrive` | Pending transfer into this zone; lease granted. Body has `character_xml`. | Load into campaign character data; respawn manager spawns them. |
| `resume` | Character's current zone is this one (e.g. reconnect); lease granted. Body has `character_xml`. | Same as `arrive`. |
| `new` | No character exists and this is an entry zone; lease granted. | Normal new-character flow. |
| `redirect` | Character is in another zone, is still held by another live instance (usually a dropped connection that zone hasn't noticed yet), or is a new player in a non-entry zone. Body has `instance_id`, `address`, `server_name`. | Send `ZONE_REDIRECT`, then drop the client. |

### `PUT /v1/players/{account_id}/character`
Periodic save and save-on-disconnect.
```json
{ "instance_id": "uuid", "character_xml": "<CharacterCampaignData …/>" }
```
→ `204`, or `409 not_owner` if this instance does not hold the lease.

### `POST /v1/players/{account_id}/release`
Player left the zone without transferring. Saves and releases the lease; the character stays in
this zone.
```json
{ "instance_id": "uuid", "character_xml": "…" }
```

### `POST /v1/transfers`
A sub reached a level exit. Moves its crew to the destination zone.
```json
{
  "instance_id": "uuid",
  "destination_zone_key": "conn-3-7",
  "players": [ { "account_id": "STEAM_…", "character_xml": "…" } ]
}
```
→ `200`
```json
{
  "transfer_id": "uuid",
  "destination": { "instance_id": "uuid", "address": "203.0.113.6:27015", "server_name": "…" },
  "expires_in_secs": 300
}
```
Atomically, for every player: saves `character_xml`, sets `zone_key` to the destination,
releases the source lease, and records a pending arrival. The destination instance is the live
one with the most free slots that fits the whole crew.
Validation: crew must be non-empty with no duplicate accounts (`400`).
Errors: `409 not_owner` (source doesn't own one of the characters; nothing is changed),
`409 not_adjacent` (destination not connected to source on the world map),
`503 no_capacity` (no live destination instance can fit the crew).

### `GET /v1/world`
The world map: zones, which are entry zones, adjacency, and live instances with player counts.

## 5. New game packet

`ServerPacketHeader.ZONE_REDIRECT` (appended to the end of the enum so existing values keep
their numbers):

| Field | Type |
|---|---|
| server name | string |
| endpoint | string (Lidgren `ip:port` or Steam P2P endpoint, parsed with `Endpoint.Parse`) |

Client handling: build a `ConnectCommand(serverName, endpoint)` and set it on
`GameMain.Instance.ConnectCommand`. The main loop does the rest.

## 6. Flows

**Zone change.**
1. Sub reaches `AtEndExit`/`AtStartExit` in zone A; crew confirms.
2. A serialises each crew member (`CharacterCampaignData`) and calls `POST /v1/transfers`.
3. A sends each crew member `ZONE_REDIRECT` with the destination's address, then removes their characters.
4. Clients reconnect to zone B through Steam.
5. B calls `claim` for each → `arrive` with their XML → respawn manager spawns them.

**Disconnect and return.** Zone calls `release` on disconnect. Next time the player joins any
zone, `claim` returns `resume` (same zone) or `redirect` (to where they are). If they reconnect
before the old zone has noticed the drop, they are redirected back to that zone, which still holds
their character.

**Two zones racing for a new player.** Character creation is first-wins; the loser redirects the
player to the winner.

**Expired transfer.** If a player never arrives, their character is already saved with
`zone_key` = destination, so their next `claim` anywhere redirects them there. Nothing is lost.

**Zone crash.** Its heartbeats stop; its leases are treated as free after the timeout. Players
resume from their last save.

## 7. Not in v1

- Submarines travelling with the crew (needs multi-sub zones, phase 3). In v1 the crew
  transfers and the destination zone provides the sub.
- Orchestrator starting and stopping zone processes on demand.
- Shared economy and shops.
