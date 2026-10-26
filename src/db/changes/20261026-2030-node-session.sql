ALTER TABLE nodes RENAME COLUMN transcript TO session;
UPDATE nodes SET session = CASE server ->> 'kind'
        WHEN 'codex' THEN server ->> 'thread'
        WHEN 'openCode2' THEN server ->> 'session'
    END
    WHERE session IS NULL AND server IS NOT NULL;
UPDATE nodes SET server = json_remove(server, '$.thread', '$.session') WHERE server IS NOT NULL;
