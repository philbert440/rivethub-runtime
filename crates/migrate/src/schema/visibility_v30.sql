CREATE TABLE IF NOT EXISTS rh_spaces (
    id TEXT PRIMARY KEY CHECK (id <> '' AND id <> 'private'),
    kind TEXT NOT NULL CHECK (kind IN ('household', 'team', 'compartment')),
    name TEXT NOT NULL CHECK (name <> '')
);
CREATE TABLE IF NOT EXISTS rh_space_members (
    space_id TEXT NOT NULL REFERENCES rh_spaces (id) ON DELETE CASCADE,
    user_id TEXT NOT NULL CHECK (user_id <> ''),
    role TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('member', 'reader', 'admin')),
    PRIMARY KEY (space_id, user_id)
);
CREATE TABLE IF NOT EXISTS rh_user_settings (
    user_id TEXT PRIMARY KEY CHECK (user_id <> ''),
    default_visibility TEXT NOT NULL DEFAULT 'private',
    default_since TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS rh_session_marks (
    session_key TEXT NOT NULL CHECK (session_key <> ''),
    owner_user_id TEXT NOT NULL,
    visibility TEXT NOT NULL,
    marked_at TEXT NOT NULL,
    PRIMARY KEY (session_key, owner_user_id)
);
ALTER TABLE rh_topics ADD COLUMN visibility TEXT NOT NULL DEFAULT 'private';
ALTER TABLE rh_topics ADD COLUMN visibility_mark TEXT;
