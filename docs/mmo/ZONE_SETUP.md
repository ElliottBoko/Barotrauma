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

## Two-zone test on one PC (Windows, PowerShell)

Goal (phase 2 "done when"): a character sails out of zone A and arrives in zone B with its inventory.
Paths below assume the repo is at `C:\Users\<you>\Barotrauma`.

### 1. Game assets

The repo has no art, sounds or item XML; Undertow only ships them with the game. Copy them in from
your own Steam install (the EULA allows this for people who own the game):

```powershell
Copy-Item -Recurse "C:\Program Files (x86)\Steam\steamapps\common\Barotrauma\Content" `
  "$HOME\Barotrauma\Barotrauma\BarotraumaShared\Content"
```

Then rebuild both projects so the assets are copied next to the executables:

```powershell
cd $HOME\Barotrauma\Barotrauma\BarotraumaServer; dotnet build WindowsServer.csproj -c Release
cd ..\BarotraumaClient; dotnet build WindowsClient.csproj -c Release
```

Output: `Barotrauma\bin\ReleaseWindows\net8.0\` (server: `DedicatedServer.exe`, client: `Barotrauma.exe`).

### 2. Tools

- **Rust:** run the installer from https://rustup.rs and accept the defaults, including the
  Visual Studio C++ build tools it offers.
- **PostgreSQL 16:** Windows installer from https://www.postgresql.org/download/windows/ .
  Remember the password you set for the `postgres` user; keep port 5432; skip Stack Builder.

Create the database (enter the password when asked):

```powershell
& "C:\Program Files\PostgreSQL\16\bin\createdb.exe" -U postgres mmo
```

### 3. Orchestrator (terminal 1)

```powershell
cd $HOME\Barotrauma\mmo\orchestrator
$env:DATABASE_URL = "postgres://postgres:YOUR_PASSWORD@localhost/mmo"
$env:ZONE_SECRET  = "change-me-to-a-long-random-string"
$env:WORLD_FILE   = "world.example.json"
cargo run --release
```

The first run compiles for a few minutes. Ready when it prints `orchestrator listening`.

### 4. Two zone servers (terminals 2 and 3)

Each zone needs its own copy of the server folder, so their settings and saves don't collide:

```powershell
Copy-Item -Recurse $HOME\Barotrauma\Barotrauma\bin\ReleaseWindows\net8.0 $HOME\zoneA
Copy-Item -Recurse $HOME\Barotrauma\Barotrauma\bin\ReleaseWindows\net8.0 $HOME\zoneB
```

Zone A, the surface station (`loc-0`), in terminal 2:

```powershell
cd $HOME\zoneA
$env:MMO_ORCHESTRATOR_URL = "http://127.0.0.1:8080"
$env:MMO_ZONE_SECRET      = "change-me-to-a-long-random-string"
$env:MMO_ZONE_KEY         = "loc-0"
$env:MMO_PUBLIC_ADDRESS   = "127.0.0.1:27015"
$env:MMO_EXIT_END         = "conn-0-1"
.\DedicatedServer.exe -name "Zone A" -port 27015 -queryport 27016
```

Zone B, the route (`conn-0-1`), in terminal 3:

```powershell
cd $HOME\zoneB
$env:MMO_ORCHESTRATOR_URL = "http://127.0.0.1:8080"
$env:MMO_ZONE_SECRET      = "change-me-to-a-long-random-string"
$env:MMO_ZONE_KEY         = "conn-0-1"
$env:MMO_PUBLIC_ADDRESS   = "127.0.0.1:27017"
$env:MMO_EXIT_START       = "loc-0"
.\DedicatedServer.exe -name "Zone B" -port 27017 -queryport 27018
```

Each server should print `[MMO] Registered as instance …`, and the orchestrator logs `zone registered`.

### 5. Play

1. Launch the modded client (`Barotrauma.exe` from the build output; the window title says
   "modified"). Steam must be running.
2. Join zone A (Join server → direct join `127.0.0.1:27015`). Start a **multiplayer campaign**
   with **Respawn mode: Mid-round**. Do the same on zone B (join it once, set it up, leave).
3. In zone A, pick up some items, undock and sail out of the outpost.
4. Expected: you're moved to zone B and spawn holding the same items. The orchestrator log shows
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
