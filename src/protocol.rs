mod agent;
mod link;
mod refusal;
mod waits;

use serde::{Deserialize, Serialize};

pub use agent::{Model, StartDigests};
use api::graph::messages::{Field, Kind, MessageKind};
pub use link::{
    AckRequest, AdapterRecord, ChoiceRequest, EndedTurn, Handoff, IdleReply, IdleRequest, ListenAt,
    ListenRequest, Report, Session, TakeRequest, Taken, WakeReply,
};
pub use waits::OpenQuestion;

pub const VERSION: &str = match option_env!("LOOM_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

pub const COMMIT: Option<&str> = option_env!("LOOM_COMMIT");

#[derive(Clone, Copy, Debug)]
pub struct Versions {
    pub now: u32,
    pub oldest: u32,
}

impl Versions {
    pub fn accepts(self, version: u32) -> bool {
        (self.oldest..=self.now).contains(&version)
    }
}

pub const PTY_HOST: Versions = Versions { now: 5, oldest: 1 };
pub const NODE_CALLS: Versions = Versions { now: 2, oldest: 1 };
pub const NODE_CALLS_HEADER: &str = "loom-protocol";
pub const STREAMS: Versions = Versions { now: 2, oldest: 1 };
pub const DISTRO_HOST: u32 = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Deploy {
    #[default]
    Run,
    Skip,
}

impl Deploy {
    pub const SKIP_MARK: &str = "[skip ci]";
}

pub const LOOM: &str = "loom";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MessageId(pub u32);

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: MessageId,
    pub from: String,
    pub kind: Kind,
    pub body: String,
    #[serde(default)]
    pub what: Option<MessageKind>,
}

pub const BATCH_SEPARATOR: &str = "\n\n";

impl Delivery {
    pub fn text(&self) -> String {
        let mode = match self.kind {
            Kind::Queue => "",
            Kind::Steer => ", mode: steer",
            Kind::Interrupt => {
                ", mode: interrupt. Stop your current work and do what this message says"
            }
        };
        let note = MessageKind::Note;
        let what = self.what.as_ref().unwrap_or(&note);
        let from = if what.by_loom() { LOOM } else { &self.from };
        let mut lines = vec![format!(
            "loom message #{} (from: {from}, kind: {}{mode})",
            self.id.0,
            what.word()
        )];
        let repeated = what.repeats_file(&self.body);
        lines.extend(
            what.fields()
                .into_iter()
                .filter(|(name, _)| !(repeated && *name == Field::File))
                .map(|(name, value)| format!("{}: {value}", name.word())),
        );
        if !self.body.is_empty() {
            lines.push(self.body.clone());
        }
        lines.join("\n")
    }
}

pub struct KindWord {
    pub word: &'static str,
    pub verb: &'static str,
    pub gate: bool,
}

const fn word(word: &'static str, verb: &'static str, gate: bool) -> KindWord {
    KindWord { word, verb, gate }
}

const KIND_WORDS: [KindWord; 15] = [
    word("USER DIRECTION", NO_HUMAN_WORD, false),
    word("USER DECISION", NO_HUMAN_WORD, false),
    word(
        "RULE UPDATE",
        "nothing for built-in skills: loom relaunches each node whose start text changed. For other rules, use `loom note` without this word",
        false,
    ),
    word("QUESTION", "`loom ask`", false),
    word("ESCALATION", "`loom ask`", false),
    word("ANSWER", "`loom answer <message-id>`", false),
    word(
        "STATUS",
        "`loom done` when your work is finished, else `loom note parent`",
        false,
    ),
    word("DONE", "`loom done`", false),
    word("READY", "`loom ready`", true),
    word("CI RESULT", "`loom verdict`", true),
    word("MERGED", "nothing: the merge queue sends kind merged", true),
    word("REWORK", "`loom verdict`", true),
    word("BLOCK", "`loom verdict`", true),
    word("VERDICT", "`loom verdict`", true),
    word("GATE", "`loom --help`", true),
];

