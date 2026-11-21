# Reports

The common rules (one file, fixed parts, display) are in [SKILL.md](SKILL.md).

- Only the fixed parts in [SKILL.md "Form"](SKILL.md#form) are required: `<title>`, the head line (document ID, version), the light default with the `.theme-toggle` button. `loom show` does not check the version. loom's document page adds the HTML download button.
- Order for a small context load ([SKILL.md](SKILL.md) gives the rule):
  - First section: the open decisions (or "결정할 것 없음"), the recommendation and what changes for the human. In a decision, do not use a term or ID that only a later section explains.
  - Decisions that the human made: one line each at most, or none.
  - Last, after a clear divider (for example `<hr>` and the heading `부록`): the agent-only detail.
- All else is free: style tokens (except the decision box rules and their tokens, see below), fonts, layout, the order of the middle sections, headings, diagrams, tables. You can start from [style.html](style.html) or write your own `<style>`.
- Decision boxes are optional. If you use them, follow [SKILL.md "Decisions and IDs"](SKILL.md#decisions-and-ids): the document page makes their options answerable. Copy their rules and tokens from style.html without change.
- Research, design and check records go in reports, not in mockups. Workflow analysis and design documents use [workflow-map](workflow-map/rules.md).
