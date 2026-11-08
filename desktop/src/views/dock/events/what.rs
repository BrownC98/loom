use api::graph::NodeState;
use api::graph::events::{BatchResult, Entry, EventRecord, Step, Who};
use api::graph::messages::{Field, Judgment, Kind, MessageKind, Question};
use api::graph::text::TextPart;
use api::profile::{AgentView, Provider};

use super::refusal;
use crate::strings::{Lang, Str};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Node,
    Message,
    Notice,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Question,
    Done,
    Fail,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Family(Family),
}

pub enum Seg {
    Kind(String, Tone),
    Who(Option<u32>, String),
    Loom,
    Text(String),
    Note(String, bool),
}

pub struct Ctx<'a> {
    pub lang: Lang,
    pub agents: &'a [AgentView],
    pub by_name: &'a dyn Fn(&str) -> Option<u32>,
}

impl Ctx<'_> {
    fn t(&self, text: Str) -> String {
        text.text(self.lang).to_owned()
    }

    fn with(&self, text: Str, args: &[(&str, &str)]) -> String {
        text.with(self.lang, args)
    }
}

pub fn category_of(entry: &Entry) -> Category {
    match entry {
        Entry::Message { .. } | Entry::Refused { .. } | Entry::Batch { .. } => Category::Message,
        Entry::Asked { .. }
        | Entry::Reported { .. }
        | Entry::Answered { .. }
        | Entry::Instructed { .. }
        | Entry::Unanswered { .. }
        | Entry::Confirmed { .. } => Category::Notice,
        _ => Category::Node,
    }
}

pub fn hidden(entry: &Entry) -> bool {
    matches!(
        entry,
        Entry::IdleOpened { .. }
            | Entry::IdleEnded { .. }
            | Entry::EndpointFailed { .. }
            | Entry::AgentStateChanged { .. }
            | Entry::AgentStateDisagreed { .. }
    )
}

pub fn family_of(entry: &Entry) -> Option<Family> {
    match entry {
        Entry::Message { what, .. } => what.as_ref().and_then(kind_family),
        Entry::Batch { result, .. } => match result {
            BatchResult::Passed => Some(Family::Done),
            BatchResult::CheckFailed
            | BatchResult::Conflict { .. }
            | BatchResult::TargetDirty
            | BatchResult::Error { .. } => Some(Family::Fail),
            BatchResult::TargetMoved | BatchResult::Interrupted => None,
        },
        Entry::Asked { .. } | Entry::Answered { .. } | Entry::Unanswered { .. } => {
            Some(Family::Question)
        }
        Entry::Reported { .. } | Entry::Instructed { .. } | Entry::Confirmed { .. } => {
            Some(Family::Done)
        }
        _ => None,
    }
}

fn kind_family(what: &MessageKind) -> Option<Family> {
    match what {
        MessageKind::Ask | MessageKind::Answer { .. } => Some(Family::Question),
        MessageKind::Done { .. } | MessageKind::Merged { .. } => Some(Family::Done),
        MessageKind::Verdict { judgment, .. } => Some(match judgment {
            Judgment::Pass => Family::Done,
            Judgment::Block => Family::Fail,
        }),
        MessageKind::Rework { .. }
        | MessageKind::Failed { .. }
        | MessageKind::SetupFailed { .. } => Some(Family::Fail),
        MessageKind::Exited { code, .. } => code.filter(|&c| c != 0).map(|_| Family::Fail),
        _ => None,
    }
}

