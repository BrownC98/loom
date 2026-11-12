# Mockups

The common rules (form, decisions, display) are in [SKILL.md](SKILL.md).

- Draw mockup screens in the design of the target product. Before you draw, read the product's theme and token files, earlier mockups and screen captures. Use the same colors, fonts and components. Use the style.html tokens only for the document frame (head line, title, sections, lists, tables).
- Body: title; one paragraph of background (the human's words); a flow strip at the top (step → step); for each screen a numbered section (①②③) with a full-page drawing and callouts (see "Mockup callouts"); each open decision in a decision box (see [SKILL.md "Decisions and IDs"](SKILL.md#decisions-and-ids)) right after the screen that the human must look at to decide it, or the nearest screen that it changes. No decision table. One desktop width.
- If a decision chooses between screen designs, draw a full-page section for each option, not only the recommended one (2026-11-12 human instruction "모든 시안에 대해 목업 만들어"). Name the decision ID and option key in its `<h2>` (`D1-A`). The decision box marks the recommended option.

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