const NO_HUMAN_WORD: &str =
    "nothing: loom sends the human's terminal input to your parent (kind human-input)";

pub fn kind_word(text: &str) -> Option<&'static KindWord> {
    let text = text.trim_start();
    KIND_WORDS.iter().find(|w| {
        text.strip_prefix(w.word)
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
    })
}

pub fn gate_word(text: &str) -> Option<&'static str> {
    kind_word(text).filter(|w| w.gate).map(|w| w.word)
}

pub fn kind_line(text: &str) -> Option<&'static KindWord> {
    text.lines().find_map(kind_word)
}

pub const CHECK_SCRIPT: &str = "scripts/gate/check.sh";
pub const SHIP_SCRIPT: &str = "scripts/gate/ship.sh";
pub const SHIP_DEPLOY_FAILED: i32 = 11;

pub const REVIEW_RULES: &str = "# Review rules

Look first for the defects that the repository's AGENTS.md lists for review, in its order.

Read the owner's check log (result.md and logs/ in $LOOM_TASK_DIR). Do not run the full checks: the owner ran them, and the merge queue runs scripts/gate/check.sh if the repository has it. You can run one targeted test that shows a defect.";

#[macro_export]
macro_rules! message_usage {
    () => {
        "- loom ask <question>: with no parent, you MUST ask the human anything (a question, a choice, an approval, a permission) only this way, in the one-line form of the ask-human rules, then end your turn. With a parent, it asks your parent; the answer comes as a message (kind: answer), or wait for it: loom wait --reply-to <message-id> --timeout <seconds>.
- loom answer <message-id> <body>: answer an ask that you received.
- loom done <summary> [--file <path>] (a relative path is in $LOOM_TASK_DIR): report a finished result to your parent, also in a turn that a message or a notice started. With a parent, call it only when no work of your own remains (no background job, no wait): loom removes a done child after its turn ends. Send interim results with loom note parent. Call it again after more work. Do not call it when you only answer a question in text. People read the summary on your node card: write it in the language they use with you. With no parent, it also carries interim results; add --asked <the human's instruction, short> and --image <path> per image the human gave. This is the only way to tell the human that a result is ready. Send no progress reports. loom removes a done child, its clean worktree and its merged branch when the child's turn or process ended and your turn ended or you called loom done. A child stays while it has a child at work, unread mail either way, an open gate round or unmerged work, and while your own branch is not merged (for gate rework). To keep a done child for more work, add --keep <name>=<reason>; a message to that child ends the keep.
- loom note <target> <body> [--steer|--interrupt]: any other message. --steer puts it between the receiver's tool calls; --interrupt stops its current work. If you receive an interrupt, stop your previous task. Do not start a body with a kind word (QUESTION, STATUS, READY, ...): use the verb for that kind.
- loom ready <branch> [--no-deploy] (branch owner, all committed) sends the branch to its gate. --no-deploy: on a merge into the original checkout, the merge commit carries [skip ci], so the remote CI and its deploy do not run; into a coordinator branch it does nothing, and the coordinator gives it on its own loom ready. loom verdict <branch> <head> pass|block [--findings <file>] (give the head from the round message that loom sends; loom refuses another head; into the original checkout: only the branch's reviewers, never the owner: at `loom ready`, loom spawns one per reviewer role as children of the gate and sends them each next round; a branch made from its parent's branch has no reviewer: that parent checks the result against the brief's done-when and judges it; without --findings, review/<branch>-<your name>.md in your task folder; block goes to the owner as rework; pass puts the branch in the merge queue, and loom checks (scripts/gate/check.sh, if the repository has it), merges and sends merged, rework or failed). A new loom ready with a new commit replaces an open round until the queue takes the branch. The gate is your parent when your branch was made from its branch, else you. All return at once. loom picks receivers. loom --help has details.
"
    };
}