fn kind_label(what: &MessageKind, how: Kind, c: &Ctx) -> String {
    let note = |c: &Ctx| c.t(Str::EventsTypeNote);
    match what {
        MessageKind::Note => match how {
            Kind::Queue => note(c),
            Kind::Steer => format!("{} \u{00b7} {}", note(c), c.t(Str::EventsHowSteer)),
            Kind::Interrupt => format!("{} \u{00b7} {}", note(c), c.t(Str::EventsHowInterrupt)),
        },
        MessageKind::Verdict { judgment, .. } => c.t(match judgment {
            Judgment::Pass => Str::EventsTypeVerdictPass,
            Judgment::Block => Str::EventsTypeVerdictBlock,
        }),
        MessageKind::Ask => c.t(Str::EventsTypeAsk),
        MessageKind::Answer { .. } => c.t(Str::EventsTypeAnswer),
        MessageKind::Done { .. } => c.t(Str::EventsTypeDone),
        MessageKind::Ready { .. } => c.t(Str::EventsTypeReady),
        MessageKind::Withdrawn { .. } => c.t(Str::EventsTypeWithdrawn),
        MessageKind::Merged { .. } => c.t(Str::EventsTypeMerged),
        MessageKind::Rework { .. } => c.t(Str::EventsTypeRework),
        MessageKind::Failed { .. } => c.t(Str::EventsTypeFailed),
        MessageKind::Exited { .. } => c.t(Str::EventsTypeExited),
        MessageKind::SetupFailed { .. } => c.t(Str::EventsTypeSetupFailed),
        MessageKind::Removed { .. } => c.t(Str::EventsTypeRemoved),
        MessageKind::Adopted { .. } => c.t(Str::EventsTypeAdopted),
        MessageKind::HumanInput { .. } => c.t(Str::EventsTypeHumanInput),
        MessageKind::RulesChanged { .. } => c.t(Str::EventsTypeRulesChanged),
    }
}

fn exit_code(code: Option<u32>, c: &Ctx) -> String {
    match code {
        Some(code) => c.with(Str::EventsCode, &[("code", &code.to_string())]),
        None => c.t(Str::EventsCodeUnknown),
    }
}

pub fn summary(what: &MessageKind, c: &Ctx) -> Option<String> {
    match what {
        MessageKind::Ready { branch, head, .. }
        | MessageKind::Verdict { branch, head, .. }
        | MessageKind::Withdrawn { branch, head }
        | MessageKind::Rework { branch, head, .. }
        | MessageKind::Failed { branch, head, .. } => Some(format!("{branch} \u{00b7} {head}")),
        MessageKind::Merged { branch, sha, .. } => Some(format!("{branch} \u{00b7} {sha}")),
        MessageKind::Exited { node, code } => {
            Some(format!("{node} \u{00b7} {}", exit_code(*code, c)))
        }
        MessageKind::SetupFailed { node, code } => {
            Some(format!("{node} \u{00b7} {}", exit_code(Some(*code), c)))
        }
        MessageKind::Removed { node } => Some(node.clone()),
        MessageKind::Adopted { node, parent } => Some(format!("{node} \u{2192} {parent}")),
        MessageKind::RulesChanged { skills, .. } => Some(skills.join(", ")),
        MessageKind::Ask
        | MessageKind::Answer { .. }
        | MessageKind::Done { .. }
        | MessageKind::Note
        | MessageKind::HumanInput { .. } => None,
    }
}

pub fn body_name(what: &MessageKind) -> Str {
    match what {
        MessageKind::Ask => Str::EventsBodyNameAsk,
        MessageKind::Answer { .. } => Str::EventsBodyNameAnswer,
        MessageKind::Done { .. } => Str::EventsBodyNameDone,
        MessageKind::Ready { .. }
        | MessageKind::Verdict { .. }
        | MessageKind::Withdrawn { .. }
        | MessageKind::Merged { .. }
        | MessageKind::Rework { .. }
        | MessageKind::Failed { .. } => Str::EventsBodyNameNext,
        MessageKind::SetupFailed { .. } => Str::EventsBodyNameOutput,
        MessageKind::HumanInput { .. } => Str::EventsBodyNameHuman,
        MessageKind::Note
        | MessageKind::Exited { .. }
        | MessageKind::Removed { .. }
        | MessageKind::Adopted { .. }
        | MessageKind::RulesChanged { .. } => Str::EventsBodyNameText,
    }
}

