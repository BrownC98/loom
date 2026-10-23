//! 터미널 화면에서 사람을 기다리는 확인 창과 쉬는 빈 입력 칸을 찾는다. 확인 창은 런타임이 신호를 주지 않는 경우(폴더 신뢰, 로그인, 셸 프롬프트)의 감지기 하나다.
//! 찾을 글은 프로필이 갖는다(`profile::dialogs`). 바뀌는 이유는 CLI 화면 문구다.
//! 근거: docs/design.md 「사람 응답 대기」.

use crate::graph::{Prompt, PromptKey, PromptOption, WaitKind};

/// 화면에서 찾을 확인 창 하나.
pub struct Dialog {
    /// 화면 글. 비교할 때 공백과 대소문자를 무시한다. TUI는 공백을 커서 이동으로 그리기 때문이다(Claude 2.1.306 실측).
    pub text: &'static str,
    pub kind: WaitKind,
    pub place: Place,
}

/// 노드 런타임의 세션 단계. 노드가 데몬을 처음 부르거나(훅, `loom` 명령) Codex 스레드가 생기면 시작한 것이다.
#[derive(Clone, Copy, PartialEq)]
pub enum Session {
    NotStarted,
    Started,
}

/// 확인 창 글이 있어야 하는 자리. 에이전트가 같은 글을 출력해도 창으로 보지 않게 한다.
pub enum Place {
    /// 마지막 줄(셸 프롬프트: 답하면 다음 줄이 생겨 맞지 않는다).
    LastLine,
    /// 세션이 시작하기 전 화면 어디든(TUI 첫 창: 폴더 신뢰, 로그인, 테마). 이 창이 떠 있는 동안 런타임은 세션을 시작하지 않는다.
    /// 시작한 뒤 화면의 같은 글은 에이전트 출력이다(2026-10-21 Claude 노드가 답에 "Yes, I trust this folder"를 써서 대기가 걸렸다).
    BeforeSession,
    /// 화면 아래 창 칸: 대화 기록보다 덜 들여 쓴 마지막 가로줄(`─`·`┌`로 시작하는 줄) 아래. 세션 중에 뜨고 훅이 알리지 않는 창이다(Cursor 승인·질문).
    /// Cursor는 대화 기록을 `TRANSCRIPT_INDENT`칸 들여 그리고 창 테두리는 왼쪽 끝에 그린다(2026.10.19 실측).
    /// 그래서 에이전트가 출력한 가로줄·표 아래의 같은 글은 창으로 보지 않는다.
    Panel,
}

/// 쉬는 TUI의 빈 입력 칸을 알아보는 글. 프로필이 갖는다(`profile::wake`).
pub struct IdleInput {
    /// 입력 칸 줄의 머리. 화면에서 이 머리로 시작하는 마지막 줄이 입력 칸이다. 대화 기록에도 이 머리로 시작하는 줄이 남는다.
    pub prompt: &'static str,
    /// 빈 입력 칸의 자리 표시 글. 입력 칸 줄이 이 글로 시작한다. 사람이 쓰던 글이 있거나 확인 창이 떠 있으면 이 줄이 없다.
    pub placeholders: &'static [&'static str],
    /// 일하는 동안만 화면에 있는 글.
    pub busy: &'static str,
}

impl IdleInput {
    /// `screen`이 쉬는 빈 입력 칸을 보인다.
    pub fn shown(&self, screen: &str) -> bool {
        !screen.contains(self.busy)
            && screen
                .lines()
                .map(str::trim_start)
                .rfind(|line| line.starts_with(self.prompt))
                .is_some_and(|line| self.placeholders.iter().any(|p| line.starts_with(p)))
    }
}

/// Cursor가 대화 기록(에이전트 답, 도구 출력, 표)을 들여 그리는 칸 수(2026.10.19 실측). `Place::Panel`의 창 테두리는 이보다 왼쪽이다.
const TRANSCRIPT_INDENT: usize = 2;

