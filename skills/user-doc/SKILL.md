---
name: user-doc
description: Make a document for the human (a mockup or a report) as one HTML file, check it against the set form and style, and attach it as a link to an inbox question. Use it to show the human a mockup or a research, design, comparison or result report, when the human asks for "목업", "보고서" or "브라우저에 띄워", and for mockup approvals and reports in o-mode. Do not use it for briefs or notes that only agents read.
---

# Documents for the human

Mockups and reports use the same design language. Make, check and attach a document in one pass. Do not open the human's browser.

A document exists so that the human decides fast and correctly. Put the decisions first. Put facts that do not change a decision after the decisions, or remove them. Do not give the human more work to choose (more inputs, more choices) unless the human asks for it. For each decision, give the recommendation, what changes for the human (a "now → after" example), the cost, the risk and how to undo it.

## Form

- One HTML file in the human's language. Put images in `data:` URLs. Do not point `src`, `href` or CSS `url()` to a different file (relative path, `file:`). `https:` URLs, for example web fonts, are permitted. Start from a copy of [style.html](style.html). Do not change the tokens, fonts, font sizes, head line, section numbers or table style in its `<style>`. Add only the blocks that you need.
- Draw mockup screens in the design of the target product. Before you draw, read the product's theme and token files, earlier mockups and screen captures. Use the same colors, fonts and components. Use the style.html tokens only for the document frame (head line, title, sections, lists, tables).
- Light is the default (`<html style="color-scheme: light">`). Put a dark-mode toggle button (`.theme-toggle`) at the top right.
- The head line at the top gives `응답할 곳: <role> · <node name>` (the node that receives the human's answer). A mockup also gives `연 담당: <role> · <node name>` and its version (`v2 · <date>`). Run `loom status` just before you ask, to get the node name. If the name changes, change the head line.
- Document ID: `loom show` adds it at the start of the head line (`문서 ID: M3`) and before the `<title>`. M is for a mockup and R for a report. A path keeps its ID through all versions (table `~/.cache/user-doc/ids.tsv`). The author does not write the ID. When you name a document in chat, give its ID.
- Give each document a short, different `<title>`. People find open windows by it.

| Type | Body |
|---|---|
| Mockup | Title; one paragraph of background (the human's words); a flow strip at the top (step → step); for each screen a numbered section (①②③) with a full-page drawing and callouts (see "Mockup callouts"); at the end an approval table (`승인할 것`) that marks the recommended option. One desktop width |
| Report | Title; the conclusion in the first section; one section for each decision, with its options in a table (columns: what changes for the human, cost, risk); in each section a diagram first (inline SVG with document tokens, visible in light and dark), then a short list; a table only for what a diagram cannot show; sources as links; at the end a decision table (`정할 것`) with one line per decision: its options and the recommended option. `loom show` refuses a report without a diagram. Research, design and check records go in reports, not in mockups |

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
  - Explanations: after the screen frame, `<div class="calls"><div><b>1</b><span>explanation</span></div>…</div>`. Item n is the callout of the box with `data-n="n"`.
  - Give the product's control classes (for example `.abtn`, `.seg`) in `<body data-ui=".abtn, .seg">`. The layout and the check both keep clear of them.
- Check: `show.sh` runs [scripts/check-callouts.cjs](scripts/check-callouts.cjs) at all window widths. It counts violations of the one-file rule and all rules above, except the developer-note rule. One violation is a failure.

## Display

1. The author (a coordinator or a worker) writes the file and checks the render. It sends its parent only the path, version and a one-line summary.
2. The node that asks the human is the lead, or outside loom the agent that talks with the human. It reviews the content and runs [scripts/show.sh](scripts/show.sh): `bash <this skill folder>/scripts/show.sh <file>`.
   - `show.sh` calls `loom show <file>`. It checks the form (title, light default, toggle button, head line) and adds the document ID. If the check fails, send the document back to the author.
   - It keeps a render made with headless Chromium. The node that asks looks at that render.
   - On a computer without bash (a Windows node), call only `loom show <file>`.
3. Attach the document to the question: `loom ask "[<title>](<file>)을 승인할까요?" --doc <file>`. The link in the text becomes the document link. Without a link, loom adds "M3 <title>" at the end. When the human clicks it, the daemon opens it in the default browser. For a new version, write the same path, run `show.sh` again and ask again with `loom ask --doc`.
4. Outside loom (no inbox), give the human a link to the file.
5. In chat, write a summary in the human's language, a markdown link to the file and the open decisions. Do not copy the full tables into chat.