pub fn field_name(field: Field) -> Str {
    match field {
        Field::Question => Str::EventsFieldQuestion,
        Field::File => Str::EventsFieldFile,
        Field::Warning => Str::EventsFieldWarning,
        Field::Branch => Str::EventsFieldBranch,
        Field::Head => Str::EventsFieldHead,
        Field::Worktree => Str::EventsFieldWorktree,
        Field::Target => Str::EventsFieldTarget,
        Field::Judgment => Str::EventsFieldJudgment,
        Field::Findings => Str::EventsFieldFindings,
        Field::Reviewer => Str::EventsFieldReviewer,
        Field::Sha => Str::EventsFieldSha,
        Field::Cause => Str::EventsFieldCause,
        Field::Log => Str::EventsFieldLog,
        Field::Node => Str::EventsFieldNode,
        Field::Code => Str::EventsFieldCode,
        Field::Parent => Str::EventsFieldParent,
        Field::Skills => Str::EventsFieldSkills,
        Field::Commit => Str::EventsFieldCommit,
    }
}

pub fn who_of(entry: &Entry) -> Vec<&Who> {
    match entry {
        Entry::Created { node, parent, .. } => [Some(node), parent.as_ref()]
            .into_iter()
            .flatten()
            .collect(),
        Entry::State { node, .. }
        | Entry::Removed { node }
        | Entry::Renamed { node, .. }
        | Entry::Relaunched { node, .. }
        | Entry::Asked { node, .. }
        | Entry::Reported { node, .. }
        | Entry::Answered { node, .. }
        | Entry::Instructed { node, .. }
        | Entry::Unanswered { node, .. }
        | Entry::Confirmed { node, .. }
        | Entry::Nudged { node, .. }
        | Entry::IdleOpened { node }
        | Entry::IdleEnded { node, .. }
        | Entry::EndpointFailed { node, .. }
        | Entry::AgentStateChanged { node, .. }
        | Entry::AgentStateDisagreed { node, .. } => vec![node],
        Entry::Adopted { node, from, to, .. } => [Some(node), from.as_ref(), to.as_ref()]
            .into_iter()
            .flatten()
            .collect(),
        Entry::Message { from, to, .. } => {
            [Some(from), to.as_ref()].into_iter().flatten().collect()
        }
        Entry::Refused { from, .. } => vec![from],
        Entry::AccountSwitched { .. } | Entry::NoAccountLeft { .. } | Entry::Batch { .. } => {
            Vec::new()
        }
    }
}

pub fn involved(entry: &Entry) -> Vec<u32> {
    who_of(entry)
        .into_iter()
        .filter_map(|who| who.id.map(|id| id.0))
        .collect()
}

pub fn matches_name(entry: &Entry, needle: &str) -> bool {
    who_of(entry).iter().any(|who| who.name.contains(needle))
        || matches!(entry, Entry::Refused { to, .. } if to.contains(needle))
        || matches!(entry, Entry::Renamed { from, .. } if from.contains(needle))
}

pub fn plain(parts: &[TextPart]) -> String {
    parts
        .iter()
        .map(|part| match part {
            TextPart::Text { text } => text.as_str(),
            TextPart::Link { label, .. } | TextPart::Doc { label } => label.as_str(),
        })
        .collect()
}

