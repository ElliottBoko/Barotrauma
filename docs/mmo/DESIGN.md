# Barotrauma MMO — Design

> **This is a modified version of Barotrauma.** It is not the original software and is not
> made or endorsed by Undertow Games / FakeFish. All rights to Barotrauma belong to Undertow Games.

Status: draft · Owner: Elliott (ElliottBoko)

Related: `PROTOCOL.md` (zone ⇄ orchestrator API), `ZONE_SETUP.md` (running zones), `LOADTEST.md` (fake players).

| Branch | Contents | Built? |
|---|---|---|
| `mmo/phase0-32-players` | Player cap 32 | Not yet (needs laptop) |
| `mmo/main` | Docs + Rust orchestrator | Yes: 20 tests pass |
| `mmo/zone-hooks` | `mmo/main` + C# game hooks + contract test | Orchestrator client yes; game hooks not yet |

## 1. Goal

Turn Barotrauma into a persistent, shared-world MMO:

- **Up to 32 players per server instance.**
- **The world map is split across many instances**, one per map location / level ("zones").
- Players keep their character, inventory, health and money as they travel between zones.

## 2. Hard constraints (licence)

From `EULA.txt` in this repo. These are not negotiable and shape the design:

| Rule | Consequence for us |
|---|---|
| Modified code may only be used as a mod for Barotrauma, non-commercially | The MMO is **free**. No paid tiers, cosmetics shop or donations-for-perks. |
| Modified software may only go to people who already own Barotrauma | Players must own the game on Steam. |
| Must not remove or bypass Steam integration, SteamID auth or DRM | **Steam login stays.** We use it as the MMO's identity system. |
| Modified software must be marked as modified and credit Undertow Games | Notice at the top of this doc, the README, and the in-game main menu. |
| Undertow Games gets a licence to anything we distribute | Accepted; keep that in mind before adding anything we want to own exclusively. |

## 3. Design principles

1. **Minimal hooks inside, services outside.** Changes to the game code stay small and
   targeted. All MMO machinery (orchestrator, database, world map, login) lives in separate
   services. This keeps the fork easy to rebase when upstream patches the game.
2. **Zones follow the campaign map.** One instance = one location or one level (connection).
   Zone changes happen at level exits, where the game already does a full load. No seamless
   cross-server physics.
3. **Reuse what the engine already has** (e.g. `CharacterCampaignData` as the transfer payload)
   before writing anything new.
4. **Borrow proven MMO patterns** instead of inventing them (see §7).
5. **Every phase ends in something runnable and testable.**

## 4. Engine findings

From reading the source at upstream commit `e6e95da` (2 Sep 2026).

| Area | Where | Finding | Impact |
|---|---|---|---|
| Player cap | `Barotrauma/BarotraumaShared/SharedSource/Networking/NetConfig.cs` | Single constant `MaxPlayers = 16`. | Raised to 32 in phase 0. |
| Session IDs | `BarotraumaShared/SharedSource/Networking/Client.cs` | `byte SessionId`. | Fine up to 255. |
| Connection limit | `BarotraumaServer/.../Peers/Server/LidgrenServerPeer.cs` | `MaxPlayers * 2`. | Scales automatically. |
| Entity IDs | `BarotraumaShared/SharedSource/Map/Entity.cs` | `ushort`, ~65k per round. | Per-zone, not global. Fine. |
| Player subs | `BarotraumaShared/SharedSource/Map/Submarine.cs` | `MainSubs = new Submarine[2]`, used in ~20 files. | **Biggest refactor** (phase 3). |
| Teams | `BarotraumaShared/SharedSource/Characters/Character.cs` | `CharacterTeamType`: Team1, Team2, FriendlyNPC. | Needs per-crew factions (phase 3). |
| Character persistence | `BarotraumaShared/.../GameModes/CharacterCampaignData.cs` and server counterpart | Character info, inventory, health, orders and wallet serialized to XML, keyed by Steam `AccountId`. | **This is the zone-transfer payload.** |
| Build | `BarotraumaServer/LinuxServer.csproj` etc. | `net8.0`; Linux, Windows and Mac server projects. | Zone servers can run on Linux. |

## 5. Architecture

