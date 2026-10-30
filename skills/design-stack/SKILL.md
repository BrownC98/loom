---
name: design-stack
description: The one entry point for UI/UX work. It runs Hallmark (direction), Impeccable (build), web-design-guidelines (platform contract) and better-* (domain review) in a fixed order, without overlap. Use it when you make, redesign, review, audit or finish a web screen, page or component. Do not use it for backend-only work.
---

# design-stack

This skill is a router. It runs each design skill once, in a fixed order. The rules are in each skill; do not copy them here. This file sets only three things: the order, the priority and when to skip.

## Priority (always solve a conflict in this order)

1. The project contract: `DESIGN.md`, `tokens.css`, the design system, brand colors.
2. **web-design-guidelines**: the web platform contract (semantic HTML, forms, focus, touch, i18n).
3. **Impeccable craft floor**: bans and the quality floor.
4. **Hallmark** theme conventions: visual taste.
5. Other habits.

A lower item gives way to a higher item. Example: the Hallmark Grid voice of capital-letter labels loses to the Impeccable ban on all-caps. A brand color from the user wins over a Hallmark catalog accent.

**Language exception**: do not apply the English copy rules of web-design-guidelines (Title Case, active voice, `&`, placeholders that end in `…`) to a Korean UI or to the product's existing strings (`i18n/ko.ts` and others). The strings that the product already uses always win.

## Paths

Each skill that this file calls is in a folder next to this `SKILL.md`. Paths in this file start in this skill's folder (the folder that holds this file). Call it `<ds>`.

| Skill | Folder | License |
|---|---|---|
| web-design-guidelines | `<ds>/web-design-guidelines` | MIT, Vercel Labs |
| Impeccable | `<ds>/impeccable` | Apache-2.0, Paul Bakaus (`NOTICE.md`) |
| Hallmark | `<ds>/hallmark` | MIT, Hallmark contributors |
| better-interface, better-accessibility, better-colors, better-typography | `<ds>/better-*` | MIT, Jakub Krehel |
| design-md-library | `<ds>/design-md-library` | MIT, VoltAgent |
| emil-design-eng | `<ds>/emil-design-eng` | MIT, Emil Kowalski |

Read each skill's `SKILL.md` as a file. Its relative links start in its own folder.

Impeccable launcher: `sh <ds>/impeccable/scripts/impeccable <verb>`. The script does not have the execute bit, so call it with `sh` (with `node` it gives a SyntaxError). On a Windows shell without `sh`, call `<ds>/impeccable/scripts/impeccable.cmd <verb>`. On the first run the launcher downloads its engine into `~/.impeccable` (network necessary).

The web-design-guidelines rules come from a URL. If your runtime has no WebFetch tool (Cursor, Codex and others), get them with `curl -fsSL <URL>`.

## Routing: first select the type of work

| Work | 1 Direction | 2 Build | 3 Check |
|---|---|---|---|
| **A. Explainer page, diagram, poster** (one-time HTML) | Hallmark | Impeccable craft floor | detect |
| **B. Product UI** (app screen, dashboard, form, mockup) | Hallmark theme values only | Impeccable **Operate** leads | detect + WIG |
| **C. Review of an existing screen or a PR** | none | none | WIG + better-interface |
| **D. A specified brand tone** | design-md-library | Impeccable craft floor | detect + WIG |

## Steps

### 1. Direction (A, B, D; one time for each screen)

- If the project has `DESIGN.md` or a token file, **skip this step**. The existing system is the direction.
- If not, use Hallmark: the genre decision in `SKILL.md` → one `references/genres/<genre>.md` → select one item from the index `references/macrostructures.md` and read only that file → if a theme spec exists, `references/themes/<theme>.md`.
- For B (product UI), take only the **palette, type and materials** of the theme. Do not use poster devices (giant display type, plates, exposed grids, cut numbers).
- Say what you selected in one line: genre · macrostructure · theme · nav/footer.

### 2. Build

- Run `sh <ds>/impeccable/scripts/impeccable context --target <file>` one time.
- Select the mode: Persuade / **Operate** (product UI) / Read / Experience. B is always Operate. Read `<ds>/impeccable/reference/operate.md`.
- Immediately before you edit, read `<ds>/impeccable/reference/craft-floor.md`. For a command-type task, read only one more file, `reference/<command>.md`.
- If the work sets new colors, read `reference/colorize.md`. If the user gave a brand color, divide that value into roles (primary / hover / wash / ink). Do not scatter swatches.

