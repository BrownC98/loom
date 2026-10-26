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

- To change the agent of a running node, use `loom relaunch <target> --agent <id>` (any node); do not remove and spawn again. For new nodes, only the human changes the role table.
- A high-risk edit scope goes to the high-risk role. That alone needs no worker.
- The lead does one- or two-line changes and gives all other work (code, screen checks, research) to one coordinator in a new worktree. It keeps the work context: current work, children, decisions, waiting approvals. It reads only the coordinator's `done` and the gate's merged and failed results, not result files.
- The node that has the facts writes the brief, spawns the workers, reviews, merges and cleans up. A lead brief gives only the goal (with done-when) and the scope. Keep its `Facts:` short: put long facts in a task folder file and give the absolute path. Do not give the brief rules path; loom adds those rules.
- A coordinator does small, closely related work itself. It divides work only into two or more independent edit scopes. With one scope, it spawns the worker without `--worktree` in its own worktree. While the worker runs, the coordinator only reads and reviews there. It also takes pieces that wait for one product decision.
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

- Merge only with `loom ready`, `loom verdict` and `loom fail`. Do not merge or push to main by hand.
- The owner commits, does the checks and calls `loom ready` without a merge of the target. On a conflict, the queue sends rework: then merge the target and fix it.
- A parent's verdict on a branch made from its branch is acceptance against done-when, not a code review. A block lists what done-when misses. A coordinator merges its workers' results before its own `loom ready`.
- The gate gives a high-risk branch to main (storage, concurrency, delivery, auth, process lifecycle, module contracts) two reviewers, else one; if unsure, two. The queue waits for all live reviewers, so spawn all of them before the first verdict.
- After `merged`, the daemon removes the reviewers, the owner node, a clean worktree and the branch. It keeps a worktree with changes, its branch and owner, and tells the owner's parent. An owner without a parent or with children stays. So call `loom done` before `loom ready`.
- Each time a child reports, clean up what is safe and unused ([reference "Cleanup"](reference.md#cleanup)): shell nodes, unmerged worktrees and branches, background processes, temp folders. `loom done` refuses while a done, exited child stays (`--keep <name>=<reason>`).
- Rare cases (handover, setup failure, release runs, no gate script): [reference "Commands"](reference.md#commands).
- The completion report gives the result, evidence, location of the change, remaining work and the `Raptor-3:` line.
