UPDATE nodes SET server = json_object(
        'pid', server ->> 'pid',
        'state', json_remove(server, '$.kind', '$.pid')
    )
    WHERE server ->> 'kind' IS NOT NULL;
UPDATE nodes SET wait = json_set(wait, '$.source', 'server')
    WHERE wait ->> 'source' IN ('codex', 'openCode2');
