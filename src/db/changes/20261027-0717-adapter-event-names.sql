UPDATE events SET entry = json_set(json_remove(entry, '$.by.hook'), '$.by.step', entry ->> '$.by.hook')
    WHERE CASE WHEN json_valid(entry) THEN entry ->> 'kind' = 'idleEnded'
        AND entry ->> '$.by.kind' = 'turn' AND json_type(entry, '$.by.hook') IS NOT NULL END;
UPDATE events SET entry = json_set(json_remove(entry, '$.by.by.hook'), '$.by.by.step', entry ->> '$.by.by.hook')
    WHERE CASE WHEN json_valid(entry) THEN entry ->> 'kind' = 'agentStateChanged'
        AND entry ->> '$.by.kind' = 'waitClosed' AND entry ->> '$.by.by.kind' = 'turn'
        AND json_type(entry, '$.by.by.hook') IS NOT NULL END;
UPDATE events SET entry = json_set(entry, '$.by.kind', CASE entry ->> '$.by.kind'
        WHEN 'serverBusy' THEN 'busy' WHEN 'serverIdle' THEN 'idle' ELSE 'lost' END)
    WHERE CASE WHEN json_valid(entry) THEN entry ->> 'kind' = 'agentStateChanged'
        AND entry ->> '$.by.kind' IN ('serverBusy', 'serverIdle', 'serverLost') END;
