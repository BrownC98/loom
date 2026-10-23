//! 쉬는 대기 없이 쉬는 노드를 깨운다. 그런 노드에는 훅이 메시지를 넘기지 못한다.
//! loom이 노드 PTY에 짧은 입력을 쳐서 새 턴을 연다. 그 턴의 첫 훅이 메시지를 넘긴다.
//! 근거: docs/design.md 「전달 지점」.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::App;
use super::idle::IdleWaits;
use super::sessions::FirstInputs;
use crate::graph::NodeId;
use crate::node_host::NodeHost;
use crate::profile::{self, AgentId};

/// 깨우기 전 유예. 노드가 이 시간 내내 쉬는 대기 없이 쉬며 메시지를 받지 못해야 데몬이 스스로 깨운다.
/// 근거: 턴 끝에서 stop 훅이 대기를 열기까지(WSL interop으로 Windows `loom.exe`를 띄운다)와,
/// 데몬이 다시 시작한 뒤 쉬는 훅이 다시 붙기까지(`cli::RECONNECT_AFTER` 2초 간격)를 덮는다. 사람이 누르면 기다리지 않는다.
const WAKE_AFTER: Duration = Duration::from_secs(10);
/// 글과 Enter 사이. 글과 Enter를 한 번에 쓰면 Cursor는 글만 입력 칸에 남기고 제출하지 않았다. 0.3초 띄우면 제출했다(cursor-agent 2026.10.22 실측).
const SUBMIT_AFTER: Duration = Duration::from_millis(300);

impl App {
    /// 쉬는 대기 없이 쉬는 노드에 깨우는 입력을 친다. 다시 띄우는 중이거나 깨울 수 없는 화면이면 치지 않는다. 쳤으면 참이다.
    pub(super) async fn wake(self: &Arc<Self>, node: NodeId, agent: &AgentId) -> bool {
        if self.relaunches.holds_mail(node) {
            return false;
        }
        let (app, agent) = (self.clone(), agent.clone());
        tokio::task::spawn_blocking(move || {
            let waking = || app.graph.waking_agent(node).is_ok_and(|a| a.is_some());
            let tools = Tools {
                host: &app.host,
                idle: &app.idle,
                first_inputs: &app.first_inputs,
            };
            wake(&tools, node, &agent, waking)
        })
        .await
        .unwrap_or(false)
    }
}

/// 깨우기가 쓰는 데몬 부품.
struct Tools<'a> {
    host: &'a NodeHost<NodeId>,
    idle: &'a IdleWaits,
    first_inputs: &'a FirstInputs,
}

/// 쉬는 대기가 없는 노드의 화면이 쉬는 빈 입력 칸이면 깨우는 입력을 친다. 쳤으면 참이다.
/// 턴 중인지는 화면으로 본다. 턴 표시(`IdleWaits::in_turn`)는 stop 훅이 없으면 턴이 끝나도 지워지지 않는다(2026-10-23 시험 데몬 실측: 한 번 깨운 노드가 다음 메시지를 받지 못했다).
/// 아래를 모두 입력 잠금 안에서 본다(`NodeHost::write_paced_if`). 그 사이 사람이 친 키는 Enter 뒤에 간다. `SUBMIT_AFTER`만큼 잠든다.
/// - 화면이 마지막으로 친 키를 그린 화면이다(`Keys::settled_screen`). 사람이 막 친 키나 앞서 친 깨우는 글이 아직 화면에 없으면 치지 않는다. 앞선 깨우기도 친 키이므로 거듭 치지 않는다.
/// - `waking`: 아직 깨우는 메시지가 있다. 후보를 고른 뒤 훅이 꺼냈으면 치지 않는다.
fn wake(tools: &Tools, node: NodeId, agent: &AgentId, waking: impl FnOnce() -> bool) -> bool {
    let Some(wake) = profile::wake(profile::profile_of(agent)) else {
        return false;
    };
    if tools.idle.waiting(node) {
        return false;
    }
    tools
        .host
        .write_paced_if(node, SUBMIT_AFTER, |keys| {
            let idle_input = keys
                .settled_screen()
                .is_some_and(|screen| wake.input.shown(&screen.text));
            if !idle_input || !waking() {
                return Err(());
            }
            tools.first_inputs.typing(node);
            Ok(vec![wake.prompt.as_bytes().to_vec(), b"\r".to_vec()])
        })
        .is_ok()
}

