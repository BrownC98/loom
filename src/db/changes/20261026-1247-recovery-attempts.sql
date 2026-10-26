CREATE TABLE recovery_attempts (
    node INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    cause TEXT NOT NULL,
    mail INTEGER,
    PRIMARY KEY (node, cause)
);
UPDATE agent_states SET state = state -> 'before' WHERE state ->> 'kind' = 'relaunching';
UPDATE agent_states SET state = '{"kind":"starting"}' WHERE state ->> 'kind' IN ('asleep', 'ended');
DELETE FROM events WHERE entry ->> 'kind' IN ('agentStateChanged', 'agentStateDisagreed')
    AND (entry LIKE '%"relaunching"%' OR entry LIKE '%"relaunchFailed"%');
