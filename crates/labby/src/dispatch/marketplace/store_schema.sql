CREATE TABLE IF NOT EXISTS registry_servers (
    server_name          TEXT NOT NULL,
    version              TEXT NOT NULL,
    is_latest            INTEGER NOT NULL DEFAULT 0,
    status               TEXT NOT NULL DEFAULT 'active',
    server_json          TEXT NOT NULL,
    response_meta_json   TEXT,
    upstream_updated_at  TEXT,
    synced_at            TEXT NOT NULL,
    PRIMARY KEY (server_name, version)
);

-- Compound index for cursor pagination: ORDER BY server_name, version
CREATE INDEX IF NOT EXISTS idx_registry_servers_cursor
    ON registry_servers(server_name, version);

CREATE INDEX IF NOT EXISTS idx_registry_servers_status
    ON registry_servers(status);

CREATE INDEX IF NOT EXISTS idx_registry_servers_updated
    ON registry_servers(upstream_updated_at);

CREATE TABLE IF NOT EXISTS registry_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS registry_server_meta (
    server_name TEXT NOT NULL,
    version     TEXT NOT NULL,
    namespace   TEXT NOT NULL,
    meta_json   TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    updated_by  TEXT,
    PRIMARY KEY (server_name, version, namespace)
);

CREATE INDEX IF NOT EXISTS idx_registry_server_meta_lookup
    ON registry_server_meta(server_name, version, namespace);
