UPDATE settings SET value = json_remove(
        json_set(value, '$.accounts', json_patch('{}', json_object(
            'claude', value ->> '$.claudeAccount',
            'codex', value ->> '$.codexAccount'))),
        '$.claudeAccount', '$.codexAccount')
    WHERE id = 1 AND json_valid(value)
    AND (json_type(value, '$.claudeAccount') IS NOT NULL
        OR json_type(value, '$.codexAccount') IS NOT NULL);
