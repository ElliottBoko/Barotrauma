# Barotrauma MMO mod — project briefing

This repo is Elliott's fork of Barotrauma (FakeFishGames/Barotrauma, upstream v1.13.4.0), being
turned into an MMO: up to 32 players per server instance, with the campaign map split into zones
(one zone server per map location or route). **This is a modified version of Barotrauma**, not
made or endorsed by Undertow Games / FakeFish.

## Read first

- `docs/mmo/DESIGN.md` — goals, licence constraints, architecture, phases, open decisions.
- `docs/mmo/PROTOCOL.md` — zone ⇄ orchestrator HTTP API and the zone-transfer flow.
- `docs/mmo/ZONE_SETUP.md` — how to run the orchestrator + two zone servers on Windows.
- `docs/mmo/LOADTEST.md` — plan for fake-player load testing (not built yet).

## Licence rules (EULA.txt) — hard constraints

- Free and non-commercial only; only for people who already own Barotrauma.
- Never remove or bypass Steam integration, SteamID auth or DRM. Steam login is the MMO's identity.
- Modified builds must be plainly marked as modified (client window title does this).

## Branches

- `mmo/phase0-32-players` — `NetConfig.MaxPlayers` 16 → 32. Builds on Windows.
- `mmo/main` — docs + Rust orchestrator (`mmo/orchestrator`, 21 tests passing).
- `mmo/zone-hooks` — everything above + C# game hooks + contract test. **Server builds cleanly on
  Windows** (2026-10-17). Client build and the two-zone test not yet done. Work here.

## Layout of MMO code

- `mmo/orchestrator/` — Rust (axum + sqlx + Postgres). World map, zone registry, character
  ownership leases (anti-duplication), crew transfers. `cargo test` needs `DATABASE_URL` pointing
  at a Postgres user with CREATEDB.
- `mmo/contract-test/` — compiles the game's `OrchestratorClient.cs` standalone and checks it
  against a running orchestrator.
- `Barotrauma/BarotraumaServer/ServerSource/Networking/Mmo/` — `MmoZone.cs` (zone logic),
  `OrchestratorClient.cs` (HTTP client).
- Partial-class files: `GameServer.Mmo.cs`, `MultiPlayerCampaign.Mmo.cs` (server),
  `GameClient.Mmo.cs` (client). Hooks in vanilla files are marked `// MMO mod:`; keep those
  edits minimal so upstream merges stay easy.
- Zone mode is off unless `MMO_ORCHESTRATOR_URL` is set; without it the fork is a vanilla server.

## Building (Windows, PowerShell)

```powershell
cd Barotrauma\BarotraumaServer; dotnet build WindowsServer.csproj -c Release
cd ..\BarotraumaClient;        dotnet build WindowsClient.csproj -c Release
```

- Output: `Barotrauma\bin\ReleaseWindows\net8.0\` (`DedicatedServer.exe`, `Barotrauma.exe`).
- ~150 warnings are vanilla and expected. "Epic could not be found" warnings are the private Epic
  SDK module, which isn't in the public source; ignore them (Steam is what we use).
- The game's art/sound/XML (`Barotrauma\BarotraumaShared\Content`) is not in git. Copy it from the
  Steam install's `Content` folder, then rebuild.

## Next steps

1. Build the client on `mmo/zone-hooks`; fix any errors.
2. Run the two-zone test in `docs/mmo/ZONE_SETUP.md` (needs Rust + PostgreSQL 16 installed).
   Done when a character sails out of zone A (`loc-0`) and spawns in zone B (`conn-0-1`) with the
   same inventory.
3. Then: phase 1 load test (32 players), phase 3 multiple player subs per zone
   (`Submarine.MainSubs` is a fixed 2-slot array used in ~20 files — the biggest refactor).

## Conventions

- Commit as Elliott; keep commits small with clear messages.
- Never commit the `Content` folder or secrets (`ZONE_SECRET`, database passwords).
- Unauthenticated (`UNAUTHENTICATED_<name>`) accounts are refused by both zones and orchestrator
  unless the explicit load-test-only setting is on; don't weaken that.
