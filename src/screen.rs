use crate::graph::{InputField, Prompt, PromptKey, PromptOption, WaitKind};

pub struct Dialog {
    pub text: &'static str,
    pub kind: WaitKind,
    pub place: Place,
    pub answer: Answer,
}

pub enum Answer {
    Plain,
    Below(Options),
    InBox(Options),
    Panel(Options),
}

impl Dialog {
    fn grammar(&self) -> Option<Grammar> {
        let (top, options) = match self.answer {
            Answer::Plain => return None,
            Answer::Below(options) => (Top::Line(self.text), options),
            Answer::InBox(options) => (Top::Boxed(self.text), options),
            Answer::Panel(options) => (Top::PanelEdge, options),
        };
        Some(Grammar {
            top,
            options,
            guard: None,
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Session {
    NotStarted,
    Started,
}

pub enum Place {
    BeforeSession,
    Panel,
}

const TRANSCRIPT_INDENT: usize = 2;

pub fn find(dialogs: &[Dialog], screen: &str, session: Session) -> Option<(WaitKind, String)> {
    let lines: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
    let panel_start = lines
        .iter()
        .rposition(|l| is_panel_edge(l))
        .map_or(lines.len(), |i| i + 1);
    let squeezed = squeeze(&lines.concat());
    let panel_at = squeezed.len() - squeeze(&lines[panel_start..].concat()).len();
    dialogs
        .iter()
        .filter_map(|d| {
            let needle = squeeze(d.text);
            let (at, text) = match d.place {
                Place::BeforeSession if session == Session::Started => return None,
                Place::BeforeSession => (squeezed.rfind(&needle)?, d.text),
                Place::Panel => (panel_at + squeezed[panel_at..].rfind(&needle)?, d.text),
            };
            Some((at, d.kind, text))
        })
        .max_by_key(|(at, ..)| *at)
        .map(|(_, kind, text)| (kind, text.to_owned()))
}

fn is_panel_edge(line: &str) -> bool {
    let body = line.trim_start_matches(' ');
    line.len() - body.len() < TRANSCRIPT_INDENT && body.starts_with(['─', '┌'])
}

fn squeeze(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

#[derive(Clone, Copy)]
pub struct Grammar {
    pub top: Top,
    pub options: Options,
    pub guard: Option<&'static str>,
}

#[derive(Clone, Copy)]
pub enum Top {
    PanelEdge,
    Line(&'static str),
    Boxed(&'static str),
}

#[derive(Clone, Copy)]
pub enum Options {
    Numbered,
    Hinted,
    Bracketed,
    Listed,
}

pub struct LinePrompt {
    pub text: &'static str,
    pub reply: Reply,
}

pub enum Reply {
    Words(&'static [(&'static str, &'static str)]),
    Field(InputField),
}

const SELECTED: [char; 4] = ['❯', '›', '→', '▶'];
const PROMPT_ENDS: [char; 4] = [':', '?', ']', ')'];
const CHECKED: char = '✔';
const BOX_SIDE: char = '│';
const HINT_LIMIT_CHARS: usize = 3;

struct Run {
    first: usize,
    end: usize,
    options: Vec<(PromptOption, bool)>,
}

pub fn prompt(
    grammars: &[Grammar],
    dialogs: &[Dialog],
    screen: &str,
    width: usize,
) -> Option<Prompt> {
    grammars
        .iter()
        .copied()
        .chain(dialogs.iter().filter_map(Dialog::grammar))
        .filter_map(|g| read(&g, screen, width))
        .rev()
        .max_by_key(|(top, _)| *top)
        .map(|(_, prompt)| prompt)
}

pub fn line(prompts: &[LinePrompt], to_cursor: &str) -> Option<(String, Prompt)> {
    let rows: Vec<String> = to_cursor
        .split('\n')
        .map(|l| l.trim_end().to_owned())
        .collect();
    let asked = rows.last()?.trim();
    if !asked.ends_with(PROMPT_ENDS) {
        return None;
    }
    let squeezed = squeeze(asked);
    let found = prompts
        .iter()
        .find(|p| squeezed.contains(&squeeze(p.text)))?;
    let (options, field) = match found.reply {
        Reply::Words(words) => (
            words
                .iter()
                .map(|(key, label)| PromptOption {
                    label: (*label).to_owned(),
                    key: PromptKey::Line((*key).to_owned()),
                })
                .collect(),
            None,
        ),
        Reply::Field(field) => (Vec::new(), Some(field)),
    };
    let text = asked.to_owned();
    let prompt = Prompt {
        body: vec![text.clone()],
        summary: text.clone(),
        rows,
        options,
        field,
    };
    Some((text, prompt))
}

fn read(grammar: &Grammar, screen: &str, width: usize) -> Option<(usize, Prompt)> {
    let lines: Vec<&str> = screen
        .lines()
        .map(|l| match grammar.top {
            Top::Boxed(_) => unbox(l),
            Top::PanelEdge | Top::Line(_) => l,
        })
        .collect();
    let top = match grammar.top {
        Top::PanelEdge => lines.iter().rposition(|l| is_panel_edge(l))? + 1,
        Top::Line(text) | Top::Boxed(text) => {
            lines.iter().rposition(|l| l.trim().starts_with(text))?
        }
    };
    let run = match grammar.options {
        Options::Listed => listed(&lines[top..])?,
        Options::Numbered | Options::Hinted | Options::Bracketed => {
            keyed(&grammar.options, &lines[top..], width)?
        }
    };
    let (first, end) = (top + run.first, top + run.end);
    let options = run.options;
    if options.len() < 2 || options.iter().filter(|(_, s)| *s).count() != 1 {
        return None;
    }
    let (body, summary) = body(grammar, &lines[top..first], width);
    let prompt = Prompt {
        rows: lines[top..end].iter().map(|&l| l.to_owned()).collect(),
        summary: summary.or_else(|| body.first().cloned())?,
        body,
        options: options
            .into_iter()
            .map(|(o, _)| match grammar.options {
                Options::Numbered => PromptOption {
                    label: without_hint(&o.label).to_owned(),
                    ..o
                },
                Options::Hinted | Options::Bracketed | Options::Listed => o,
            })
            .collect(),
        field: None,
    };
    Some((top, prompt))
}

fn keyed(kind: &Options, lines: &[&str], width: usize) -> Option<Run> {
    let first = lines.iter().position(|l| option(kind, l).is_some())?;
    let mut options: Vec<(PromptOption, bool)> = Vec::new();
    let mut indent = 0;
    let mut previous = "";
    let mut end = first;
    for &line in &lines[first..] {
        if let Some((parsed, selected)) = option(kind, line) {
            if let Options::Numbered = kind
                && parsed.key != PromptKey::Char(digit(options.len() + 1)?)
            {
                return None;
            }
            indent = line.len() - line.trim_start().len();
            options.push((parsed, selected));
        } else if !line.trim().is_empty() && line.len() - line.trim_start().len() > indent + 1 {
            let (last, _) = options.last_mut()?;
            let joint = wrapped(previous, line, width).unwrap_or(" ");
            last.label = format!("{}{joint}{}", last.label, line.trim());
        } else {
            break;
        }
        previous = line;
        end += 1;
    }
    Some(Run {
        first,
        end,
        options,
    })
}

fn listed(lines: &[&str]) -> Option<Run> {
    let marked = lines
        .iter()
        .position(|l| l.trim_start().starts_with(SELECTED))?;
    let column = label_column(lines[marked]);
    let same = |l: &&&str| !l.trim().is_empty() && label_column(l) == column;
    let first = marked - lines[..marked].iter().rev().take_while(same).count();
    let options: Vec<_> = lines[first..]
        .iter()
        .take_while(same)
        .enumerate()
        .map(|(i, l)| {
            let label: String = l.chars().skip(column).collect();
            let moves = i8::try_from(i).ok()? - i8::try_from(marked - first).ok()?;
            Some((
                PromptOption {
                    label: label.trim_end().to_owned(),
                    key: PromptKey::Move(moves),
                },
                first + i == marked,
            ))
        })
        .collect::<Option<_>>()?;
    Some(Run {
        first,
        end: first + options.len(),
        options,
    })
}

fn label_column(line: &str) -> usize {
    line.chars()
        .take_while(|c| *c == ' ' || *c == CHECKED || SELECTED.contains(c))
        .count()
}

fn unbox(line: &str) -> &str {
    line.trim()
        .strip_prefix(BOX_SIDE)
        .and_then(|l| l.strip_suffix(BOX_SIDE))
        .unwrap_or(line)
}

fn body(grammar: &Grammar, lines: &[&str], width: usize) -> (Vec<String>, Option<String>) {
    let margin = lines
        .iter()
        .filter(|l| !l.trim().is_empty() && !is_rule(l))
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut area = Area::Head;
    let mut rows: Vec<(Area, String)> = Vec::new();
    let mut previous = "";
    for &line in lines {
        let spaces = line.len() - line.trim_start_matches(' ').len();
        let text = line[spaces.min(margin)..].trim_end();
        let rule = is_rule(text);
        let next = match area {
            Area::Head if rule => Some(Area::Command {
                ruled: true,
                hang: 0,
            }),
            Area::Head if text.starts_with('$') => Some(Area::Command {
                ruled: false,
                hang: text.len() - text[1..].trim_start().len(),
            }),
            Area::Command { ruled: true, .. } if rule => Some(Area::Tail),
            Area::Command { ruled: false, .. } if text.is_empty() => Some(Area::Tail),
            _ => None,
        };
        let opened = next.is_some();
        if let Some(next) = next {
            area = next;
            previous = "";
            if rule {
                continue;
            }
        }
        if let Area::Command { hang, .. } = area {
            let code = text
                .strip_prefix('│')
                .map_or(text, |rest| rest.strip_prefix(' ').unwrap_or(rest));
            let code = if opened {
                code
            } else {
                code.strip_prefix(&*" ".repeat(hang)).unwrap_or(code)
            };
            if Some(code.trim_start_matches('$').trim()) != grammar.guard {
                rows.push((area, code.to_owned()));
            }
        } else if text.is_empty() || rule {
            previous = "";
        } else {
            match (wrapped(previous, line, width), rows.last_mut()) {
                (Some(joint), Some((_, last))) => *last = format!("{last}{joint}{}", text.trim()),
                _ => rows.push((area, text.trim().to_owned())),
            }
            previous = line;
        }
    }
    let summary = rows
        .iter()
        .find(|(area, row)| matches!(area, Area::Command { .. }) && !row.trim().is_empty())
        .map(|(_, row)| row.clone());
    let body = rows
        .into_iter()
        .filter(|(area, row)| !(*area == Area::Head && row.starts_with("Tip:")))
        .map(|(_, row)| row)
        .collect();
    (body, summary)
}

#[derive(Clone, Copy, PartialEq)]
enum Area {
    Head,
    Command { ruled: bool, hang: usize },
    Tail,
}

fn option(options: &Options, line: &str) -> Option<(PromptOption, bool)> {
    let rest = line.trim_start();
    let unmarked = rest.trim_start_matches(SELECTED).trim_start();
    let selected = unmarked.len() != rest.len();
    let parsed = match options {
        Options::Numbered => {
            let (number, label) = unmarked.split_once(". ")?;
            let number: u32 = number.parse().ok()?;
            PromptOption {
                label: label.trim_end().to_owned(),
                key: PromptKey::Char(char::from_digit(number, 10)?),
            }
        }
        Options::Hinted => {
            let (label, hint) = unmarked.trim_end().strip_suffix(')')?.rsplit_once(" (")?;
            PromptOption {
                label: label.to_owned(),
                key: hint.split(" or ").find_map(hinted_key)?,
            }
        }
        Options::Bracketed => {
            let (hint, label) = unmarked.strip_prefix('[')?.split_once("] ")?;
            PromptOption {
                label: label.trim_end().to_owned(),
                key: single_key(hint)?,
            }
        }
        Options::Listed => return None,
    };
    Some((parsed, selected))
}

fn without_hint(label: &str) -> &str {
    match label.rsplit_once(" (") {
        Some((text, hint))
            if hint.ends_with(')') && hint.chars().count() <= HINT_LIMIT_CHARS + 1 =>
        {
            text
        }
        _ => label,
    }
}

fn hinted_key(hint: &str) -> Option<PromptKey> {
    match hint {
        "tab" => Some(PromptKey::Tab),
        "shift+tab" => Some(PromptKey::BackTab),
        _ => single_key(hint),
    }
}

fn single_key(hint: &str) -> Option<PromptKey> {
    let mut chars = hint.chars();
    let c = chars.next()?;
    (chars.next().is_none() && c.is_ascii_alphanumeric()).then_some(PromptKey::Char(c))
}

fn wrapped(previous: &str, line: &str, width: usize) -> Option<&'static str> {
    let used = previous.chars().count();
    let word = line.split_whitespace().next()?.chars().count();
    let last = previous.split_whitespace().last()?.chars().count();
    let room = width - (line.len() - line.trim_start().len()).min(width);
    if is_rule(previous) || is_rule(line) {
        None
    } else if used >= width && last + word > room {
        Some("")
    } else if used + 1 + word > width {
        Some(if previous.ends_with('/') { "" } else { " " })
    } else {
        None
    }
}

fn is_rule(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty() && line.chars().all(|c| matches!(c, '╌' | '─'))
}

fn digit(n: usize) -> Option<char> {
    char::from_digit(u32::try_from(n).ok()?, 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIALOGS: [Dialog; 1] = [Dialog {
        text: "Yes, I trust this folder",
        kind: WaitKind::Trust,
        place: Place::BeforeSession,
        answer: Answer::Plain,
    }];

    const LINES: [LinePrompt; 2] = [
        LinePrompt {
            text: "password",
            reply: Reply::Field(InputField::Secret),
        },
        LinePrompt {
            text: "[y/n]",
            reply: Reply::Words(&[("y", "yes"), ("n", "no")]),
        },
    ];

    #[test]
    fn tui_text_matches_without_spaces() {
        let tail = "Quick safety check\n❯1.Yes,Itrustthisfolder\n  2. No, exit\nEnter to confirm";
        assert_eq!(
            find(&DIALOGS, tail, Session::NotStarted),
            Some((WaitKind::Trust, "Yes, I trust this folder".into()))
        );
    }

    #[test]
    fn first_dialogs_do_not_match_after_the_session_starts() {
        let middle = "⏺ The dialog says \"Yes, I trust this folder\".\n  Done.\n\n❯ ";
        let last = "⏺ Done.\n  Yes, I trust this folder";
        for screen in [middle, last] {
            assert_eq!(find(&DIALOGS, screen, Session::Started), None, "{screen}");
            assert_eq!(
                find(&DIALOGS, screen, Session::NotStarted).unwrap().0,
                WaitKind::Trust
            );
        }
    }

    #[test]
    fn a_shell_prompt_is_the_cursor_line_with_nothing_typed_after_it() {
        let waiting = "$ sudo ls\n[sudo] password for me:";
        let (text, prompt) = line(&LINES, waiting).unwrap();
        assert_eq!(text, "[sudo] password for me:");
        assert_eq!(prompt.rows, ["$ sudo ls", "[sudo] password for me:"]);
        assert_eq!(prompt.field, Some(InputField::Secret));
        for answered in [
            "$ sudo ls\n[sudo] password for me:\n",
            "$ sudo ls\n[sudo] password for me:\nfile\nme@host:~$",
            "read\nEnter password: hunter2",
            "Continue? [y/n] y",
            "",
        ] {
            assert_eq!(line(&LINES, answered), None, "{answered:?}");
        }
        let local = line(&LINES, "Account: LOCAL\nPassword:").unwrap().1;
        let remote = line(&LINES, "Account: REMOTE\nPassword:").unwrap().1;
        assert_ne!(local, remote);
    }

    #[test]
    fn the_dialog_nearest_the_end_wins() {
        let dialogs = [
            Dialog {
                text: "Choose the text style",
                kind: WaitKind::Auth,
                place: Place::BeforeSession,
                answer: Answer::Plain,
            },
            Dialog {
                text: "Select login method:",
                kind: WaitKind::Auth,
                place: Place::BeforeSession,
                answer: Answer::Plain,
            },
        ];
        let tail = "Choose the text style\n❯ Dark\nSelect login method:\n❯ 1. Claude account";
        assert_eq!(
            find(&dialogs, tail, Session::NotStarted).unwrap().1,
            "Select login method:"
        );
    }
}
