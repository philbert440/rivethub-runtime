CREATE TABLE IF NOT EXISTS rh_conversation_nodes (
    conversation_id TEXT PRIMARY KEY REFERENCES ros_conversations (id) ON DELETE CASCADE,
    node TEXT NOT NULL CHECK (node <> '')
);
