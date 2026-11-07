---
name: user-doc
description: A mockup, report or workflow map for the human as one checked HTML file on an inbox question. Use for "목업", "보고서", "브라우저에 띄워", "워크플로우 분석", "워크플로우 설계", "정보 구조".
---

# Documents for the human

Mockups, reports and workflow maps use the same design language. Make, check and attach a document in one pass. Do not open the human's browser.

A document exists so that the human decides fast and correctly. The author does the study and recommends one option; the human only picks. Do not give the human more work to choose (more inputs, more choices, details to fill in) unless the human asks for it. For each decision, give the recommendation, what changes for the human (a "now → after" example), the cost, the risk and how to undo it. Put facts that do not change a decision after the decisions, or remove them.

## Types

Read the file of your type before you write. This file gives the rules for all types.

| Type | Rules | Use |
|---|---|---|
| Mockup | [mockup.md](mockup.md) | A screen to approve before screen code |
| Report | [report.md](report.md) | Research, design, comparison and result records |
| Workflow map | [workflow-map/rules.md](workflow-map/rules.md) | Workflow analysis and design: work layers, information units and their edges |

## Form

- One HTML file in the human's language. Put images in `data:` URLs. Do not point `src`, `href` or CSS `url()` to a different file (relative path, `file:`). `https:` URLs, for example web fonts, are permitted. Start from a copy of [style.html](style.html). Do not change the tokens, fonts, font sizes, head line, section numbers or table style in its `<style>`. Add only the blocks that you need. Exception: a workflow map starts from its own [template.html](workflow-map/template.html), with the style rules in [rules.md](workflow-map/rules.md).
- Light is the default (`<html style="color-scheme: light">`). Put a dark-mode toggle button (`.theme-toggle`) at the top right.
- The head line at the top gives `응답할 곳: <role> · <node name>` (the node that receives the human's answer). A mockup also gives `연 담당: <role> · <node name>` and its version (`v2 · <date>`). Run `loom status` just before you ask, to get the node name. If the name changes, change the head line.
- Document ID: `loom show` adds it at the start of the head line (`문서 ID: M3`) and before the `<title>`. M is for a mockup and R for a report or a workflow map. A path keeps its ID through all versions (table `~/.cache/user-doc/ids.tsv`). The author does not write the ID. When you name a document in chat, give its ID.
- Give each document a short, different `<title>`. People find open windows by it.
- Location: a mockup goes in the repository at `docs/mockups/<name>.html`; a report goes in the task folder (`$LOOM_TASK_DIR`, else `~/.cache/loom/<task>/<subject>.html`). A workflow map goes where the human says, else where a report goes. Write a new version to the same path and increase its version number.

## Decisions and IDs

- A workflow map has no decision boxes: ask each decision for the human as a separate `loom ask` question. The rules below apply to mockups and reports.
- Put each open decision in its own decision box. The box is amber, the color of the loom question notice. Explanation callouts stay red. Do not put a decision, an option or a recommendation in a callout or a note.
- Copy the markup and the `.decision` styles from [style.html](style.html):
  `<section class="decision" id="D1" data-for="2 3"><h3><span class="did">D1</span>question</h3><div class="opts"><div class="opt" data-recommended><span class="key">A</span><span class="label">option<span class="mark">권장</span><span class="sub">now → after · cost · risk · undo</span></span></div>…</div><ul class="why"><li>reason</li></ul></section>`.
  The option buttons and the recommended mark have the look of the inbox question options.
- Target: in a mockup, `data-for` gives the numbers of the boxes that the decision is about. [callouts.js](callouts/callouts.js) writes the line `대상: <section title> · 2, 3번` below the question and puts the decision ID next to the number of each of those callouts. Do not write that line yourself.
- Give two to four `.opt`. Exactly one has `data-recommended` and `<span class="mark">권장</span>`. Give at most three reasons in `.why`.
- IDs: decisions are `D1`, `D2`, …; `id` and `.did` have the same text. Callouts are numbers (see [mockup.md "Mockup callouts"](mockup.md#mockup-callouts)). No ID is used twice in a document. Name items by ID in the document, in chat and in questions (`D2`, `7번`).
- Check: `show.sh` fails a document with an option or recommendation outside a decision box (in a mockup: the text `권장`, `정할 것` or `Recommended` outside a box, the lead, the head line and the screen drawings), a decision without exactly one recommended option, a missing ID or an ID used twice. In a document with callouts, it also fails a decision without `data-for` or with a number that is not a box with a callout in a screen section (`.fig`).

## Display

1. The author (a coordinator or a worker) writes the file and checks the render. It sends its parent only the path, version and a one-line summary.
   - While you draw, check with `show.sh --quick <file>`. It checks only at 1440px. It does not call `loom show` and does not record a pass, so `loom ask --doc` refuses the file. Run `show.sh <file>` (all widths) once when the drawing is done.
2. The node that asks the human is the lead, or outside loom the agent that talks with the human. It reviews the content and runs [scripts/show.sh](scripts/show.sh): `bash <this skill folder>/scripts/show.sh <file>`.
   - `show.sh` calls `loom show <file>`. It checks the form (title, light default, toggle button, head line) and adds the document ID. If the check fails, send the document back to the author.
   - It keeps a render made with headless Chromium. The node that asks looks at that render.
   - On a computer without bash (a Windows node), call only `loom show <file>`.
3. Attach the document to the question: `loom ask "[<h1>](<file>) v3을 승인할까요?" --doc <file>`. The link text is the document's `<h1>`. A version or other words go after the link. loom drops `[` and `]` from the `<h1>` and refuses other link text. The link in the text becomes the document link. Without a link, loom adds "M3 <title>" at the end. When the human clicks it, the daemon opens it in the default browser. For a new version, write the same path, run `show.sh` again and ask again with `loom ask --doc`.
4. Outside loom (no inbox), give the human a link to the file.
5. In chat, write a summary in the human's language, a markdown link to the file and the open decisions. Do not copy the full tables into chat.
