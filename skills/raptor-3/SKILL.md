---
name: raptor-3
description: 랩터-3 (raptor-3). Keep the number of parts in a code base to a minimum ("the best part is no part"). Use Musk's five design steps (question the requirement → delete → simplify → accelerate → automate) to find parts without a requirement, reinvented code, duplicate state, slow loops and repeated manual steps, and fix them in that order. Use it when the human asks for "랩터-3", "raptor-3", "바퀴의 재발명", "부품 최소화", "상태 통일", "단순화 점검" or "사이클 타임 단축". It applies to all languages, projects and agents.
---

# Raptor-3

Remove parts from all work: code, documents, scripts, CI, deployment and instruction files.

## Parts

A part is a thing that we must maintain. Examples: our code, a file, an export, an option, a script, a service, a copy of a fact.

- Make the number of parts that the goal needs as small as possible. First, cut the code that agents write.
- For the same result, select in this order: delete; assemble existing parts or libraries; write new code.
- The code in an external library is not our part. Our parts are the dependency line and the calls. A widely used, maintained library moves maintenance out of the repository. Thus a change from our own code to a proven library removes parts.
- Do not replace a library that we use with our own code. That adds parts. If a library must go (no maintenance, a security defect, a license problem), first find a different proven library.

## Five steps

These are Musk's five design steps. This order is the only source for the order of findings and of work.

- Do not apply a step to a part before the earlier steps are complete for it. Do not merge, accelerate or automate a part that you will delete.
- Put each finding in one step only. If it fits two steps, put it in the earlier step.
- Each "Find" item is an audit unit. An audit examines only the units that the request names.

1. **Question the requirement.** Each part must connect to a current user, a caller or a requirement. That requirement must have an owner (a person, an issue, a document).
2. **Delete.** Delete each part that has no requirement or no owner. Do not optimize a part that you will delete. If you never had to add back a part, you did not delete enough.
   - Find `parts`: features, screens and APIs that nothing calls. Dead code, exports and translations. Dependencies that a tool such as `knip` reports as unused. Options that always have the same value. Interfaces with one implementation. Wrappers that only pass calls through. Also examine items outside the code: scripts, CI steps, services, environment variables and documents that nobody uses.
3. **Simplify.** Merge only the parts that remain. Replace our own code with a standard feature, an existing part or a proven library.
   - Find `reinvent`: the same work in two implementations. This includes duplicates in the repository. It also includes our own code for work that a library already does. That library can be standard, the platform, installed or widely used. For each replacement, write the package name, version, scope, an assembly sketch and the behavior differences to keep.
   - Find `state`: the same fact in two places. Examples: copies between a client store, a context, storage and the URL. Copies of a server response. Enums and status strings that each layer declares again. Code that keeps two states the same. Give the single source. If a finding is also `reinvent`, put it in `reinvent`.
4. **Accelerate.** Measure the cycle time of each process that remains. Processes are tests, build, CI, merge, deployment and the main screen tasks. Reduce the slowest bottleneck first. Do not write that something is fast without a measurement.
5. **Automate.** Automate only the steps that a person repeats. Use existing tools and scripts first. A new automation is a part, so it must pass step 1.
   - Find `speed` (steps 4 and 5): slow tests, builds, CI jobs and waits. Independent steps that run one after another. Manual commands and checks that people repeat. For each finding, write the measurement (command, number of samples, median, maximum) and the target. If the `ci-bottleneck` skill is available, measure CI and tests with its procedure.

## While you work

Do steps 1 to 3 (question the requirement, delete, simplify) at these three times. Do not keep them for an audit at the end.

1. Before you write code, make a parts list. Write each new part with its requirement in one line. Do not make a part without a requirement. Also write the existing parts and libraries that do the same work. Write the parts that this change lets you delete.
2. Before you add a part that is not on the list, do steps 1 to 3 for it. Then add it to the list.
3. Before you commit, read `git diff --cached` again. Delete new parts without a requirement and old parts that the change made unused.

The completion report has one `Raptor-3:` line. It gives the deleted parts, reused parts and libraries, why new code was necessary, and changes from the first list.

## Screen (UI/UX) audit

When the human asks for a raptor-3 UI/UX improvement, also do this. A slice is one screen feature (for example: sign-up, a list, a viewer, settings, layout).

For each audit unit, also find these on screens:

| Unit | Find on screens |
|---|---|
| `parts` | many entry points to the same action, options that almost always have the same value, confirmation dialogs for actions that you can undo, text that the screen does not need, translation keys that nothing uses |
| `reinvent` | our own tables, popovers and menus that the UI library has; focus and keyboard handling that the browser already does |
| `state` | one state with different text or colors on different screens; colors written outside the tokens |
| `speed` | the number of clicks and the wait (loading, spinners) for the main tasks; input and selections that the user repeats by hand each time |

1. Collect evidence.
   - On the preview server, capture the screens at desktop width. Do not examine mobile screens.
   - For a screen, do the review procedure of the built-in skill `design-stack` for existing screens one time.
   - Use the captures again as the "current" screens in the mockup.
2. Set the requirements. The mockup that the user approved is the requirement for a screen.
   - Do not list an element of the approved mockup as a candidate for deletion.
   - Record each difference between a screen and its mockup as a defect.
3. Decide for each group of changes.
   - If the visible behavior does not change, implement it and attach only before and after captures. Examples: a standard part with the same look, a single source of state, deleted unused translation keys. An accessibility fix that keeps the layout is an example too.
   - If the visible behavior changes, follow [o-mode "Mockup approval"](../o-mode/reference.md#mockup-approval). Examples: deletion of a screen element, merged status displays, a layout change.
   - Get approval for each deletion of a screen element through the mockup. The caller of a button is the user, so code alone cannot show that the button is unused.
   - While you wait for an approval, work on the other groups.
4. Attach desktop before and after captures to each MR.
5. If a design check skill is available, use its stop conditions. For each screen, report the decrease in controls and the change in clicks for the main tasks.