/// `screen`(지금 터미널 화면의 글, `NodeHost::screen`)에서 맞는 확인 창과 그 한 줄 글. 여럿이 맞으면 화면 끝에 가장 가까운 것이다.
/// 지나간 창의 글이 끝 쪽에 남아 있어도 지금 창을 고른다(Claude 첫 실행: 테마 창 뒤 로그인 창, 2.1.306 실측).
/// 셸 프롬프트는 마지막 줄을, TUI 창은 찾을 글을 한 줄 글로 쓴다.
pub fn find(dialogs: &[Dialog], screen: &str, session: Session) -> Option<(WaitKind, String)> {
    let lines: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
    let last = lines.last()?.trim();
    let panel_start = lines
        .iter()
        .rposition(|l| is_panel_edge(l))
        .map_or(lines.len(), |i| i + 1);
    let squeezed = squeeze(&lines.concat());
    let panel_at = squeezed.len() - squeeze(&lines[panel_start..].concat()).len();
    let squeezed_last = squeeze(last);
    dialogs
        .iter()
        .filter_map(|d| {
            let needle = squeeze(d.text);
            let (at, text) = match d.place {
                Place::LastLine => (squeezed_last.contains(&needle).then_some(usize::MAX)?, last),
                Place::BeforeSession if session == Session::Started => return None,
                Place::BeforeSession => (squeezed.rfind(&needle)?, d.text),
                Place::Panel => (panel_at + squeezed[panel_at..].rfind(&needle)?, d.text),
            };
            Some((at, d.kind, text))
        })
        .max_by_key(|(at, ..)| *at)
        .map(|(_, kind, text)| (kind, text.to_owned()))
}

/// 창 칸의 위 테두리. 들여 쓴 칸을 세려고 줄 앞 공백을 자르지 않은 줄을 받는다.
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

/// 승인 창을 읽는 법. 프로필이 갖는다(`profile::prompt_grammar`).
pub struct Grammar {
    pub top: Top,
    pub options: Options,
    /// loom이 셸 명령 앞에 붙이는 줄(`claude::GUARD`). 사람이 승인할 명령이 아니므로 본문에서 뺀다. Cursor는 그 앞에 `$`를 그린다.
    pub guard: Option<&'static str>,
}

