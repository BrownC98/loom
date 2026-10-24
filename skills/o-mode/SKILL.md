---
name: o-mode
description: The Loom version of o-mode (orchestration mode). A procedure to develop repository work in parallel on Loom nodes. It covers how the lead (총괄), coordinators (코디) and workers (워커) assign, communicate, review, merge and clean up with Loom and Git. Use it in sessions that have LOOM_TOKEN and LOOM_URL.
---

# Loom parallel development

## Limits

- This file, the [brief rules](agent-rules.md) and the [reference](reference.md) are the only source of this procedure. Read a reference section when you do its task. Do not use a different orchestration skill.
- A node without a parent is the lead. A child keeps the role from its brief.
- Assign, communicate and supervise only with Loom, not with Orca (CLI, app, skills, scripts), also when Loom lacks a feature.
- A request to benchmark a different product is a comparison study. Read the given material and local source first. Operate that product only when the human tells you to.
- At start, run `loom status` and read the repository AGENTS.md. In a brief, write the setup steps for a new worktree and how to check completion.

## Roles and assignment

The default roles are 총괄, 코디, 워커, 고위험 워커, 검토자 and 보조 검토자. `loom roles` shows each role's responsibility, agent, model and effort.

- To change the agent of a running node, use `loom relaunch <child> --agent <id>`; do not remove and spawn again. For new nodes, only the human changes the role table.
- Spawn a 고위험 워커 only for the edits that its responsibility names; that alone does not require a worker.
- Each branch to main gets one 검토자. Only a high-risk branch also gets a 보조 검토자.
- The lead does one- or two-line changes and gives all other work (code, screen checks, research) to one 코디. It keeps the work context (current work, children, decisions, waiting approvals), receives only the 코디's `done` and the gate's merged and failed results, and does not read result files.
- The node that has the facts writes the brief, spawns the workers, reviews, merges and cleans up. A lead brief gives only the goal (with done-when) and the scope. Keep its `Facts:` short: put long facts in a task folder file and give the absolute path.
- A 코디 does small, closely related work itself. It divides work only into two or more independent edit scopes. With one scope, it spawns the worker without `--worktree` in its own worktree and then only reads and reviews there. It also takes the pieces that wait for the same product decision.
- Write names in the human's language: a branch has the name of its work (`알림표시통일`), a node is `<role> <work>` (`코디 알림표시통일`), the lead is `<repository> 총괄`. Names are unique.
- A `--worktree` child starts from its parent's `HEAD`. Do not put two editors in one worktree.
- For screen work, read [reference "Mockup approval"](reference.md#mockup-approval) first. Before the human approves, do not write screen product code or spawn implementation workers.
- Make an HTML report only when the human asks ([reference](reference.md#reports-to-the-human)). Else write problem, cause and fix, one line each, per item. If the next step is clear, do it.

## Supervision and review

- Do not poll: notices tell you when a child is done, exits, is removed or is handed over.
- After a child's time budget, send `loom note <child> "Report status and blockers." --steer`; if it does not answer, run `loom status`.
- To stop a child, send `--interrupt` and ask for its result, failure cause and remaining work. Before you reassign, make sure that it stopped.
- If the same completion command fails two times, or the environment blocks you for over 10 minutes, follow [reference "Blocked"](reference.md#blocked).
- A reviewer compares the diff, done-when, the check output and the real behavior. Check again if evidence is missing, a check failed or the code changed since. Do not weaken a check or done-when. Send back each problem that can be fixed now.

## Merge and cleanup

- Merge only with `loom ready`, `loom verdict` and `loom fail`. Do not merge or push to main by hand.
- The owner commits, does the checks and calls `loom ready` without a merge of the target. On a conflict, the queue sends rework: then merge the target and fix it.
- A 코디's verdict on a worker branch is acceptance against done-when, not a code review. A block lists what done-when misses. A 코디 merges its workers' results before its own `loom ready`.
- The queue waits for all live reviewers, so spawn the 보조 검토자 before the first verdict. Do other work while the queue runs.
- After `merged`, the daemon removes the reviewers, the owner node, a clean worktree and the branch. It keeps a worktree with changes, its branch and owner, and tells the owner's parent. An owner without a parent or with children stays. Thus call `loom done` before `loom ready`.
- Each time that a child reports, the parent cleans up what is safe and no longer needed: unmerged children, shell nodes, their worktrees and branches, background processes, temporary folders ([reference "Cleanup"](reference.md#cleanup)).
- Rare cases (handover, setup failure, release runs, no gate script): [reference "Commands"](reference.md#commands).
- In the completion report, write the result, the evidence, where the change is, the remaining work and the `Raptor-3:` line.
