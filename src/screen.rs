//! 터미널 화면에서 사람을 기다리는 확인 창을 찾는다. 런타임이 신호를 주지 않는 경우(폴더 신뢰, 로그인, 셸 프롬프트)의 감지기 하나다.
//! 찾을 글은 프로필이 갖는다(`profile::dialogs`). 바뀌는 이유는 CLI 화면 문구다.
//! 근거: docs/design.md 「사람 응답 대기」.

use crate::graph::WaitKind;

/// 화면에서 찾을 확인 창 하나.
pub struct Dialog {
    /// 화면 글. 비교할 때 공백과 대소문자를 무시한다. TUI는 공백을 커서 이동으로 그리기 때문이다(Claude 2.1.306 실측).
    pub text: &'static str,
    pub kind: WaitKind,
    /// 마지막 줄에 있어야 맞는다(셸 프롬프트: 답하면 다음 줄이 생겨 맞지 않는다). 아니면 끝 쪽 화면 어디에 있어도 맞는다(TUI 창).
    pub at_end: bool,
}

/// `screen`(지금 터미널 화면의 글, `NodeHost::screen`)에서 맞는 확인 창과 그 한 줄 글. 여럿이 맞으면 화면 끝에 가장 가까운 것이다.
/// 지나간 창의 글이 끝 쪽에 남아 있어도 지금 창을 고른다(Claude 첫 실행: 테마 창 뒤 로그인 창, 2.1.306 실측).
/// 셸 프롬프트는 마지막 줄을, TUI 창은 찾을 글을 한 줄 글로 쓴다.
pub fn find(dialogs: &[Dialog], tail: &str) -> Option<(WaitKind, String)> {
    let last = tail.lines().rev().map(str::trim).find(|l| !l.is_empty())?;
    let (squeezed_tail, squeezed_last) = (squeeze(tail), squeeze(last));
    dialogs
        .iter()
        .filter_map(|d| {
            let needle = squeeze(d.text);
            let at = match d.at_end {
                true => squeezed_last.contains(&needle).then_some(usize::MAX),
                false => squeezed_tail.rfind(&needle),
            }?;
            let text = if d.at_end { last } else { d.text };
            Some((at, d.kind, text))
        })
        .max_by_key(|(at, ..)| *at)
        .map(|(_, kind, text)| (kind, text.to_owned()))
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
            at_end: false,
        },
        Dialog {
            text: "password",
            kind: WaitKind::Input,
            at_end: true,
        },
    ];

    #[test]
    fn tui_text_matches_without_spaces() {
        let tail = "Quick safety check\n❯1.Yes,Itrustthisfolder\n  2. No, exit\nEnter to confirm";
        assert_eq!(
            find(&DIALOGS, tail),
            Some((WaitKind::Trust, "Yes, I trust this folder".into()))
        );
    }

    #[test]
    fn shell_prompt_matches_only_on_the_last_line() {
        let waiting = "$ sudo ls\n[sudo] password for me: ";
        assert_eq!(
            find(&DIALOGS, waiting),
            Some((WaitKind::Input, "[sudo] password for me:".into()))
        );
        let answered = "$ sudo ls\n[sudo] password for me:\nfile\nme@host:~$ ";
        assert_eq!(find(&DIALOGS, answered), None);
        assert_eq!(find(&DIALOGS, ""), None);
    }

    #[test]
    fn the_dialog_nearest_the_end_wins() {
        let dialogs = [
            Dialog {
                text: "Choose the text style",
                kind: WaitKind::Auth,
                at_end: false,
            },
            Dialog {
                text: "Select login method:",
                kind: WaitKind::Auth,
                at_end: false,
            },
        ];
        let tail = "Choose the text style\n❯ Dark\nSelect login method:\n❯ 1. Claude account";
        assert_eq!(find(&dialogs, tail).unwrap().1, "Select login method:");
    }
}
