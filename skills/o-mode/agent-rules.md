# Loom brief rules

Loom 코디 and 워커 nodes follow [o-mode](SKILL.md). The brief gives the role and the scope. The human's instructions come first.

## Start

- Read the repository AGENTS.md and the brief. Keep facts that you examined apart from hypotheses.
- For roles, communication, mockup approval and cleanup, use the procedures in this skill. Do not use rules from a different orchestration skill.
- Before you edit, make the raptor-3 parts list. The final result is a three-line summary and the path of the result file: `result.md` in the task folder, or `<your node name>.md` if you do part of your parent's task.

## Work and contact

- A worker implements and examines its piece to the end. A 코디 designs, assigns and reviews.
- Read-only research into a cause can go outside your edit scope. You select the implementation method in the scope and the contract.
- Ask your parent before you enlarge the edit scope or change done-when, product behavior or a shared contract. Continue the work that the question does not affect.
- Follow [reference "Questions, answers and waits"](reference.md#questions-answers-and-waits). Write reports in English and text for the human in the human's language.
- Apply a steer message to your current work. On an interrupt message, stop the current work and do what it tells.
- loom sends the human's terminal instructions to your parent as `kind: human-input`; do not report them again. If one changes the scope or a contract, wait for your parent's answer before you edit. A 코디 sends such a child `human-input` on to its parent with `loom note parent`. If loom cannot see your terminal input, send `loom note parent "<human's words> → <what you will do>"`.
- Do not use `git stash`. All worktrees share it. Keep existing changes.
- Examine the real behavior and the done-when conditions. Do not make a test or an assertion weaker to pass.
- Do not accept review findings without examination.
  - The branch owner accepts each finding or rejects it with evidence: `path:line`, a test that fails or passes, or steps that show the problem.
  - The gate (the owner) carries rejections and answers between the owner and the reviewers. A worker branch under a 코디 has no reviewer: the 코디 accepts it against the done-when conditions. The lead does not review or argue.
  - A reviewer answers each rejection with evidence. Evidence decides, not authority.
  - When there are two reviewers, each one also examines the other's findings and answers a disagreement with evidence.
  - A block is temporary until the argument ends. It becomes final when the owner accepts it or the gate decides.
  - If a dispute remains after two rounds, the gate decides and writes the reason in the verdict's findings file.
  - A reviewer that receives `kind: withdrawn` stops that review.

## Completion

- Look at `git status --short` and the diff. Do all checks that the repository requires.
- The report gives the cause, changed files, check commands and results, evidence paths, branch and commit SHA. If something is not committed, say so.
- Keep remaining risks apart from candidates outside the scope. Fix each defect that you can fix now. Add the `Raptor-3:` line.
- Write an HTML report only if your parent says that the human asked for one. Use the form in [reference "Reports to the human"](reference.md#reports-to-the-human). Write long results for agents to a file.
- A branch owner (a 코디, or a worker without one) follows [o-mode "Merge and cleanup"](SKILL.md#merge-and-cleanup) for `loom ready` and rework. A node that spawns workers follows [o-mode "Roles and assignment"](SKILL.md#roles-and-assignment).
- Report feedback and rework after completion with the procedure above. Do not delete your worktree before your parent examines the result.
- Before `loom done`, stop or delete the test daemons, background processes and temporary folders that you started. Keep only evidence files. A node's PTY host (`loom pty-host`) stays alive after its daemon stops. Thus stop all loom processes that have your test `HOME` in their environment, also the installed `loom`. Clean up your children as [reference "Cleanup"](reference.md#cleanup) tells.
- In the result, write your worktree path and if follow-up work is necessary.
