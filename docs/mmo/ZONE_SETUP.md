# Running zone servers (v0)

> Part of the Barotrauma MMO mod, a modified version of Barotrauma. See `DESIGN.md`.

**Status: written but not yet compiled.** The game hooks on branch `mmo/zone-hooks` could not be
built in the cloud workspace (the game's NuGet packages aren't reachable there). The orchestrator
HTTP client inside them *has* been compiled and tested against the real orchestrator
(`mmo/contract-test`). Expect a few compile fixes on first build.

## What the hooks do

Zone mode is **off unless `MMO_ORCHESTRATOR_URL` is set**, so the fork still runs as a normal
Barotrauma server without it.

| When | What happens | Where |
|---|---|---|
| Server starts | Registers with the orchestrator, then heartbeats | `GameServer.StartServer` → `MmoZone.TryStart` |
| Player joins | Asks the orchestrator about them: load their character, start fresh, or send them to the right zone | `GameServer.OnInitializationComplete` → `MmoZone.OnClientJoined` |
| Every 60 s | Saves every living character this zone holds | `MmoZone.Update` |
| Sub reaches a level exit | Instead of changing level, the crew aboard is transferred to the next zone and told to reconnect there; their characters and items are removed here | `CampaignMode.LoadNewLevel` → `MmoZone.OnLevelExit` |
| Player leaves | Saves and releases their character | `GameServer.DisconnectClient` → `MmoZone.OnClientDisconnecting` |
| Client receives redirect | Quits and connects to the new zone (reuses the game's own reconnect mechanism) | `GameClient.ReadZoneRedirect` |

Changes to existing game files total ~35 lines; the rest is in new files:
`BarotraumaServer/ServerSource/Networking/Mmo/`, `GameServer.Mmo.cs`, `MultiPlayerCampaign.Mmo.cs`,
`GameClient.Mmo.cs`.

## Environment variables (per zone server)

| Variable | Example | Meaning |
|---|---|---|
| `MMO_ORCHESTRATOR_URL` | `http://127.0.0.1:8080` | Turns zone mode on |
| `MMO_ZONE_SECRET` | (same as orchestrator `ZONE_SECRET`) | Auth for orchestrator calls |
| `MMO_ZONE_KEY` | `loc-0` | Which world-map zone this server hosts |
| `MMO_PUBLIC_ADDRESS` | `127.0.0.1:27015` | Address players are sent to for this zone |
| `MMO_EXIT_END` | `conn-0-1` | Zone reached by leaving through the end of the level (optional) |
| `MMO_EXIT_START` | `loc-0` | Zone reached by leaving through the start (optional) |

## Two-zone test on one PC

Goal (phase 2 "done when"): a character sails out of zone A and arrives in zone B with its inventory.

1. **Build** the fork's server and client (`WindowsServer.csproj`, `WindowsClient.csproj`) from
   branch `mmo/zone-hooks`. Fix any compile errors first.
2. **Orchestrator.** Install Rust and PostgreSQL, then from `mmo/orchestrator`:
   ```
   set DATABASE_URL=postgres://postgres:PASSWORD@localhost/mmo
   set ZONE_SECRET=pick-a-long-random-string
   set WORLD_FILE=world.example.json
   cargo run --release
   ```
3. **Zone A** (`loc-0`), in its own folder copy of the server build, own terminal:
   ```
   set MMO_ORCHESTRATOR_URL=http://127.0.0.1:8080
   set MMO_ZONE_SECRET=pick-a-long-random-string
   set MMO_ZONE_KEY=loc-0
   set MMO_PUBLIC_ADDRESS=127.0.0.1:27015
   set MMO_EXIT_END=conn-0-1
   DedicatedServer.exe -name "Zone A" -port 27015 -queryport 27016
   ```
4. **Zone B** (`conn-0-1`), second folder copy, ports 27017/27018,
   `MMO_ZONE_KEY=conn-0-1`, `MMO_PUBLIC_ADDRESS=127.0.0.1:27017`, `MMO_EXIT_START=loc-0`.
5. On each server, start a **multiplayer campaign** and set **Respawn mode: Mid-round** (arriving
   players are spawned by the respawn system). Zone A should start at an outpost; zone B on a
   route level.
6. Join zone A with the modded client, pick up some items, undock and leave the outpost.
   You should be moved to zone B and spawn with the same items. The orchestrator log shows
   `claim … new`, `transfer`, then `claim … arrive`.

A single PC can't easily run two clients on one Steam account, so this test uses one player.
Multi-player tests need the load-test tool (`LOADTEST.md`) or friends.

## Known gaps in v0

- **Campaign setup is manual.** Each zone needs a campaign started by hand; automating it
  (fixed seed, correct level per zone key) is follow-up work.
- **Routes are fixed per server.** `MMO_EXIT_END/START` pick the neighbour; leaving an outpost
  always goes to `MMO_EXIT_END`. Choosing among several routes needs map UI work.
- **Death rules undecided.** A dead character isn't saved, so the last save stands (no loss).
- **The sub doesn't travel.** Crew transfers; the destination zone provides the sub (phase 3).
- **Spawn timing.** If the orchestrator is slow, a player could spawn before their saved
  character loads. Measure on first test.