pub fn body_of(record: &EventRecord, c: &Ctx) -> Option<Vec<TextPart>> {
    let parts = record.parts.clone();
    match &record.entry {
        Entry::Reported { instruction, .. } => {
            let line = c.with(Str::EventsInstruction, &[("text", instruction)]);
            let mut all = parts;
            all.push(TextPart::Text {
                text: format!("\n{line}"),
            });
            Some(all)
        }
        Entry::Asked { .. }
        | Entry::Answered { .. }
        | Entry::Instructed { .. }
        | Entry::Message { .. } => (!parts.is_empty()).then_some(parts),
        Entry::Batch {
            members,
            result,
            log,
            ..
        } => {
            let mut lines: Vec<String> = members
                .iter()
                .map(|m| {
                    let waited = c.with(Str::EventsWaited, &[("secs", &m.waited_secs.to_string())]);
                    format!("{} {} \u{00b7} {waited}", m.branch, m.head)
                })
                .collect();
            match result {
                BatchResult::Conflict { branch } => {
                    lines.push(c.with(Str::EventsConflictWith, &[("branch", branch)]));
                }
                BatchResult::Error { cause } => lines.push(cause.clone()),
                _ => {}
            }
            if let Some(log) = log {
                lines.push(format!("log: {log}"));
            }
            Some(vec![TextPart::Text {
                text: lines.join("\n"),
            }])
        }
        _ => None,
    }
}

fn state_label(state: &NodeState, c: &Ctx) -> String {
    match state {
        NodeState::Running => c.t(Str::NodeStateRunning),
        NodeState::Done => c.t(Str::NodeStateDone),
        NodeState::Exited { code } => {
            let code = code.map_or_else(|| "?".to_owned(), |code| code.to_string());
            c.with(Str::NodeStateExited, &[("code", &code)])
        }
        NodeState::SetupFailed { .. } => c.t(Str::NodeStateSetupFailed),
    }
}

fn agent_label(agent: Option<&api::ids::AgentId>, c: &Ctx) -> String {
    match agent {
        None => c.t(Str::NodeTerminal),
        Some(id) => c
            .agents
            .iter()
            .find(|a| a.id == *id)
            .map_or_else(|| id.0.clone(), |a| a.label.clone()),
    }
}

fn who(who: &Who) -> Seg {
    Seg::Who(who.id.map(|id| id.0), who.name.clone())
}

fn step_text(step: Step, c: &Ctx) -> String {
    c.t(match step {
        Step::Sent => Str::EventsStepSent,
        Step::Held => Str::EventsStepHeld,
        Step::Delivered => Str::EventsStepDelivered,
    })
}

fn step_note(what: &MessageKind, id: u32, step: Step, c: &Ctx) -> String {
    let step = step_text(step, c);
    match what {
        MessageKind::Ask => format!("#{id} \u{00b7} {step}"),
        MessageKind::Answer { question } => {
            let reference = match question {
                Question::Message { id } => format!("#{id}"),
                Question::Inbox { id } => format!("Q{id}"),
            };
            format!(
                "{} \u{00b7} {step}",
                c.with(Str::EventsAnswerTo, &[("ref", &reference)])
            )
        }
        _ => step,
    }
}

fn how_text(how: Kind, c: &Ctx) -> String {
    c.t(match how {
        Kind::Queue => Str::EventsHowQueue,
        Kind::Steer => Str::EventsHowSteer,
        Kind::Interrupt => Str::EventsHowInterrupt,
    })
}

fn wait_note(wait: api::ids::WaitId, extra: Option<String>) -> Seg {
    Seg::Note(
        match extra {
            Some(extra) => format!("Q{} \u{00b7} {extra}", wait.0),
            None => format!("Q{}", wait.0),
        },
        false,
    )
}

fn provider_text(provider: Provider, c: &Ctx) -> String {
    c.t(match provider {
        Provider::Claude => Str::SwitchProviderClaude,
        Provider::Codex => Str::SwitchProviderCodex,
    })
}

