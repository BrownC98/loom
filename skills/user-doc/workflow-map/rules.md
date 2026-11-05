# Workflow maps

A workflow map is a workflow analysis or design document (a work flow, a data pipeline, an analysis procedure). Work layers go from top to bottom. Each layer holds cards for information units (attributes and functions). Edges show what each unit needs and produces. A detail panel on the right shows a real example, real failures and missing-item checks.

The common rules (form, decisions, display) are in [SKILL.md](../SKILL.md). The report body rules in [report.md](../report.md) (conclusion section, diagram per decision) and the decision boxes do not apply to this type (see SKILL.md).

The reference is [example.html](example.html) (실시예 분석 정보 구조 v0.1, a file that the human gave on 2026-11-03).

## Procedure

1. Copy [template.html](template.html). Change only the data block (from `const META=` to the end of `const E=[…];`) and the text of `<header>`, `<footer>` and `<title>`. Do not change the styles or the engine code below the data block.
2. Match the density and tone of [example.html](example.html). Read it once when you make your first document.
3. Render the file with `show.sh --quick` ([SKILL.md "Display"](../SKILL.md#display)). If a red 「데이터 오류」 box shows at the top, fix the data. The box shows layer, node and check IDs that do not exist, nodes without edges and duplicate IDs.
4. Set 「응답할 곳」 in `<div class="who">` to your node name, then show the file to the human as SKILL.md "Display" tells.

## Data form

| Name | Form | Meaning |
|---|---|---|
| `META` | `{scope, exLabel, intro, introEnd}` | Scope badge text (`1차 목표`), example title (`예시 — <name of the real case>`), HTML of the first panel (how to read, flow list), last line of the first panel |
| `LAYERS` | `{id, name, q, color, tag?, sub?, out?}[]` | Work steps, from top to bottom. `q` is the one-line question that the step answers. `sub` marks a sub-step of the layer above. `out` marks a layer outside the scope (put it last) |
| `CHECKS` | `{'①': '<name> — <comparison method>'}` | Missing-item check points. If there are none, `{}` |
| `N` | node array | One card is one information unit (class) |
| `E` | `[from, to, kind, label]` | `need`: from is an input of to. `produce`: from makes to. `outedge`: an edge outside the scope |

Node fields:

- `id`: the English class name (`ExperimentGroup`). `ko`: the Korean name.
- `layer`, `checks?: ['①']`, `out?: true` (outside the scope: leave `attrs` and `fns` empty).
- `desc`: what this information is and why it is necessary. Write where the next layer uses it.
- `attrs`: `[name, form, note]`. In the form, write choices (`A | B`), other node names, `[]` (list) and `?` (can be missing).
- `fns`: `[signature, input, output, note]`. If the output is a defect, write `→ ReviewItem` and put the check number in the note.
- `ex`: HTML. Values from one real case, with the source position (paragraph, file, line). You can use tables and inline SVG. In SVG, use color tokens such as `var(--accent)`.
- `fail?`: `[case name, HTML]`. A failure that really occurred and its cause: a failure that occurred because this node was missing or weak.

## Content rules

- Use one real case in the `ex` of all nodes. Do not make up values. Mark values that you did not examine with `판독 필요` or `확인 필요`.
- Take `fail` only from real records (delivered files, feedback, logs). If there is no record, leave out the field.
- Put work outside the scope in the `LX` layer. It is hidden by default. Write the name of the final output node in `META.introEnd`.
- Put the `ReviewItem` (확인 필요) node, where defects and mismatches collect, in the common layer `LC`. Each check function has a `produce` edge to this node.
- Divide layers by one criterion only (work order). Do not put nodes of a different criterion in the same layer.
- Write the version and date next to the title (`v0.1 · 2026-11-05`), the goal in one line in the header, and the source document paths in `<footer>`.
- Keep class names, function names and quotes in their original form.