/// 승인 창 본문이 시작하는 곳.
pub enum Top {
    /// 마지막 창 칸 테두리(`Place::Panel`과 같은 줄) 아래.
    PanelEdge,
    /// 이 글로 시작하는 마지막 줄부터(Codex: 테두리가 없다).
    Line(&'static str),
}

/// 선택지 줄의 꼴.
pub enum Options {
    /// `❯ 1. Yes`: 숫자 키 하나가 고르고 정한다. 끝의 `(y)` 같은 짧은 힌트는 글에서 뺀다(Codex).
    Numbered,
    /// `→ Run (once) (y)`: 끝 괄호가 키다(Cursor).
    Hinted,
}

/// 지금 고른 선택지 앞 표시. 승인 창이면 선택지 하나에 꼭 있다. 에이전트가 출력한 번호 목록은 이 표시가 없다.
const SELECTED: [char; 3] = ['❯', '›', '→'];
/// 숫자 선택지 끝의 키 힌트 글자 수 상한(Codex `(y)`, `(esc)`).
const HINT_LIMIT_CHARS: usize = 3;

/// 폭이 `width`칸인 `screen`(`NodeHost::screen`)에 떠 있는 승인 창. 창 꼴이 아니면 없다.
pub fn prompt(grammar: &Grammar, screen: &str, width: usize) -> Option<Prompt> {
    let lines: Vec<&str> = screen.lines().collect();
    let top = match grammar.top {
        Top::PanelEdge => lines.iter().rposition(|l| is_panel_edge(l))? + 1,
        Top::Line(text) => lines.iter().rposition(|l| l.trim().starts_with(text))?,
    };
    let first = top
        + lines[top..]
            .iter()
            .position(|l| option(&grammar.options, l).is_some())?;
    let mut options: Vec<(PromptOption, bool)> = Vec::new();
    let mut indent = 0;
    let mut previous = "";
    let mut end = first;
    for &line in &lines[first..] {
        if let Some((parsed, selected)) = option(&grammar.options, line) {
            if let Options::Numbered = grammar.options
                && parsed.key != PromptKey::Char(digit(options.len() + 1)?)
            {
                return None;
            }
            indent = line.len() - line.trim_start().len();
            options.push((parsed, selected));
        } else if !line.trim().is_empty() && line.len() - line.trim_start().len() > indent + 1 {
            // 긴 선택지가 다음 줄로 넘어갔다(Claude 2.1.309 실측: 경로가 든 「always allow」).
            let (last, _) = options.last_mut()?;
            let joint = wrapped(previous, line, width).unwrap_or(" ");
            last.label = format!("{}{joint}{}", last.label, line.trim());
        } else {
            break;
        }
        previous = line;
        end += 1;
    }
    if options.len() < 2 || options.iter().filter(|(_, s)| *s).count() != 1 {
        return None;
    }
    let (body, summary) = body(grammar, &lines[top..first], width);
    Some(Prompt {
        rows: lines[top..end].iter().map(|&l| l.to_owned()).collect(),
        summary: summary.or_else(|| body.first().cloned())?,
        body,
        options: options
            .into_iter()
            .map(|(o, _)| match grammar.options {
                // 힌트는 넘어간 줄 끝에 있을 수 있어 이은 뒤에 뺀다(Codex 0.159.2, 80칸 실측).
                Options::Numbered => PromptOption {
                    label: without_hint(&o.label).to_owned(),
                    ..o
                },
                Options::Hinted => o,
            })
            .collect(),
    })
}

/// 창 본문 줄과 명령 한 줄. 줄은 자리로 나눈다: 명령 앞 머리(CLI 안내), 명령 칸, 명령 뒤.
/// 명령 칸은 Claude의 두 점선 사이이거나(2.1.309), `$ `로 시작해 빈 줄까지다(Codex 0.159.2, Cursor 2026.10.22).
/// 명령 줄은 창 여백, Claude의 `│ ` 칸, 칸을 연 `$ ` 줄의 맞춤 들여쓰기만 빼고 그대로 둔다. 「Tip:」 안내는 머리에서만 뺀다.
/// 머리와 뒤의 줄은 다듬고, CLI가 폭에 맞춰 넘긴 줄을 앞 줄에 잇는다(80칸 화면의 Claude 「Tip:」 줄, Codex 「Reason:」 줄 실측).
fn body(grammar: &Grammar, lines: &[&str], width: usize) -> (Vec<String>, Option<String>) {
    // 점선은 창 왼쪽 끝부터 그린다(Claude 2.1.309).
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
            // Cursor는 이어지는 명령 줄을 `$` 뒤 글에 맞춰 들여 쓴다(2026.10.22 실측). 맞춤은 명령 칸을 연 `$` 줄에서만 잰다.
            // 명령 안의 `$` 줄(Claude 칸 안의 `$HOME` 등)은 맞춤이 아니다(2026-10-23 리뷰 「heredoc 들여쓰기가 줄었다」).
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
            // Claude는 여러 줄 명령을 `│ ` 칸에 그린다(2.1.309, loom guard 줄이 붙은 명령 실측).
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

/// 승인 창 본문 줄의 자리.
#[derive(Clone, Copy, PartialEq)]
enum Area {
    /// 명령 앞의 CLI 안내(도구 이름, 「Tip:」, 이유).
    Head,
    /// 명령 칸. `ruled`면 점선이 닫는다(Claude). 아니면 빈 줄이 닫는다. `hang`은 `$` 줄에 이어지는 줄의 맞춤 들여쓰기다.
    Command { ruled: bool, hang: usize },
    /// 명령 뒤(「Do you want to proceed?」).
    Tail,
}

/// 선택지 줄 하나와 그 줄이 고른 줄인지.
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
    };
    Some((parsed, selected))
}

