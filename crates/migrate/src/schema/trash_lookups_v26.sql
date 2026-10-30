CREATE INDEX IF NOT EXISTS rh_wiki_citations_summary ON ros_wiki_citations (summary_id);
CREATE INDEX IF NOT EXISTS rh_wiki_provenance_source ON ros_wiki_provenance (source_kind, source_id);
CREATE INDEX IF NOT EXISTS rh_wiki_provenance_conversation ON ros_wiki_provenance (conversation_id);
