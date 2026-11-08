ALTER TABLE nodes ADD COLUMN default_name INTEGER NOT NULL DEFAULT 0;
UPDATE nodes SET default_name = 1
WHERE parent IS NULL
  AND name = coalesce(json_extract(agent, '$'), 'shell') || '-'
      || CAST(CAST(substr(name, length(coalesce(json_extract(agent, '$'), 'shell')) + 2) AS INTEGER) AS TEXT);
