DROP TRIGGER worktree_spot_of_parent;
CREATE TRIGGER worktree_spot_of_parent AFTER UPDATE OF parent ON nodes
WHEN OLD.parent IS NOT NEW.parent AND (
    (OLD.parent IS NOT NULL AND (SELECT worktree FROM nodes WHERE id = OLD.parent) IS NOT NEW.worktree)
    OR (NEW.parent IS NOT NULL AND (SELECT worktree FROM nodes WHERE id = NEW.parent) IS NOT NEW.worktree)
) BEGIN
    DELETE FROM spots WHERE spot ->> 'kind' = 'worktree' AND spot ->> 'repo' = NEW.repo AND spot ->> 'path' = NEW.worktree;
END;
