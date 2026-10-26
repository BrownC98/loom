ALTER TABLE agent_states DROP COLUMN changed_ns;
DELETE FROM events WHERE entry ->> 'kind' = 'noIdleWait';
