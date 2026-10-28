---
name: user-doc
description: Make a document for the human (a mockup or a report) as one HTML file, check it against the set form and style, and attach it as a link to an inbox question. Use it to show the human a mockup or a research, design, comparison or result report, when the human asks for "목업", "보고서" or "브라우저에 띄워", and for mockup approvals and reports in o-mode. Do not use it for briefs or notes that only agents read.
---

# Documents for the human

Mockups and reports use the same design language. Make, check and attach a document in one pass. Do not open the human's browser.

A document exists so that the human decides fast and correctly. The author does the study and recommends one option; the human only picks. Do not give the human more work to choose (more inputs, more choices, details to fill in) unless the human asks for it. For each decision, give the recommendation, what changes for the human (a "now → after" example), the cost, the risk and how to undo it. Put facts that do not change a decision after the decisions, or remove them.

In a report:

- The first section answers the human's question in one claim and gives the number of open decisions (`지금 정할 것: 1개`).
- Start each heading with its conclusion, not only a topic: `새 요청은 지금처럼 총괄이 받는다 · 권장`, not `새 요청은 어디로 가나`.
- Give each decision section a status: `정할 것` (open) or `반영함` (done, the human only checks it). Put open decisions first. In each group, put decisions that cannot be undone first. A done item gives what was done, what changes for the human and how to undo it, in three lines or fewer.
- For each open decision, give the recommendation and its reasons before the options. Give at most three reasons, each a full sentence with evidence that the human can trace (a measurement, a human instruction, a code path). Give a number with what it is compared to (`총괄이 넘기는 데 1.1분, 코디 구현은 15.4분`). Mark an estimate as `추정`.
- Give two to four options. If a current state exists, one option is `지금 그대로`. Compare all options in the same columns: what changes for the human (a "now → after" example), cost, risk, how to undo. Give each rejected option one line with the reason.

## Form

