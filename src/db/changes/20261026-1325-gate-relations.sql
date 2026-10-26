ALTER TABLE gates DROP COLUMN gate;
ALTER TABLE gates ADD COLUMN merged_into TEXT;
UPDATE gates SET merged_into = target WHERE state = '"merged"';
ALTER TABLE gates DROP COLUMN target;
UPDATE gates SET state = '"working"', batch = NULL
WHERE state IN ('"ready"', '"passed"') AND owner NOT IN (SELECT id FROM nodes);