pub fn what(record: &EventRecord, c: &Ctx) -> Vec<Seg> {
    let arrow = || Seg::Text("\u{2192}".to_owned());
    let kind = |text: Str| Seg::Kind(c.t(text), Tone::Plain);
    match &record.entry {
        Entry::Created {
            node,
            parent,
            agent,
        } => {
            let agent = agent_label(agent.as_ref(), c);
            let note = match parent {
                Some(parent) => format!(
                    "{} \u{00b7} {agent}",
                    c.with(Str::EventsParent, &[("name", &parent.name)])
                ),
                None => agent,
            };
            vec![
                kind(Str::EventsKindCreated),
                who(node),
                Seg::Note(note, false),
            ]
        }
        Entry::State { node, from, to } => vec![
            kind(Str::EventsKindState),
            who(node),
            Seg::Note(
                format!("{} \u{2192} {}", state_label(from, c), state_label(to, c)),
                false,
            ),
        ],
        Entry::Adopted {
            node,
            from,
            to,
            held,
        } => {
            let name = |who: &Option<Who>| {
                who.as_ref()
                    .map_or_else(|| c.t(Str::EventsNoParent), |w| w.name.clone())
            };
            let mut note = format!("{} \u{2192} {}", name(from), name(to));
            if *held > 0 {
                note.push_str(&format!(
                    " \u{00b7} {}",
                    c.with(Str::EventsMovedHeld, &[("n", &held.to_string())])
                ));
            }
            vec![
                kind(Str::EventsKindAdopted),
                who(node),
                Seg::Note(note, false),
            ]
        }
        Entry::Removed { node } => vec![kind(Str::EventsKindRemoved), who(node)],
        Entry::Relaunched { node, error, .. } => {
            let mut out = vec![kind(Str::EventsKindRelaunched), who(node)];
            out.extend(error.iter().map(|error| Seg::Note(error.clone(), true)));
            out
        }
        Entry::Nudged { node, check } => {
            use api::graph::events::TurnCheck;
            vec![
                kind(Str::EventsKindNudged),
                who(node),
                Seg::Note(
                    c.t(match check {
                        TurnCheck::Unreported => Str::EventsTurnCheckUnreported,
                        TurnCheck::KindLine => Str::EventsTurnCheckKindLine,
                        TurnCheck::QuestionBox => Str::EventsTurnCheckQuestionBox,
                    }),
                    false,
                ),
            ]
        }
        Entry::Renamed { node, from } => vec![
            kind(Str::EventsKindRenamed),
            Seg::Text(from.clone()),
            arrow(),
            who(node),
        ],
        Entry::Message {
            how,
            what,
            from,
            to,
            step,
            ..
        } => {
            let mut out = Vec::new();
            match (what, &record.message) {
                (Some(what), Some(facts)) => {
                    out.push(Seg::Kind(
                        kind_label(what, *how, c),
                        kind_family(what).map_or(Tone::Plain, Tone::Family),
                    ));
                    out.push(if facts.from_loom {
                        Seg::Loom
                    } else {
                        who(from)
                    });
                    if let Some(to) = to {
                        out.push(arrow());
                        out.push(who(to));
                    }
                    let note = facts
                        .from_loom
                        .then(|| summary(what, c))
                        .flatten()
                        .unwrap_or_else(|| step_note(what, facts.id, *step, c));
                    out.push(Seg::Note(note, false));
                }
                _ => {
                    out.push(Seg::Kind(how_text(*how, c), Tone::Plain));
                    out.push(who(from));
                    if let Some(to) = to {
                        out.push(arrow());
                        out.push(who(to));
                    }
                    out.push(Seg::Note(step_text(*step, c), false));
                }
            }
            out
        }
        Entry::Refused {
            how,
            what,
            from,
            to,
            reason,
            ..
        } => {
            let head = match what {
                Some(what) => Seg::Kind(kind_label(what, *how, c), Tone::Family(Family::Fail)),
                None => Seg::Kind(how_text(*how, c), Tone::Plain),
            };
            vec![
                head,
                who(from),
                arrow(),
                Seg::Who((c.by_name)(to), to.clone()),
                Seg::Note(
                    c.with(
                        Str::EventsRefused,
                        &[("reason", &refusal::nameless(reason, c.lang))],
                    ),
                    true,
                ),
            ]
        }
        Entry::Asked { node, wait, .. } => {
            vec![
                kind(Str::EventsKindAsked),
                who(node),
                wait_note(*wait, None),
            ]
        }
        Entry::Reported { node, wait, .. } => vec![
            kind(Str::EventsKindReported),
            who(node),
            wait_note(*wait, None),
        ],
        Entry::Confirmed { node, wait } => vec![
            kind(Str::EventsKindConfirmed),
            who(node),
            wait_note(*wait, None),
        ],
        Entry::Answered {
            node, wait, images, ..
        }
        | Entry::Instructed {
            node, wait, images, ..
        } => {
            let label = if matches!(record.entry, Entry::Answered { .. }) {
                Str::EventsKindAnswered
            } else {
                Str::EventsKindInstructed
            };
            let extra =
                (*images > 0).then(|| c.with(Str::EventsImages, &[("n", &images.to_string())]));
            vec![kind(label), who(node), wait_note(*wait, extra)]
        }
        Entry::Unanswered { node, wait, by } => {
            use api::graph::events::ClosedBy;
            let by = c.t(match by {
                ClosedBy::Human => Str::EventsClosedByHuman,
                ClosedBy::Agent => Str::EventsClosedByAgent,
                ClosedBy::Restart => Str::EventsClosedByRestart,
                ClosedBy::Exit => Str::EventsClosedByExit,
            });
            vec![
                kind(Str::EventsKindUnanswered),
                who(node),
                wait_note(*wait, Some(by)),
            ]
        }
        Entry::Batch {
            batch,
            target,
            members,
            step,
            result,
            check_secs,
            ..
        } => {
            let result_text = c.t(match result {
                BatchResult::Passed => Str::EventsBatchResultPassed,
                BatchResult::CheckFailed => Str::EventsBatchResultCheckFailed,
                BatchResult::Conflict { .. } => Str::EventsBatchResultConflict,
                BatchResult::TargetMoved => Str::EventsBatchResultTargetMoved,
                BatchResult::TargetDirty => Str::EventsBatchResultTargetDirty,
                BatchResult::Interrupted => Str::EventsBatchResultInterrupted,
                BatchResult::Error { .. } => Str::EventsBatchResultError,
            });
            let mut note = format!(
                "{} \u{00b7} {result_text}",
                c.with(
                    Str::EventsBatch,
                    &[
                        ("batch", &batch.to_string()),
                        ("target", target),
                        ("n", &members.len().to_string()),
                        ("step", &step.to_string()),
                    ],
                )
            );
            if let Some(secs) = check_secs {
                note.push_str(&format!(
                    " \u{00b7} {}",
                    c.with(Str::EventsCheckSecs, &[("secs", &secs.to_string())])
                ));
            }
            vec![
                kind(Str::EventsKindBatch),
                Seg::Loom,
                Seg::Note(note, false),
            ]
        }
        Entry::AccountSwitched {
            provider,
            moved,
            later,
            failed,
            ..
        } => vec![
            kind(Str::EventsKindAccountSwitched),
            Seg::Text(provider_text(*provider, c)),
            Seg::Note(
                c.with(
                    Str::SwitchCounts,
                    &[
                        ("moved", &moved.to_string()),
                        ("later", &later.to_string()),
                        ("failed", &failed.to_string()),
                    ],
                ),
                false,
            ),
        ],
        Entry::NoAccountLeft { provider, .. } => vec![
            kind(Str::EventsKindNoAccountLeft),
            Seg::Text(provider_text(*provider, c)),
        ],
        Entry::IdleOpened { .. }
        | Entry::IdleEnded { .. }
        | Entry::EndpointFailed { .. }
        | Entry::AgentStateChanged { .. }
        | Entry::AgentStateDisagreed { .. } => Vec::new(),
    }
}
