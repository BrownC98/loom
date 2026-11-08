DELETE FROM events WHERE entry ->> 'kind' = 'agentStateDisagreed';
UPDATE events SET entry = json_object(
    'kind', 'processChanged',
    'node', entry -> 'node',
    'from', CASE
        WHEN entry ->> '$.from.kind' IS NULL THEN json('{"kind":"stopped","code":null}')
        WHEN entry ->> '$.from.kind' IN ('listening', 'deaf') THEN json('{"kind":"idle"}')
        WHEN entry ->> '$.from.kind' = 'blocked' THEN json_object('kind', 'blocked', 'before',
            CASE WHEN entry ->> '$.from.before.kind' IN ('listening', 'deaf') THEN json('{"kind":"idle"}')
            ELSE json('{"kind":"working"}') END)
        ELSE json('{"kind":"working"}') END,
    'to', CASE
        WHEN entry ->> '$.to.kind' IS NULL AND entry ->> '$.by.kind' = 'slept' THEN json('{"kind":"asleep"}')
        WHEN entry ->> '$.to.kind' IS NULL THEN json('{"kind":"stopped","code":null}')
        WHEN entry ->> '$.to.kind' IN ('listening', 'deaf') THEN json('{"kind":"idle"}')
        WHEN entry ->> '$.to.kind' = 'blocked' THEN json_object('kind', 'blocked', 'before',
            CASE WHEN entry ->> '$.to.before.kind' IN ('listening', 'deaf') THEN json('{"kind":"idle"}')
            ELSE json('{"kind":"working"}') END)
        ELSE json('{"kind":"working"}') END,
    'by', entry -> 'by',
    'detail', coalesce(entry ->> '$.to.kind', 'ended'))
WHERE entry ->> 'kind' = 'agentStateChanged';
UPDATE events SET entry = json_object(
    'kind', 'processChanged',
    'node', entry -> 'node',
    'from', CASE WHEN entry ->> '$.from.kind' = 'exited'
        THEN json_object('kind', 'stopped', 'code', entry -> '$.from.code') ELSE json('{"kind":"working"}') END,
    'to', CASE WHEN entry ->> '$.to.kind' = 'exited'
        THEN json_object('kind', 'stopped', 'code', entry -> '$.to.code') ELSE json('{"kind":"working"}') END,
    'by', CASE WHEN entry ->> '$.from.kind' = 'exited'
        THEN json('{"kind":"relaunched"}') ELSE json('{"kind":"exited"}') END,
    'detail', CASE WHEN entry ->> '$.from.kind' = 'exited' THEN 'starting' ELSE 'ended' END)
WHERE entry ->> 'kind' = 'state'
    AND (entry ->> '$.from.kind' = 'exited' OR entry ->> '$.to.kind' = 'exited')
    AND coalesce(nullif(entry ->> '$.from.kind', 'exited'), 'running') = coalesce(nullif(entry ->> '$.to.kind', 'exited'), 'running');
UPDATE events SET entry = json_set(entry, '$.from', json('{"kind":"running"}'))
WHERE entry ->> 'kind' = 'state' AND entry ->> '$.from.kind' = 'exited';
UPDATE events SET entry = json_set(entry, '$.to', json('{"kind":"running"}'))
WHERE entry ->> 'kind' = 'state' AND entry ->> '$.to.kind' = 'exited';