/// 쉬는 대기 없이 `WAKE_AFTER` 넘게 쉬는 빈 입력 칸을 보이며 메시지를 받지 못한 노드를 깨운다. 데몬이 사는 동안 돈다.
/// 일하는 동안은 세지 않는다. 턴이 끝난 뒤 stop 훅이 대기를 열 시간을 준다.
pub(super) async fn watch(app: Arc<App>) {
    let mut stuck = Stuck::default();
    loop {
        tokio::time::sleep(WAKE_AFTER / 2).await;
        let waking = match app.graph.waking_agents() {
            Ok(waking) => waking,
            Err(e) => {
                eprintln!("loom: 안 읽은 메시지가 있는 노드를 읽지 못했다: {e}");
                continue;
            }
        };
        let unwaited: Vec<(NodeId, AgentId)> = waking
            .into_iter()
            .filter(|(node, agent)| idle_unwaited(&app.host, &app.idle, *node, agent))
            .collect();
        let due = stuck.due(Instant::now(), unwaited.iter().map(|(node, _)| *node));
        for (node, agent) in unwaited.iter().filter(|(node, _)| due.contains(node)) {
            if app.wake(*node, agent).await {
                stuck.0.remove(node);
            }
        }
    }
}

/// 깨우는 프로필이고, 쉬는 대기가 없고, 지금 화면이 쉬는 빈 입력 칸이다.
fn idle_unwaited(host: &NodeHost<NodeId>, idle: &IdleWaits, node: NodeId, agent: &AgentId) -> bool {
    profile::wake(profile::profile_of(agent)).is_some_and(|wake| {
        !idle.waiting(node)
            && host
                .settled_screen(node)
                .is_some_and(|screen| wake.input.shown(&screen.text))
    })
}

/// 노드마다 쉬는 대기 없이 메시지를 받지 못한 채 처음 본 때.
#[derive(Default)]
struct Stuck(HashMap<NodeId, Instant>);

