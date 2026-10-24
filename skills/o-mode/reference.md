# o-mode reference

Procedures from [o-mode](SKILL.md) for rare tasks. Read this file when you do one of them.

## Commands

| Task | Command or action |
|---|---|
| Handover | In the new lead, `loom adopt <코디 name>`. Look at the relations and the kept messages |
| Send an action to all children | `loom note` to each live child. A 코디 sends the action on to its own children |
| Reassign after a failure | Make sure that the old node stopped, then spawn a node with a new name. In the brief, write the kept branch and results |

A `--worktree` child runs the repository setup script on the parent's committed `HEAD`. Thus commit a `HEAD` on which the setup (with the build) passes, then spawn. A `setup-failed` message has the fields `node` and `code`, and its body is the end of the output. On it, the parent finds the cause in that output, fixes it and commits. Then it removes the failed child with `loom rm`. It removes the worktree and branch with `git worktree remove` and `git branch -d`. Then it spawns the child again with the same name.

- The node that starts a release or APK run (`.github/workflows/release.yml`) watches it to the end.
- In a repository without `scripts/gate/check.sh`, still call `loom ready`. The gate merges by hand and reports with `loom done`.

## Mockup approval

- [user-doc](../user-doc/SKILL.md) sets the form, style and display of mockups and reports. This section gives only the Loom roles.
- For screen work, check with an installed design check skill (for example `design-stack`) if there is one. The 코디 makes the mockup, checks the render and reports only the path, the version and the check result.
- The lead checks the file with user-doc `show.sh` and attaches it with `loom ask "<question>" --doc <file>`. Nobody opens the human's browser. The head line names the lead as the node to answer to.
- Before the human approves, do not write screen product code and do not spawn implementation workers. Show the version to approve and the open decisions.
- Write the approved version, its path and its content in the implementation brief. Keep the node and the worktree while the approval waits.

## Blocked

- If the same completion command fails two times, or the environment blocks you over 10 minutes, tell your parent. Give the cause, the effect, what you tried and the decision that you need.
- The parent selects who fixes it and how. It tells the related children, with the number of the first message, and examines the result.

## Questions, answers and waits

The usage text gives the commands, the waits and the size limits. These rules are not in it:

- At start and after an `adopted` notice, run `loom status` to see your current parent and children. If a send fails, first look at the relations and `alive`. Report to your parent. Send steer and interrupt messages only between a current parent and its children. Steer also goes between the reviewers of one branch.
- A node number is not a message number. To ask a child something, send `loom note` and get a `loom done` or an answer. If an answer is refused because the parent changed, send the current parent the question's history and its result.
- A question stays open after its time limit. Do not send the same question again automatically. Look at the relations and `alive`, then follow the supervision procedure.
- `done` means that the work is complete, not that the process stopped. A done node with `alive: true` can receive feedback, and then it is running again. Spawn a node with `alive: false` again with a new name.
- `done --file` sends the absolute path, not the file content. A result file has no size limit.

## Reports to the human

- Make an HTML report in the user-doc form only when the human asks for it. The lead reviews it, checks it with `show.sh` and attaches it with `loom ask --doc` (do not open the human's browser). In chat and `loom done`, write only a summary and the link.
- Text that agents read is English markdown.

## Cleanup

- Do not clean up unsaved changes or work that waits for a check, approval or merge. Also keep user nodes in use and the base checkout.
- Before cleanup, run `git status --short` in that worktree. Look for uncommitted changes and untracked files. If review evidence is only in that folder, save it first. Do not clean up a worktree that a different task uses.
- Remove a node with `loom rm <child name or number>`. You can remove only your own children. The process of that child stops too.
- If the removed child had children, they become your children. Save their results, then remove them with `loom rm` too.
- Remove an unmerged worktree from the base checkout with `git worktree remove <path>` after you remove its node. The daemon deletes merged branches. Do not force a deletion.
- In the completion report, give each worktree that must stay: its path, reason, owner and cleanup condition. "Done" alone is not a reason to leave it.
- Before the completion report, make sure of the cleanup with `git worktree list`, `git branch` and, if possible, `loom status`. A delete command alone does not show that the cleanup is complete.