### 3. Check (after you finish, each one time)

```bash
# Design anti-patterns: local, no network
sh <ds>/impeccable/scripts/impeccable detect --json <file>

# Web platform contract: get the rules from the URL each time
WebFetch https://raw.githubusercontent.com/vercel-labs/web-interface-guidelines/main/command.md
```

- A: detect only. Most WIG form and touch rules do not apply, so WIG finds little.
- B, C, D: detect and WIG. If they disagree, WIG wins (priority 2 > 3).
- Run `better-interface` **one more time** only when a person needs a ranked verdict. If only one area has a problem, call only `better-accessibility`, `better-colors` or `better-typography`.
- Examine the render in one batch: capture the desktop width and fix all results in one batch. Examine one more time at most, then stop.
- Do not make a separate mobile screen. Do no mobile layout, mockup, capture or check (2026-10-14 user instruction "앞으로 모바일 화면은 따로 만들지마"). Do not delete existing mobile behavior.

### 4. Make the tokens permanent (before the second screen)

Fix the tokens in `tokens.css` or `DESIGN.md`. After that, skip step 1. The Hallmark variety rule then goes the other way: screens share one system and do not differ from each other.

## Base screen (when you make many screens in one format)

When you make many screens or mockups in one format, the first screen is the base for all the others. Defects in the base screen copy to all the others. Thus, **do all of these items for the base screen, with no skip** (2026-09-30 user instruction: the first screen must obey industry UI/UX laws. On September 28 the first mockup read only design-stack and skipped steps 2 and 3. A later detect showed that a WCAG AA contrast failure remained).

1. Direction: step 1 above (skip it if product tokens exist).
2. Build: all of step 2 above (`impeccable context`, `reference/operate.md`, `reference/craft-floor.md`).
3. Check: `impeccable detect` + WIG + `better-interface` one time (ranked verdict) + the score table of the 10 Nielsen heuristics in Impeccable `reference/critique.md`. The skip rule "only when necessary" does not apply to the base screen.
4. Check record: write the detect count with each fix or false-positive reason, the WIG fixes, the heuristic score table and the top better-interface problems. Put the record at the end of the base screen file. For a mockup, put it in the status report, not in the mockup (o-mode "Mockup approval"). Write the record location in the completion report. Without a record, the screen is not approved as the base screen.
5. When a person approves, make it permanent as in step 4: put the format, layout and notation rules in the shared rules (the brief), and name the file as the base mockup.

Later screens follow **the base screen for style and the skills for principles** (2026-09-30 user instruction).
- Style (base screen): layout format, tokens, colors, type, component shapes, copy tone, notation rules. First read the shared rules and the base file, and use them as they are.
- Principles (skills): for each screen, read and obey `reference/craft-floor.md` and `reference/operate.md`. When you finish, run `impeccable detect` and WIG one time each, and write the results in one line. If a new interaction comes in (form, modal, keyboard flow), add the one related better-* skill (`better-accessibility` or another).
- If they conflict, the principle wins. The base screen style is not a product contract (priority 1), so it is below WIG and Impeccable. If the base screen itself breaks a principle, fix it in the later screen, and report that the base screen and the shared rules also need the fix.

## When to skip (to keep the work small)

- Read Hallmark `slop-test.md` only at the end. Do not read it early.
- Read only the index of Hallmark `component-cookbook.md`, then open only the archetype file that you selected. Do not read all of it.
- **Do not call better-layout or better-ui.** They are almost the same as the Impeccable craft floor. loom does not ship them.
- **Do not use frontend-design with Hallmark.** They are the same layer; use Hallmark. loom does not ship frontend-design.
- Use emil-design-eng only when animation is the core of the task.
- Do not run detect two or more times on one file. Collect the fixes and make them in one batch.
- Do not fix all review results. For a false positive (for example, the tool cannot read `var()` padding), write the reason in one line and keep it.

## Stop condition

Stop when detect and WIG find 0 real defects and two render checks or fewer find nothing to fix. More polish after that only costs.
