UPDATE events SET entry = json_set(entry, '$.reason.value', json_object(
        'name', entry ->> '$.reason.value',
        'linked', json('["Claude","Codex","OpenCode 2","Pi","OMP","Cursor"]')))
    WHERE CASE WHEN json_valid(entry) THEN entry ->> 'kind' = 'refused'
        AND entry ->> '$.reason.code' = 'notSteerable' AND json_type(entry, '$.reason.value') = 'text' END;
UPDATE events SET entry = json_set(entry, '$.reason.value', json_object(
        'agent', entry ->> '$.reason.value',
        'linked', json('["claude","codex","opencode2","pi","omp","cursor"]')))
    WHERE CASE WHEN json_valid(entry) THEN entry ->> 'kind' = 'refused'
        AND entry ->> '$.reason.code' = 'noLink' AND json_type(entry, '$.reason.value') = 'text' END;
