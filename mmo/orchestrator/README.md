# baro-orchestrator

Part of the Barotrauma MMO mod. **This is a modified version of Barotrauma**, not the original
software, and is not made or endorsed by Undertow Games. See `docs/mmo/DESIGN.md`.

The orchestrator owns the world map, keeps a registry of running zone servers, holds the
"who owns this character" leases that stop items being duplicated, and moves crews between
zones. Zone servers talk to it over HTTP; the contract is in `docs/mmo/PROTOCOL.md`.

## Run it

Needs Rust (stable) and PostgreSQL 14+.

```sh
createdb mmo
export DATABASE_URL=postgres://USER:PASS@localhost/mmo
export ZONE_SECRET=$(openssl rand -hex 24)   # zone servers must send this
export WORLD_FILE=world.example.json
cargo run --release
```

Migrations run automatically on start-up, and the world file is synced into the database.

| Variable | Default | Meaning |
|---|---|---|
| `DATABASE_URL` | required | Postgres connection string |
| `ZONE_SECRET` | required, ≥16 chars | Shared secret zone servers send as `Authorization: Bearer …` |
| `WORLD_FILE` | `world.json` | World map (zones, entry zones, links) |
| `BIND_ADDR` | `0.0.0.0:8080` | Listen address |
| `HEARTBEAT_INTERVAL_SECS` | `10` | Zones heartbeat this often; 3 missed = dead |
| `TRANSFER_TTL_SECS` | `300` | How long seats are held for a crew in transit |
| `RUST_LOG` | `info` | Log filter |

## Test it

```sh
export DATABASE_URL=postgres://USER:PASS@localhost/postgres   # user needs CREATEDB
cargo test
```

Each integration test runs against its own fresh database and covers: auth, registration,
entry-zone rules, full crew transfer and arrival, redirects, the anti-duplication lease
(including takeover from a dead zone), all-or-nothing transfers, map adjacency, capacity
including crews in transit, dead destinations, and simultaneous first joins.

## World file

```json
{
  "zones": [ { "key": "loc-0", "name": "Surface Station", "entry": true }, … ],
  "links": [ ["loc-0", "conn-0-1"], … ]
}
```

Keys are lowercase letters, digits and `-`. Use `loc-<n>` for locations and `conn-<a>-<b>` for
the routes between them. At least one zone must be an entry zone (where new players start).