impl Stuck {
    /// 이번에 본 노드(`now`에 쉬는 대기 없이 메시지가 남은 노드) 가운데 `WAKE_AFTER` 넘게 그랬던 노드. 이번에 없는 노드는 잊는다.
    fn due(&mut self, now: Instant, seen: impl Iterator<Item = NodeId>) -> Vec<NodeId> {
        let seen: Vec<NodeId> = seen.collect();
        self.0.retain(|node, _| seen.contains(node));
        seen.into_iter()
            .filter(|node| now - *self.0.entry(*node).or_insert(now) >= WAKE_AFTER)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_is_due_only_after_it_stays_stuck_for_the_grace() {
        let mut stuck = Stuck::default();
        let start = Instant::now();
        assert!(stuck.due(start, [NodeId(1)].into_iter()).is_empty());
        assert!(
            stuck
                .due(start + WAKE_AFTER / 2, [NodeId(1), NodeId(2)].into_iter())
                .is_empty()
        );
        assert_eq!(
            stuck.due(start + WAKE_AFTER, [NodeId(1), NodeId(2)].into_iter()),
            [NodeId(1)]
        );
        // 한 번이라도 대기가 열리거나 메시지를 받으면 다시 센다.
        assert!(
            stuck
                .due(start + WAKE_AFTER * 2, [NodeId(2)].into_iter())
                .contains(&NodeId(2))
        );
        assert!(
            stuck
                .due(start + WAKE_AFTER * 2, [NodeId(1)].into_iter())
                .is_empty()
        );
    }

    /// 노드 PTY 호스트를 테스트 프로세스 안에 띄운다(`node_host::in_process`, unix만).
    #[cfg(unix)]
    mod pty {
        use super::*;
        use crate::node_host;

        /// 화면을 그리고, 받은 키를 `then`(셸 명령)에 넘기는 가짜 Cursor.
        fn fake_cursor_then(dir: &std::path::Path, screen: &str, then: &str) -> NodeHost<NodeId> {
            // `-icrnl`: 친 Enter(`\r`)를 바꾸지 않고 그대로 돌려받는다.
            let mut cmd = portable_pty::CommandBuilder::new("sh");
            cmd.args([
                "-c",
                &format!("stty -icanon -echo -icrnl; printf '%s' '{screen}'; exec {then}"),
            ]);
            node_host::in_process(dir, NodeId(1), cmd);
            let host = NodeHost::new(dir.to_path_buf());
            host.resume(NodeId(1), |_| {}).unwrap();
            while !host.screen(NodeId(1)).text.contains("cur @") {
                std::thread::sleep(Duration::from_millis(10));
            }
            host
        }

        /// 받은 키를 그대로 화면에 다시 그리는 가짜 Cursor.
        fn fake_cursor(dir: &std::path::Path, screen: &str) -> NodeHost<NodeId> {
            fake_cursor_then(dir, screen, "cat")
        }

        /// 마지막 표시 「Z」까지 읽어 지금까지 친 키를 돌려준다.
        fn typed_until_mark(
            host: &NodeHost<NodeId>,
            typed: &mut tokio::sync::broadcast::Receiver<bytes::Bytes>,
        ) -> String {
            host.write_if(NodeId(1), |_| Ok::<_, ()>(b"Z".to_vec()))
                .unwrap();
            let mut seen = String::new();
            while !seen.ends_with('Z') {
                seen.push_str(&String::from_utf8_lossy(&typed.blocking_recv().unwrap()));
            }
            seen
        }

        #[derive(Default)]
        struct Parts {
            idle: IdleWaits,
            inputs: FirstInputs,
        }

        impl Parts {
            fn wake(&self, host: &NodeHost<NodeId>, agent: &str, waking: bool) -> bool {
                let tools = Tools {
                    host,
                    idle: &self.idle,
                    first_inputs: &self.inputs,
                };
                wake(&tools, NodeId(1), &AgentId(agent.into()), || waking)
            }
        }

        const IDLE: &str =
            "  done\n\n  → Add a follow-up\n\n  Claude Sonnet 5 300K High  cur @ master\n";
        const BUSY: &str = "  → Add a follow-up      ctrl+c to stop\n\n  Claude Sonnet 5 300K High  cur @ master\n";
        const DRAFT: &str = "  → half a sentence\n\n  Claude Sonnet 5 300K High  cur @ master\n";
        /// 대화 기록에 자리 표시 글과 같은 줄이 있어도 입력 칸(마지막 「→ 」 줄)은 사람이 쓰던 글이다.
        const DRAFT_BELOW_LOOKALIKE: &str = "  → Add a follow-up\n  done\n\n  → half a sentence\n\n  Claude Sonnet 5 300K High  cur @ master\n";

        /// 쉬는 대기 없이 쉬는 Cursor 노드는 메시지를 받지 못했다. 깨우는 입력과 Enter를 한 번 친다.
        #[test]
        fn an_idle_cursor_without_a_wait_gets_the_wake_prompt_once() {
            let dir = tempfile::tempdir().unwrap();
            let host = fake_cursor(dir.path(), IDLE);
            let mut typed = host.attach(NodeId(1)).unwrap().output;
            let parts = Parts::default();
            // 고른 뒤 훅이 메시지를 꺼냈으면 치지 않는다.
            assert!(!parts.wake(&host, "cursor", false));
            assert!(parts.wake(&host, "cursor", true));
            assert_eq!(
                typed_until_mark(&host, &mut typed),
                format!("{}\rZ", profile::cursor::WAKE.prompt)
            );
            assert!(
                parts.inputs.injected(NodeId(1)),
                "the wake prompt is not human input"
            );
            // 방금 친 깨우는 글은 아직 그린 화면이 아니다. 다시 치지 않는다.
            assert!(!parts.wake(&host, "cursor", true));
            // 그 턴이 끝나 다시 쉬는 빈 입력 칸이면 새 메시지에 또 깨운다. 가짜 화면은 친 키를 그려 늘 빈 입력 칸이다.
            std::thread::sleep(node_host::INPUT_SETTLE);
            assert!(parts.wake(&host, "cursor", true));
            assert_eq!(
                typed_until_mark(&host, &mut typed),
                format!("{}\rZ", profile::cursor::WAKE.prompt)
            );

            let parts = Parts::default();
            // 쉬는 대기가 있으면 그 대기가 넘긴다. 치지 않는다.
            let _ = parts.idle.open(NodeId(1));
            assert!(!parts.wake(&host, "cursor", true));
            // 훅 프로필이 아닌 노드에는 치지 않는다.
            parts.idle.end(NodeId(1));
            assert!(!parts.wake(&host, "codex", true));
            assert_eq!(typed_until_mark(&host, &mut typed), "Z");
            host.kill(NodeId(1));
        }

        /// 글과 Enter 사이에 사람이 친 키는 Enter 뒤에 간다. 깨우는 글에 섞여 제출되지 않는다.
        #[test]
        fn a_key_typed_between_the_prompt_and_enter_waits_for_the_enter() {
            let dir = tempfile::tempdir().unwrap();
            let host = Arc::new(fake_cursor(dir.path(), IDLE));
            let mut typed = host.attach(NodeId(1)).unwrap().output;
            let waker = {
                let host = host.clone();
                std::thread::spawn(move || Parts::default().wake(&host, "cursor", true))
            };
            let prompt = profile::cursor::WAKE.prompt;
            let mut seen = String::new();
            while seen.len() < prompt.len() {
                seen.push_str(&String::from_utf8_lossy(&typed.blocking_recv().unwrap()));
            }
            host.write_if(NodeId(1), |_| Ok::<_, ()>(b"H".to_vec()))
                .unwrap();
            assert!(waker.join().unwrap());
            seen.push_str(&typed_until_mark(&host, &mut typed));
            assert_eq!(seen, format!("{prompt}\rHZ"));
            host.kill(NodeId(1));
        }

        /// 사람이 친 키를 화면이 아직 그리지 않았으면 빈 입력 칸으로 보여도 치지 않는다. 창 제목만 바꾼 출력은 그린 것이 아니다.
        #[test]
        fn a_screen_that_has_not_drawn_the_last_key_is_not_trusted() {
            let dir = tempfile::tempdir().unwrap();
            let keys = dir.path().join("keys");
            let then = format!(
                "sh -c 'head -c 1 > \"{}\"; printf \"\\033]0;title\\007\"; exec cat >/dev/null'",
                keys.display()
            );
            let host = fake_cursor_then(dir.path(), IDLE, &then);
            let mut output = host.attach(NodeId(1)).unwrap().output;
            host.write_if(NodeId(1), |_| Ok::<_, ()>(b"h".to_vec()))
                .unwrap();
            output.blocking_recv().unwrap();
            std::thread::sleep(node_host::INPUT_SETTLE);
            assert!(
                profile::cursor::WAKE
                    .input
                    .shown(&host.screen(NodeId(1)).text)
            );
            assert!(!Parts::default().wake(&host, "cursor", true));
            let cursor = AgentId("cursor".into());
            assert!(!idle_unwaited(
                &host,
                &IdleWaits::default(),
                NodeId(1),
                &cursor
            ));
            assert_eq!(std::fs::read(&keys).unwrap(), b"h");
            host.kill(NodeId(1));
        }

        /// 터미널 응답(초점 등)은 사람이 친 키가 아니다. 입력 칸을 바꾸지 않으므로 화면을 그대로 믿는다.
        #[test]
        fn a_terminal_report_does_not_hold_the_wake() {
            let dir = tempfile::tempdir().unwrap();
            let host = fake_cursor_then(dir.path(), IDLE, "cat >/dev/null");
            host.write_if(NodeId(1), |_| Ok::<_, ()>(b"\x1b[O".to_vec()))
                .unwrap();
            let cursor = AgentId("cursor".into());
            assert!(idle_unwaited(
                &host,
                &IdleWaits::default(),
                NodeId(1),
                &cursor
            ));
            assert!(Parts::default().wake(&host, "cursor", true));
            host.kill(NodeId(1));
        }

        /// 일하는 동안은 유예를 세지 않는다. 쉬는 빈 입력 칸이 된 때부터 `WAKE_AFTER`를 다 센다.
        #[test]
        fn the_grace_starts_when_the_input_turns_idle() {
            let dir = tempfile::tempdir().unwrap();
            let host = fake_cursor(dir.path(), BUSY);
            let idle = IdleWaits::default();
            let cursor = AgentId("cursor".into());
            let mut stuck = Stuck::default();
            let mut ready = |at| {
                let seen = idle_unwaited(&host, &idle, NodeId(1), &cursor).then_some(NodeId(1));
                stuck.due(at, seen.into_iter())
            };
            let start = Instant::now();
            assert!(ready(start).is_empty());
            let redraw = format!("\x1b[2J\x1b[H{IDLE}");
            host.write_if(NodeId(1), |_| Ok::<_, ()>(redraw.into_bytes()))
                .unwrap();
            while !idle_unwaited(&host, &idle, NodeId(1), &cursor) {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(ready(start + WAKE_AFTER).is_empty());
            assert!(ready(start + WAKE_AFTER * 2).contains(&NodeId(1)));
            host.kill(NodeId(1));
        }

        /// 일하는 화면이나 사람이 쓰던 글이 있는 입력 칸에는 치지 않는다.
        #[test]
        fn a_busy_or_drafted_input_is_not_typed_into() {
            for screen in [BUSY, DRAFT, DRAFT_BELOW_LOOKALIKE] {
                let dir = tempfile::tempdir().unwrap();
                let host = fake_cursor(dir.path(), screen);
                let mut typed = host.attach(NodeId(1)).unwrap().output;
                let parts = Parts::default();
                assert!(!parts.wake(&host, "cursor", true), "{screen}");
                assert!(!parts.inputs.injected(NodeId(1)));
                assert_eq!(typed_until_mark(&host, &mut typed), "Z");
                host.kill(NodeId(1));
            }
        }
    }
}
