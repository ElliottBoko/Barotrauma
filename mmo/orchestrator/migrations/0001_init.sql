-- Barotrauma MMO orchestrator schema, v1.
-- Part of a modified version of Barotrauma (see docs/mmo/DESIGN.md).

-- World map: one row per place a zone server can host.
CREATE TABLE zones (
    zone_key      TEXT PRIMARY KEY,                 -- 'loc-3', 'conn-3-7'
    display_name  TEXT NOT NULL,
    is_entry      BOOLEAN NOT NULL DEFAULT FALSE    -- new players may start here
);

-- Undirected adjacency between zones, stored once with zone_a < zone_b.
CREATE TABLE zone_links (
    zone_a  TEXT NOT NULL REFERENCES zones(zone_key) ON DELETE CASCADE,
    zone_b  TEXT NOT NULL REFERENCES zones(zone_key) ON DELETE CASCADE,
    PRIMARY KEY (zone_a, zone_b),
    CHECK (zone_a < zone_b)
);

-- Running dedicated servers. Liveness = recent heartbeat.
CREATE TABLE zone_instances (
    instance_id     UUID PRIMARY KEY,
    zone_key        TEXT NOT NULL REFERENCES zones(zone_key),
    address         TEXT NOT NULL,
    server_name     TEXT NOT NULL,
    max_players     INTEGER NOT NULL CHECK (max_players > 0),
    player_count    INTEGER NOT NULL DEFAULT 0 CHECK (player_count >= 0),
    registered_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_heartbeat  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX zone_instances_zone ON zone_instances (zone_key, last_heartbeat);

-- One character per Steam account. owner_instance is the lease: only that
-- instance may save the character, which prevents item/money duplication.
CREATE TABLE characters (
    account_id      TEXT PRIMARY KEY,               -- e.g. 'STEAM_1:1:508792388'
    zone_key        TEXT NOT NULL REFERENCES zones(zone_key),
    character_xml   TEXT,                           -- CharacterCampaignData; NULL until first save
    owner_instance  UUID REFERENCES zone_instances(instance_id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX characters_owner ON characters (owner_instance);

-- A crew moving between zones.
CREATE TABLE transfers (
    transfer_id    UUID PRIMARY KEY,
    from_instance  UUID REFERENCES zone_instances(instance_id) ON DELETE SET NULL,
    to_instance    UUID REFERENCES zone_instances(instance_id) ON DELETE SET NULL,
    to_zone_key    TEXT NOT NULL REFERENCES zones(zone_key),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at     TIMESTAMPTZ NOT NULL
);

CREATE TABLE transfer_players (
    transfer_id  UUID NOT NULL REFERENCES transfers(transfer_id) ON DELETE CASCADE,
    account_id   TEXT NOT NULL REFERENCES characters(account_id) ON DELETE CASCADE,
    arrived_at   TIMESTAMPTZ,
    PRIMARY KEY (transfer_id, account_id)
);
CREATE INDEX transfer_players_pending ON transfer_players (account_id) WHERE arrived_at IS NULL;
