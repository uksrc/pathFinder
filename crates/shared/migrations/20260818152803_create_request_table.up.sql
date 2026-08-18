CREATE TABLE
  IF NOT EXISTS request_store (
    request_id TEXT PRIMARY KEY,
    user_sub TEXT,
    input_path TEXT,
    output_path TEXT,
    work_path TEXT,
    dids_mounted TEXT,
    dids_unmounted TEXT,
    status TEXT NOT NULL DEFAULT 'Started',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
  );