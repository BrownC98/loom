ALTER TABLE nodes RENAME COLUMN server TO adapter_state;
UPDATE nodes SET wait = json_set(wait, '$.source', 'agent')
    WHERE wait ->> 'source' IN ('hook', 'server');