- One HTML file in the human's language. Put images in `data:` URLs. Do not point `src`, `href` or CSS `url()` to a different file (relative path, `file:`). `https:` URLs, for example web fonts, are permitted. Start from a copy of [style.html](style.html). Do not change the tokens, fonts, font sizes, head line, section numbers or table style in its `<style>`. Add only the blocks that you need.
- Draw mockup screens in the design of the target product. Before you draw, read the product's theme and token files, earlier mockups and screen captures. Use the same colors, fonts and components. Use the style.html tokens only for the document frame (head line, title, sections, lists, tables).
- Light is the default (`<html style="color-scheme: light">`). Put a dark-mode toggle button (`.theme-toggle`) at the top right.
- The head line at the top gives `응답할 곳: <role> · <node name>` (the node that receives the human's answer). A mockup also gives `연 담당: <role> · <node name>` and its version (`v2 · <date>`). Run `loom status` just before you ask, to get the node name. If the name changes, change the head line.
- Document ID: `loom show` adds it at the start of the head line (`문서 ID: M3`) and before the `<title>`. M is for a mockup and R for a report. A path keeps its ID through all versions (table `~/.cache/user-doc/ids.tsv`). The author does not write the ID. When you name a document in chat, give its ID.
- Give each document a short, different `<title>`. People find open windows by it.

| Type | Body |
|---|---|
| Mockup | Title; one paragraph of background (the human's words); a flow strip at the top (step → step); for each screen a numbered section (①②③) with a full-page drawing and callouts (see "Mockup callouts"); each open decision in a decision box (see "Decisions and IDs") right after the screen that the human must look at to decide it, or the nearest screen that it changes. No decision table. One desktop width |
| Report | Title; the conclusion in the first section; one section for each decision, in the order and form above; in each open decision section a diagram first that compares "now" with "after" or the options side by side (inline SVG with document tokens, visible in light and dark), then the recommendation, reasons and options table; a done item has only its three lines; a table only for what a diagram cannot show; sources as links; at the end a decision table (`정할 것`) with one line per decision: its options and the recommended option. `loom show` refuses a report without a diagram. Research, design and check records go in reports, not in mockups |

- Location: a mockup goes in the repository at `docs/mockups/<name>.html`; a report goes in the task folder (`$LOOM_TASK_DIR`, else `~/.cache/loom/<task>/<subject>.html`). Write a new version to the same path and increase its version number.

## Mockup callouts

- Draw the full page of each screen (head line, tabs, panels) at desktop width. Do not draw only the area that changes.
- Put only text that the user sees on a screen. Do not write notes for developers (sizes, scale, internal values that the user does not use). The checker cannot find these notes; the author and the node that asks read the document to make sure.
- Put a red numbered box around each item to explain. A new item gets a dashed box. A box has one border line. Do not give the item a second line such as `outline`.
- A box fits its item: it is 5px larger than the item. It stays in the table cell, image frame or clipping ancestor that holds the item. If a table cell is the item, the box is 1px inside the cell. The gaps between the content and the box differ by 4px or less left to right and top to bottom. A box border on a capture does not cut through the content of the capture.
- Draw boxes, badges, callouts and leader lines on top of the drawing. Do not change the item's layout.
- A number badge is not clipped or covered, and it does not cover text in the box. It can cover only a large control that holds the center of the box.
- Each explanation is a callout next to its box, in the nearest empty area. Empty background in the screen frame is an area too. A leader line goes from the box to the callout and is 120 screen px or shorter. If there is no area, the layout adds space below the section. If the line is still long, shorten the callout text or leave empty space in the drawing. A callout does not cover text, controls, boxes, badges or other callouts. A leader line does not cross other boxes, callouts or leader lines. If lines cross, put the callouts in the same top-to-bottom order as their boxes. Do not put a list of explanations below the drawing.
- These rules apply at window widths 1920, 1440, 1280, 1100 and 960px. They also apply when images load late and after a resize. The screen frame stays in the section width.
- Make: copy [callouts/callouts.css](callouts/callouts.css) into the `<style>` and [callouts/callouts.js](callouts/callouts.js) into a `<script>` at the end of `<body>`. The script places the boxes, badges, callouts and leader lines after the images load and after each resize.
  - In each section, put a screen frame in `<div class="fig"><h2>…</h2>`. The frame is `.page` (a drawn screen), `.xlwin` (an Excel window) or `.shotwrap` (a capture `<img>`).
  - Item: `<span class="mk" data-n="1">…</span>`; a new item is `class="mk new"`. `.mk` and `.new` only mark the item. Do not give them a style (outline, border, padding). For an item on a capture, put an empty element in `.shotwrap` with `style="left:…;top:…;width:…;height:…"`. The values are the position on the capture, in %. That box stays at that position.
  - Explanations: after the screen frame, `<div class="calls"><div><b>1</b><span>explanation</span></div>…</div>`. Item `<b>n</b>` is the callout of the box with `data-n="n"`. Numbers continue through the whole document: if section ① ends at 3, section ② starts at 4.
  - Give the product's control classes (for example `.abtn`, `.seg`) in `<body data-ui=".abtn, .seg">`. The layout and the check both keep clear of them.
- Check: `show.sh` runs [scripts/check-callouts.cjs](scripts/check-callouts.cjs) at all window widths. It counts violations of the one-file rule and all rules above, except the developer-note rule. One violation is a failure.

## Decisions and IDs

- Put each open decision in its own decision box. The box is amber, the color of the loom question notice. Explanation callouts stay red. Do not put a decision, an option or a recommendation in a callout or a note.
- Copy the markup and the `.decision` styles from [style.html](style.html):
  `<section class="decision" id="D1" data-for="2 3"><h3><span class="did">D1</span>question</h3><div class="opts"><div class="opt" data-recommended><span class="key">A</span><span class="label">option<span class="mark">권장</span><span class="sub">now → after · cost · risk · undo</span></span></div>…</div><ul class="why"><li>reason</li></ul></section>`.
  The option buttons and the recommended mark have the look of the inbox question options.
- Target: in a mockup, `data-for` gives the numbers of the boxes that the decision is about. [callouts.js](callouts/callouts.js) writes the line `대상: <section title> · 2, 3번` below the question and puts the decision ID next to the number of each of those callouts. Do not write that line yourself.
- Give two to four `.opt`. Exactly one has `data-recommended` and `<span class="mark">권장</span>`. Give at most three reasons in `.why`.
- IDs: decisions are `D1`, `D2`, …; `id` and `.did` have the same text. Callouts are numbers (see "Mockup callouts"). No ID is used twice in a document. Name items by ID in the document, in chat and in questions (`D2`, `7번`).
- Check: `show.sh` fails a document with an option or recommendation outside a decision box (in a mockup: the text `권장`, `정할 것` or `Recommended` outside a box, the lead, the head line and the screen drawings), a decision without exactly one recommended option, a missing ID or an ID used twice. In a document with callouts, it also fails a decision without `data-for` or with a number that is not a box with a callout in a screen section (`.fig`).

## Display

1. The author (a coordinator or a worker) writes the file and checks the render. It sends its parent only the path, version and a one-line summary.
2. The node that asks the human is the lead, or outside loom the agent that talks with the human. It reviews the content and runs [scripts/show.sh](scripts/show.sh): `bash <this skill folder>/scripts/show.sh <file>`.
   - `show.sh` calls `loom show <file>`. It checks the form (title, light default, toggle button, head line) and adds the document ID. If the check fails, send the document back to the author.
   - It keeps a render made with headless Chromium. The node that asks looks at that render.
   - On a computer without bash (a Windows node), call only `loom show <file>`.
3. Attach the document to the question: `loom ask "[<h1>](<file>) v3을 승인할까요?" --doc <file>`. The link text is the document's `<h1>`. A version or other words go after the link. loom drops `[` and `]` from the `<h1>` and refuses other link text. The link in the text becomes the document link. Without a link, loom adds "M3 <title>" at the end. When the human clicks it, the daemon opens it in the default browser. For a new version, write the same path, run `show.sh` again and ask again with `loom ask --doc`.
4. Outside loom (no inbox), give the human a link to the file.
5. In chat, write a summary in the human's language, a markdown link to the file and the open decisions. Do not copy the full tables into chat.
