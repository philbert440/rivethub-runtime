ALTER TABLE rh_topics ADD COLUMN owner_user_id TEXT;
ALTER TABLE rh_session_placements ADD COLUMN owner_user_id TEXT;
ALTER TABLE rh_conversation_placements ADD COLUMN owner_user_id TEXT;
ALTER TABLE rh_wiki_entity_candidates ADD COLUMN owner_user_id TEXT;
ALTER TABLE rh_wiki_deferred ADD COLUMN owner_user_id TEXT;
