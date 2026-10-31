CREATE TEMP TABLE rh_store_owner (owner TEXT NOT NULL);
INSERT INTO rh_store_owner (owner)
SELECT coalesce(
    (SELECT CASE WHEN count(DISTINCT owner_user_id) = 1 THEN min(owner_user_id) END
       FROM ros_conversations WHERE owner_user_id IS NOT NULL),
    'owner');
UPDATE ros_conversations SET owner_user_id = (SELECT owner FROM rh_store_owner)
 WHERE owner_user_id IS NULL;
UPDATE ros_messages
   SET owner_user_id = (SELECT c.owner_user_id FROM ros_conversations c WHERE c.id = ros_messages.conversation_id)
 WHERE owner_user_id IS NOT (SELECT c.owner_user_id FROM ros_conversations c WHERE c.id = ros_messages.conversation_id);
UPDATE rh_topics SET owner_user_id = (SELECT owner FROM rh_store_owner)
 WHERE kind <> 'type' AND owner_user_id IS NULL;
UPDATE rh_conversation_placements
   SET owner_user_id = coalesce(
       (SELECT c.owner_user_id FROM ros_conversations c WHERE c.id = rh_conversation_placements.conversation_id),
       (SELECT owner FROM rh_store_owner))
 WHERE owner_user_id IS NULL;
UPDATE rh_session_placements
   SET owner_user_id = coalesce(
       (SELECT c.owner_user_id FROM ros_conversations c WHERE c.session_key = rh_session_placements.session_id
         ORDER BY c.created_at LIMIT 1),
       (SELECT owner FROM rh_store_owner))
 WHERE owner_user_id IS NULL;
UPDATE rh_wiki_entity_candidates
   SET owner_user_id = coalesce(
       (SELECT c.owner_user_id FROM ros_summaries s JOIN ros_conversations c ON c.id = s.conversation_id
         WHERE s.id = rh_wiki_entity_candidates.summary_id),
       (SELECT owner FROM rh_store_owner))
 WHERE owner_user_id IS NULL;
UPDATE rh_wiki_deferred
   SET owner_user_id = coalesce(
       (SELECT c.owner_user_id FROM ros_summaries s JOIN ros_conversations c ON c.id = s.conversation_id
         WHERE s.id = rh_wiki_deferred.summary_id),
       (SELECT owner FROM rh_store_owner))
 WHERE owner_user_id IS NULL;
DROP TABLE rh_store_owner;