```
                        ┌──────────────────────────┐
  Steam login ─────────▶│       Orchestrator       │◀──── admin tools
                        │ world map · zone registry│
                        │ transfer tokens · spawn  │
                        └─────┬──────────────┬─────┘
                              │              │ start/stop
                    read/write│              ▼
                        ┌─────┴─────┐   ┌─────────┐ ┌─────────┐ ┌─────────┐
                        │ Postgres  │◀──│ Zone A  │ │ Zone B  │ │ Zone C  │ … (≤32 players each)
                        │ characters│   │ (fork)  │ │ (fork)  │ │ (fork)  │
                        │ subs, econ│   └─────────┘ └─────────┘ └─────────┘
                        └───────────┘         ▲          ▲
                                              └── client reconnects ──┘
```

**Zone server (the fork).** A normal Barotrauma dedicated server with small hooks:
on player join → ask the orchestrator (by Steam AccountId) whether to load a character, start
fresh, or redirect; on player leave → save and release; at a level exit → transfer the crew and
redirect them. No tokens pass through the client: Steam auth already proves who the player is.

**Orchestrator (new service).** Owns the world map, knows which zone instances are running,
issues short-lived transfer tokens, starts and stops zone servers, and overflows busy zones
into extra copies.

**Database.** Postgres. Characters (keyed by Steam AccountId), owned submarines, location state,
and the shared economy.

**Client (the fork).** Understands a "reconnect to zone X with token T" message and shows the
modification notice. Players must run this build.

### Zone transfer flow

1. Sub reaches a level exit in zone A.
2. Zone A sends each crew member's `CharacterCampaignData` to the orchestrator in one atomic
   transfer; the orchestrator picks a live zone-B instance with room.
3. Zone A removes the characters and their items, and sends each client `ZONE_REDIRECT`.
4. Clients reconnect to B through Steam as normal.
5. Zone B claims each player by AccountId, gets their character, and the respawn manager spawns
   them with their saved inventory.

Full detail and failure cases: `PROTOCOL.md`.

## 6. Build phases

| Phase | Deliverable | Done when |
|---|---|---|
| **0 — Cap & version check** | `MaxPlayers = 32` (done, on this branch). Decompile the installed game with ILSpy and compare to this source. | Fork builds; we know whether public source matches the Steam build. |
| **1 — 32 in one instance** | Dedicated server build, 32-player stress test. | Stable tick rate with 32 players, AI and flooding active. |
| **2 — Database + two-zone handoff** | Orchestrator v0 ✅, Postgres schema ✅, transfer hooks (written, unbuilt). | One character sails out of zone A and arrives in zone B with inventory intact. |
| **3 — Multiple crews per zone** | Replace the 2-slot `MainSubs` array and 2-team model. | Three independent player subs share one zone without conflicts. |
| **4 — Persistent world** | Every map location is a live zone; overflow instancing; shared economy. | A full map runs with players moving freely between zones. |

## 7. Patterns borrowed from other games

- **EVE Online:** one solar system per server, travel through gates. Our zone model is the same idea.
- **WoW-style instancing:** extra copies of a full zone spin up when it hits 32 players.
- **Albion Online:** full-loot risk that scales with danger. Maps onto Europa's depth naturally:
  safer near the surface, deadlier and richer deeper down.

## 8. Open decisions

- ~~**Orchestrator language**~~ **Decided: Rust.** It could be built and tested in the cloud workspace
  (C# game builds there can't fetch NuGet packages), and it treats character XML as opaque, so
  sharing C# save code wasn't needed after all.
- **Hosting:** Elliott's PC for development; rented Linux server(s) for real zones.
- **Submarine ownership:** personal, crew-owned, or rented per voyage.
- **Death rules:** full loot, partial loot, or permadeath per zone tier.

## 9. Risks

- **Character duplication** (the classic MMO exploit). Handled by ownership leases: only one live
  zone can hold a character, transfers are all-or-nothing, and transferred characters' items are
  deleted from the source zone. Covered by orchestrator tests.

- **Source/Steam version mismatch.** If the public source lags the Steam build, protocol
  differences could block clients. Phase 0's ILSpy check covers this.
- **Single-threaded server performance** at 32 players. Phase 1 measures it before we build on it.
- **Multi-crew refactor size.** `MainSubs` touches ~20 files; expect most of phase 3's effort here.
- **Upstream updates.** Each game patch must be merged into the fork. Principle 1 keeps that manageable.
- **Licence changes.** Undertow Games can change the EULA at any time; re-check it on each upstream merge.
