# Reports

The common rules (form, decisions, display) are in [SKILL.md](SKILL.md).

- The first section answers the human's question in one claim and gives the number of open decisions (`지금 정할 것: 1개`).
- Start each heading with its conclusion, not only a topic: `새 요청은 지금처럼 총괄이 받는다 · 권장`, not `새 요청은 어디로 가나`.
- Give each decision section a status: `정할 것` (open) or `반영함` (done, the human only checks it). Put open decisions first. In each group, put decisions that cannot be undone first. A done item gives what was done, what changes for the human and how to undo it, in three lines or fewer.
- For each open decision, give the recommendation and its reasons before the options. Give at most three reasons, each a full sentence with evidence that the human can trace (a measurement, a human instruction, a code path). Give a number with what it is compared to (`총괄이 넘기는 데 1.1분, 코디 구현은 15.4분`). Mark an estimate as `추정`.
- Give two to four options. If a current state exists, one option is `지금 그대로`. Compare all options in the same columns: what changes for the human (a "now → after" example), cost, risk, how to undo. Give each rejected option one line with the reason.
- Body: title; the conclusion in the first section; one section for each decision, in the order and form above; in each open decision section a diagram first that compares "now" with "after" or the options side by side (inline SVG with document tokens, visible in light and dark), then the recommendation, reasons and options table; a done item has only its three lines; a table only for what a diagram cannot show; sources as links; at the end a decision table (`정할 것`) with one line per decision: its options and the recommended option. `loom show` refuses a report without a diagram. Research, design and check records go in reports, not in mockups. Workflow analysis and design documents use [workflow-map](workflow-map/rules.md).