/// 숫자 선택지 끝의 짧은 키 힌트(`(y)`, `(esc)`)를 뺀 글.
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

/// Cursor 힌트 글 하나의 키. `esc`는 PTY에서 이스케이프 시퀀스의 시작과 구별되지 않으므로 함께 적힌 `n`을 쓴다.
fn hinted_key(hint: &str) -> Option<PromptKey> {
    match hint {
        "tab" => Some(PromptKey::Tab),
        "shift+tab" => Some(PromptKey::BackTab),
        _ => {
            let mut chars = hint.chars();
            let c = chars.next()?;
            (chars.next().is_none() && c.is_ascii_alphanumeric()).then_some(PromptKey::Char(c))
        }
    }
}

/// `line`이 폭 `width`인 화면에서 `previous`가 넘친 나머지면 두 줄 사이에 넣을 글. CLI는 낱말 사이에서 줄을 넘기므로 빈칸이다. 낱말 가운데서 끊었으면 빈 글이다.
fn wrapped(previous: &str, line: &str, width: usize) -> Option<&'static str> {
    let used = previous.chars().count();
    let word = line.split_whitespace().next()?.chars().count();
    let last = previous.split_whitespace().last()?.chars().count();
    let room = width - (line.len() - line.trim_start().len()).min(width);
    if is_rule(previous) || is_rule(line) {
        None
    } else if used >= width && last + word > room {
        // 한 줄에 들어가지 않는 낱말은 가운데서 끊는다(Claude 2.1.309 경로 실측).
        Some("")
    } else if used + 1 + word > width {
        // Codex는 주소를 `/` 뒤에서 끊는다(0.159.2 실측).
        Some(if previous.ends_with('/') { "" } else { " " })
    } else {
        None
    }
}

/// 칸을 나누는 줄(`╌`, `─`). Claude 명령 칸의 빈 줄(`│`)은 아니다.
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

    const DIALOGS: [Dialog; 2] = [
        Dialog {
            text: "Yes, I trust this folder",
            kind: WaitKind::Trust,
            place: Place::BeforeSession,
        },
        Dialog {
            text: "password",
            kind: WaitKind::Input,
            place: Place::LastLine,
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

    /// 2026-10-21 Claude 노드가 답에 창 글을 써서 거짓 대기가 걸렸다. 세션이 시작한 뒤에는 화면 가운데에도 마지막 줄에도 맞지 않는다.
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
    fn shell_prompt_matches_only_on_the_last_line() {
        let waiting = "$ sudo ls\n[sudo] password for me: ";
        assert_eq!(
            find(&DIALOGS, waiting, Session::Started),
            Some((WaitKind::Input, "[sudo] password for me:".into()))
        );
        let answered = "$ sudo ls\n[sudo] password for me:\nfile\nme@host:~$ ";
        assert_eq!(find(&DIALOGS, answered, Session::Started), None);
        assert_eq!(find(&DIALOGS, "", Session::NotStarted), None);
    }

    #[test]
    fn the_dialog_nearest_the_end_wins() {
        let dialogs = [
            Dialog {
                text: "Choose the text style",
                kind: WaitKind::Auth,
                place: Place::BeforeSession,
            },
            Dialog {
                text: "Select login method:",
                kind: WaitKind::Auth,
                place: Place::BeforeSession,
            },
        ];
        let tail = "Choose the text style\n❯ Dark\nSelect login method:\n❯ 1. Claude account";
        assert_eq!(
            find(&dialogs, tail, Session::NotStarted).unwrap().1,
            "Select login method:"
        );
    }
}
