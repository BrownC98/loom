//! 터미널 화면에서 사람을 기다리는 확인 창을 찾는다. 런타임이 신호를 주지 않는 경우(폴더 신뢰, 로그인, 셸 프롬프트)의 감지기 하나다.
//! 찾을 글은 프로필이 갖는다(`profile::dialogs`). 바뀌는 이유는 CLI 화면 문구다.
//! 근거: docs/design.md 「사람 응답 대기」.

use crate::graph::WaitKind;

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
