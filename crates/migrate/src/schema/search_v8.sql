CREATE VIRTUAL TABLE rh_messages_trigram USING fts5(id UNINDEXED, content, tool_result, tokenize = 'trigram');
INSERT INTO rh_messages_trigram(id, content, tool_result) SELECT id, content, tool_result FROM ros_messages;
CREATE TRIGGER rh_messages_trigram_insert AFTER INSERT ON ros_messages BEGIN
    INSERT INTO rh_messages_trigram(id, content, tool_result) VALUES (new.id, new.content, new.tool_result);
END;
CREATE TRIGGER rh_messages_trigram_delete AFTER DELETE ON ros_messages BEGIN
    DELETE FROM rh_messages_trigram WHERE id = old.id;
END;
CREATE TRIGGER rh_messages_trigram_update AFTER UPDATE OF content, tool_result ON ros_messages BEGIN
    DELETE FROM rh_messages_trigram WHERE id = old.id;
    INSERT INTO rh_messages_trigram(id, content, tool_result) VALUES (new.id, new.content, new.tool_result);
END;
CREATE VIRTUAL TABLE rh_summaries_trigram USING fts5(id UNINDEXED, content, tokenize = 'trigram');
INSERT INTO rh_summaries_trigram(id, content) SELECT id, content FROM ros_summaries;
CREATE TRIGGER rh_summaries_trigram_insert AFTER INSERT ON ros_summaries BEGIN
    INSERT INTO rh_summaries_trigram(id, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER rh_summaries_trigram_delete AFTER DELETE ON ros_summaries BEGIN
    DELETE FROM rh_summaries_trigram WHERE id = old.id;
END;
CREATE TRIGGER rh_summaries_trigram_update AFTER UPDATE OF content ON ros_summaries BEGIN
    DELETE FROM rh_summaries_trigram WHERE id = old.id;
    INSERT INTO rh_summaries_trigram(id, content) VALUES (new.id, new.content);
END;
