# Load testing: fake players

> Part of the Barotrauma MMO mod, a modified version of Barotrauma. See `DESIGN.md`.

Phase 1 needs proof that one zone server holds up with 32 players. Getting 31 real people online
for every test isn't realistic, and the real game client is far too heavy to run 31 copies of.
So we test in two stages.

## Stage 1: server-side stand-ins (cheap, do first)

Spawn AI-controlled crew in a normal campaign round until there are 32 characters, then measure the
server's tick time. This measures the CPU cost of 32 characters (physics, AI, items, flooding,
fire), which is the most likely bottleneck since the server simulation is largely single-threaded.

It does **not** measure network cost (per-client sync traffic), which stage 2 covers.

**Pass mark:** server holds its normal update rate (60 updates/s) with 32 characters during a
busy scenario (combat with monsters, breaches flooding, fires). Record the worst one-minute stretch.

## Stage 2: thin protocol clients ("ghosts")

A small headless program that connects N fake players over the real network protocol, without
graphics, sound or game simulation.

### Why it's feasible

From reading the netcode (upstream 1.13.4.0):

- **Logging in without Steam.** If the server's vanilla setting `RequireAuthentication` is false,
  clients without a Steam ticket are accepted as `UNAUTHENTICATED_<name>`
  (`LidgrenServerPeer.ProcessAuthTicket`). This is the game's own LAN option, not a bypass. It is
  **for private test servers only**; the MMO zone code and the orchestrator both refuse those
  accounts unless explicitly put in insecure load-test mode (`MMO_ALLOW_UNAUTHENTICATED` /
  `ALLOW_UNAUTHENTICATED` = `I_UNDERSTAND_THIS_IS_INSECURE`).
- **Skipping what it doesn't understand.** Server messages use segment tables
  (`SegmentTable.cs`): a table of pointers at the end of each message says where each section
  starts. A ghost can jump straight to the sections it needs.
- **Staying in sync cheaply.** The server kicks clients that fall behind on entity events
  (`ServerEntityEventManager`). Each event is length-prefixed
  (`NetEntityEventManager.Write`), so a ghost can count event IDs and acknowledge them without
  decoding a single event.
- **Networking library is in the repo.** Lidgren is vendored as source
  (`Libraries/Lidgren.Network`), so the ghost builds with no NuGet packages.

### What a ghost does

1. Connect with Lidgren and complete the handshake: version and auth packet (no ticket), content
   package list check, name.
2. In the lobby, acknowledge lobby and client-list updates.
3. In game, every ~50 ms send `UPDATE_INGAME` with:
   - **SyncIds**: last chat ID, last entity event ID received, last client-list ID, campaign IDs.
   - **Character input**: walk in a random direction, occasionally use or pick up nearby items, so
     the server does real work for each ghost.
4. Read each `UPDATE_INGAME` from the server: jump to the entity-event segment, read the first ID and
   count, skip each event by its length, and update "last received".
5. Report per ghost: connected?, kicked (and why), round-trip time, bytes in/out per second.

### Running a test

```
ghosts --server 127.0.0.1:27015 --count 31 --duration 10m --report ghosts.csv
```

With one real client also connected, you can see what the server looks like at 32 players.

**Pass mark:** no ghost kicked for desync; server update rate holds; per-client bandwidth noted for
hosting costs.

### Estimated work

- Handshake and lobby: the largest part, because those packets must be written exactly as the
  real client writes them. Mirror `GameClient`'s connection-initialisation code field by field.
- In-game ack loop and inputs: small, thanks to segment tables and length-prefixed events.
- It must be tested against a running dedicated server, so it waits until the fork builds on a
  real machine.

## Stage 3: multi-zone load (later)

Ghosts that also follow `ZONE_REDIRECT`, to test many crews moving between zones at once and the
orchestrator under load. The orchestrator side can already be load-tested on its own with any HTTP
benchmarking tool against the endpoints in `PROTOCOL.md`.
