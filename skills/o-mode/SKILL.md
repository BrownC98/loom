---
name: o-mode
description: The Loom version of o-mode (orchestration mode). A procedure to develop repository work in parallel on Loom nodes. It covers how the lead, coordinators and workers assign, communicate, review, merge and clean up with Loom and Git. Use it in sessions that have LOOM_TOKEN and LOOM_URL.
---

# Loom parallel development

## Limits

- This file, the [brief rules](agent-rules.md) and the [reference](reference.md) are the only source of this procedure. Read a reference section when you do its task. Do not use a different orchestration skill.
- A node without a parent is the lead. A child keeps the role from its brief.
- Assign, communicate and supervise only with Loom. Do not use Orca (CLI, app, skills, scripts), also when Loom lacks a feature.
- A request to benchmark a different product is a study: read the given material and local source first. Operate that product only when the human says so.
- At start, run `loom status` and read the repository AGENTS.md. In a brief, write the setup steps for a new worktree and how to check completion.

## Roles and assignment

Lead, coordinator, worker and reviewer are jobs. Pick a role for each job from `loom roles` by its duty; the human renames roles.

- To change the agent of a running node, use `loom relaunch <target> --agent <id>` (any node); do not remove and spawn again. Do not send `--interrupt` after it: the switch stops the old work, and a later interrupt stops the new session. For new nodes, only the human changes the role table. Give `loom spawn` the options `--agent`, `--model` or `--effort` only when the human or your brief names that agent, model or effort.
- The lead does one- or two-line changes and gives all other work (code, screen checks, research) to one coordinator in a new worktree. It keeps the work context: current work, children, decisions, waiting approvals. It reads only the coordinator's `done` and the gate's merged and failed results, not result files.
- The node that has the facts writes the brief, spawns the workers, reviews, merges and cleans up. A lead brief gives only the goal (with done-when) and the scope. Keep its facts short: put long facts in a task folder file and give the absolute path. Do not give the brief rules path; loom adds those rules.
- A coordinator with one edit scope implements it itself and spawns no worker. Only a high-risk edit scope (storage format, auth, concurrency, data-loss paths, contracts between modules) goes to the high-risk role, without `--worktree` in the coordinator's worktree. While it runs there, the coordinator only reads and reviews. The merge gate reviewers review the branch.
- A coordinator gives work that edits no files (research, measurements, proofs, reviews of evidence) to several workers at once, one question each, without `--worktree`. It splits edits into two or more workers when their files do not overlap; one worker owns each shared file, and each gets `--worktree`. A high-risk scope goes to the high-risk role. The coordinator also takes pieces that wait for one product decision.
- Write unique names in the human's language. A branch has the name of its work (`알림표시통일`), a node `<role> <work>`, the lead `<repository> <role>`.
- A `--worktree` child starts from its parent's `HEAD`. Do not put two editors in one worktree.
- For screen work, read [reference "Mockup approval"](reference.md#mockup-approval) first. Before approval, write no screen product code and spawn no implementation workers.
- Make an HTML report only when the human asks ([reference](reference.md#reports-to-the-human)). Else write problem, cause and fix, one line each, per item. If the next step is clear, do it.

## Supervision and review

- Do not poll. Loom tells you when a child is done, exits or is removed.
- After a child's time budget, send `loom note <child> "Report status and blockers." --steer`; if no answer, run `loom status`.
- To stop a child, send `--interrupt` and ask for its result, failure cause and remaining work. Before you reassign, make sure it stopped.
- If the same completion command fails twice, or the environment blocks you over 10 minutes, follow [reference "Blocked"](reference.md#blocked).
- A reviewer compares the diff, done-when, check output and the real behavior. Check again if evidence is missing, a check failed or the code changed since. Do not weaken a check or done-when. Send back each problem that can be fixed now.

## Merge and cleanup

- Merge only with `loom ready` and `loom verdict`. Do not merge or push to main by hand.
- The owner commits, does the checks, keeps their output in `logs/` of its task folder for the reviewers and calls `loom ready` without a merge of the target. On a conflict, the queue sends rework: then merge the target and fix it.
- If the brief says the change must not deploy, call `loom ready <branch> --no-deploy`. The queue merges it in a batch of its own with `[skip ci]` in the merge commits, so the remote CI and its deploy do not run. The flag acts only on a merge into the original checkout. A coordinator that merges such workers gives the flag on its own `loom ready`.
- To change a branch before the queue takes it, commit and call `loom ready` again. The new round replaces the open round, and its reviewers get it.
- A verdict names the head that the reviewer judged: `loom verdict <branch> <head> pass|block`. Copy the head from the round message that loom sends. loom refuses a verdict for another head.
- A parent's verdict on a branch made from its branch is acceptance against done-when, not a code review. A block lists what done-when misses. A coordinator merges its workers' results before its own `loom ready`.
- On `loom ready` of a branch to main, loom spawns one reviewer per reviewer role as children of the gate, and gives them the review rules. Do nothing for reviewers. The queue waits for all live reviewers. To add a reviewer of another role, use `loom spawn --role <role> --review <branch>`.
- After `merged`, the daemon removes the reviewers, the owner's finished children, the owner node, a clean worktree and the branch. It keeps a worktree with changes, its branch and owner, and tells the owner's parent. An owner without a parent or with a child at work stays. So call `loom done` before `loom ready`.
- loom keeps your finished workers until your branch is merged. If the gate reviewers block your branch, send each finding in a worker's edit scope to that worker with `loom note <worker>`. A worker in your worktree fixes it, commits and calls `loom done`. A worker with its own worktree commits the fix on its branch and calls `loom ready <its branch>`; pass it and wait for `merged`. Then call `loom ready` again. Fix the other findings yourself. A worker whose branch merged into yours calls `loom done` again after `merged`.
- Each time a child reports, clean up what is safe and unused ([reference "Cleanup"](reference.md#cleanup)): shell nodes, unmerged worktrees and branches, background processes, temp folders.
- loom removes a done child when the child's turn or process ended and your turn ended or you called `loom done`. Thus a child that you do not answer goes when you end your turn. To send rework, send it in the same turn: a message to the child keeps it. A child also stays while it has a child at work, unread mail either way, an open gate round, or a worktree with changes or unmerged commits. It also stays while your own worktree has changes or commits that its target does not have, until your branch is merged. This hold ends when you are `done`, your turn ended and you have no open gate round. loom also removes a clean worktree of that child whose branch has no unmerged commits, and that branch. It keeps the task folders. The `loom done` reply tells you each kept child and why. Save the results that you need before you end the turn. To keep a done child for more work, add `--keep <name>=<reason>` to `loom done`; a message to that child ends the keep.
- Rare cases (handover, setup failure, release runs): [reference "Commands"](reference.md#commands).
- The completion report gives the result, evidence, location of the change, remaining work and the `Raptor-3:` line.
