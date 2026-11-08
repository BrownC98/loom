UPDATE nodes SET default_name = 1
WHERE parent IS NULL
  AND default_name = 0
  AND name IN (
    SELECT base || sep || word
    FROM (
      SELECT replace(p, rtrim(p, replace(p, '/', '')), '') AS base
      FROM (SELECT rtrim(replace(r.path, '\', '/'), '/') AS p FROM repos r WHERE r.id = nodes.repo)
    ),
    (SELECT '-' AS sep UNION ALL SELECT ' '),
    (SELECT '총괄' AS word UNION ALL SELECT 'root' UNION ALL SELECT 'lead')
  );
