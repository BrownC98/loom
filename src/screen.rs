use crate::protocol::{InputField, Prompt, PromptKey, PromptOption, WaitKind};

#[derive(Default)]
pub struct Screen {
    pub text: String,
    pub undimmed: String,
    pub to_cursor: String,
    pub cols: u16,
}

pub struct Dialog {
    pub text: &'static str,
    pub kind: WaitKind,
    pub place: Place,
    pub answer: Answer,
}

pub enum Answer {
    Plain,
    Press(&'static str),
    Below(Options),
    InBox(Options),
    Panel(Options),
}

impl Dialog {
    fn grammar(&self) -> Option<Grammar> {
        let (top, options) = match self.answer {
            Answer::Plain | Answer::Press(_) => return None,
            Answer::Below(options) => (Top::Line(self.text), options),
            Answer::InBox(options) => (Top::Boxed(self.text), options),
            Answer::Panel(options) => (Top::PanelEdge, options),
        };
        Some(Grammar {
            top,
            options,
            guard: None,
            writes: &[],
        })
    }

    fn pressed(&self, screen: &str) -> Option<(usize, Prompt)> {
        let Answer::Press(label) = self.answer else {
            return None;
        };
        let lines: Vec<&str> = screen.lines().collect();
        let top = lines.iter().rposition(|l| !l.trim().is_empty())?;
        if !lines[top].contains(self.text) {
            return None;
        }
        let line = lines[top].trim().to_owned();
        Some((
            top,
            Prompt {
                rows: vec![lines[top].to_owned()],
                body: vec![line.clone()],
                summary: line,
                options: vec![PromptOption {
                    label: label.to_owned(),
                    key: PromptKey::Enter,
                    writes: false,
                }],
                field: None,
            },
        ))
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum SessionStart {
    NotStarted,
    Started,
}

pub enum Place {
    BeforeSession,
    Panel,
    OtherBeforeSession,
}

const TRANSCRIPT_INDENT: usize = 2;

pub fn find(dialogs: &[Dialog], screen: &str, session: SessionStart) -> Option<(WaitKind, String)> {
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
            let (named, at) = match d.place {
                Place::BeforeSession | Place::OtherBeforeSession
                    if session == SessionStart::Started =>
                {
                    return None;
                }
                Place::BeforeSession => (true, squeezed.rfind(&needle)?),
                Place::OtherBeforeSession => (false, squeezed.rfind(&needle)?),
                Place::Panel => (true, panel_at + squeezed[panel_at..].rfind(&needle)?),
            };
            Some(((named, at), d.kind, d.text))
        })
        .max_by_key(|(rank, ..)| *rank)
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
    pub writes: &'static [&'static str],
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
    Ticked,
}

pub struct LinePrompt {
    pub text: &'static str,
    pub reply: Reply,
}

pub enum Reply {
    Words(&'static [(&'static str, &'static str)]),
    Field(InputField),
}

const SELECTED: [char; 5] = ['❯', '›', '→', '▶', '>'];
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
        .chain(dialogs.iter().filter_map(|d| d.pressed(screen)))
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
        .filter_map(|p| Some((squeezed.rfind(&squeeze(p.text))?, p)))
        .max_by_key(|(at, _)| *at)?
        .1;
    let (options, field) = match found.reply {
        Reply::Words(words) => (
            words
                .iter()
                .map(|(key, label)| PromptOption {
                    label: (*label).to_owned(),
                    key: PromptKey::Line((*key).to_owned()),
                    writes: false,
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
        Top::PanelEdge => {
            let marked = lines
                .iter()
                .rposition(|l| l.trim_start().starts_with(SELECTED))
                .unwrap_or(lines.len());
            lines[..marked].iter().rposition(|l| is_panel_edge(l))? + 1
        }
        Top::Line(text) | Top::Boxed(text) => {
            lines.iter().rposition(|l| l.trim().starts_with(text))?
        }
    };
    let run = match grammar.options {
        Options::Listed | Options::Ticked => listed(&grammar.options, &lines[top..])?,
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
            .map(|(o, _)| {
                let label = match grammar.options {
                    Options::Numbered => without_hint(&o.label).to_owned(),
                    Options::Hinted | Options::Bracketed | Options::Listed | Options::Ticked => {
                        o.label
                    }
                };
                PromptOption {
                    writes: grammar.writes.iter().any(|w| label.starts_with(w)),
                    label,
                    ..o
                }
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
    for (at, &line) in lines.iter().enumerate().skip(first) {
        if line.trim().is_empty() {
            previous = "";
            continue;
        }
        if let Some((parsed, selected)) = option(kind, line) {
            if let Options::Numbered = kind
                && parsed.key != PromptKey::Char(digit(options.len() + 1)?)
            {
                return None;
            }
            indent = line.len() - line.trim_start().len();
            options.push((parsed, selected));
        } else if line.len() - line.trim_start().len() > indent + 1 {
            let (last, _) = options.last_mut()?;
            if let Some(joint) = wrapped(previous, line, width) {
                last.label = format!("{}{joint}{}", last.label, line.trim());
            }
        } else {
            break;
        }
        previous = line;
        end = at + 1;
    }
    Some(Run {
        first,
        end,
        options,
    })
}

fn listed(kind: &Options, lines: &[&str]) -> Option<Run> {
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
            let (label, key) = match kind {
                Options::Ticked => (
                    label
                        .strip_prefix(['['])
                        .and_then(|l| l.split_once("] "))?
                        .1
                        .to_owned(),
                    PromptKey::Tick(moves),
                ),
                _ => (label, PromptKey::Move(moves)),
            };
            Some((
                PromptOption {
                    label: label.trim_end().to_owned(),
                    key,
                    writes: false,
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
                writes: false,
            }
        }
        Options::Hinted => {
            let (label, hint) = unmarked.trim_end().strip_suffix(')')?.rsplit_once(" (")?;
            PromptOption {
                label: label.to_owned(),
                key: hint.split(" or ").find_map(hinted_key)?,
                writes: false,
            }
        }
        Options::Bracketed => {
            let (hint, label) = unmarked.strip_prefix('[')?.split_once("] ")?;
            PromptOption {
                label: label.trim_end().to_owned(),
                key: single_key(hint)?,
                writes: false,
            }
        }
        Options::Listed | Options::Ticked => return None,
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
