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
            wake(&app.host, &app.idle, &app.first_inputs, node, &agent)
        })
        .await
        .unwrap_or(false)
    }
}

/// 쉬는 대기가 없는 노드의 화면이 쉬는 빈 입력 칸이면 깨우는 입력을 친다. 쳤으면 참이다.
/// 턴 중인지는 화면으로 본다. 턴 표시(`IdleWaits::in_turn`)는 stop 훅이 없으면 턴이 끝나도 지워지지 않는다(2026-10-23 시험 데몬 실측: 한 번 깨운 노드가 다음 메시지를 받지 못했다).
/// 화면을 읽고 글·Enter를 치는 동안 입력 잠금을 쥔다. 그 사이 사람이 친 키가 끼지 않는다(`NodeHost::write_paced_if`). `SUBMIT_AFTER`만큼 잠든다.
fn wake(
    host: &NodeHost<NodeId>,
    idle: &IdleWaits,
    first_inputs: &FirstInputs,
    node: NodeId,
    agent: &AgentId,
) -> bool {
    let Some(wake) = profile::wake(profile::profile_of(agent)) else {
        return false;
    };
    if idle.waiting(node) {
        return false;
    }
    host.write_paced_if(node, SUBMIT_AFTER, |keys| {
        if !wake.input.shown(&keys.screen().text) {
            return Err(());
        }
        first_inputs.typing(node);
        Ok(vec![wake.prompt.as_bytes().to_vec(), b"\r".to_vec()])
    })
    .is_ok()
}

/// 쉬는 대기 없이 `WAKE_AFTER` 넘게 쉬며 메시지를 받지 못한 노드를 깨운다. 데몬이 사는 동안 돈다.
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
            .filter(|(node, agent)| {
                profile::wake(profile::profile_of(agent)).is_some() && !app.idle.waiting(*node)
            })
            .collect();
        let due = stuck.due(Instant::now(), unwaited.iter().map(|(node, _)| *node));
        for (node, agent) in unwaited.iter().filter(|(node, _)| due.contains(node)) {
            if app.wake(*node, agent).await {
                stuck.0.remove(node);
            }
        }
    }
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

        /// 화면을 그리고, 받은 키를 그대로 출력하는 가짜 Cursor.
        fn fake_cursor(dir: &std::path::Path, screen: &str) -> NodeHost<NodeId> {
            // `-icrnl`: 친 Enter(`\r`)를 바꾸지 않고 그대로 돌려받는다.
            let mut cmd = portable_pty::CommandBuilder::new("sh");
            cmd.args([
                "-c",
                &format!("stty -icanon -echo -icrnl; printf '%s' '{screen}'; exec cat"),
            ]);
            node_host::in_process(dir, NodeId(1), cmd);
            let host = NodeHost::new(dir.to_path_buf());
            host.resume(NodeId(1), |_| {}).unwrap();
            while !host.screen(NodeId(1)).text.contains("cur @") {
                std::thread::sleep(Duration::from_millis(10));
            }
            host
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

        const IDLE: &str =
            "  done\n\n  → Add a follow-up\n\n  Claude Sonnet 5 300K High  cur @ master\n";
        const BUSY: &str = "  → Add a follow-up      ctrl+c to stop\n\n  Claude Sonnet 5 300K High  cur @ master\n";
        const DRAFT: &str = "  → half a sentence\n\n  Claude Sonnet 5 300K High  cur @ master\n";
        /// 대화 기록에 자리 표시 글과 같은 줄이 있어도 입력 칸(마지막 「→ 」 줄)은 사람이 쓰던 글이다.
        const DRAFT_BELOW_LOOKALIKE: &str = "  → Add a follow-up\n  done\n\n  → half a sentence\n\n  Claude Sonnet 5 300K High  cur @ master\n";

        /// 쉬는 대기 없이 쉬는 Cursor 노드는 메시지를 받지 못했다. 깨우는 입력과 Enter를 친다.
        #[test]
        fn an_idle_cursor_without_a_wait_gets_the_wake_prompt() {
            let dir = tempfile::tempdir().unwrap();
            let host = fake_cursor(dir.path(), IDLE);
            let mut typed = host.attach(NodeId(1)).unwrap().output;
            let idle = IdleWaits::default();
            let inputs = FirstInputs::default();
            let cursor = AgentId("cursor".into());
            assert!(wake(&host, &idle, &inputs, NodeId(1), &cursor));
            assert_eq!(
                typed_until_mark(&host, &mut typed),
                format!("{}\rZ", profile::cursor::WAKE.prompt)
            );
            assert!(
                inputs.injected(NodeId(1)),
                "the wake prompt is not human input"
            );

            // 쉬는 대기가 있으면 그 대기가 넘긴다. 치지 않는다.
            let _ = idle.open(NodeId(1));
            assert!(!wake(&host, &idle, &inputs, NodeId(1), &cursor));
            // 훅 프로필이 아닌 노드에는 치지 않는다.
            idle.end(NodeId(1));
            let codex = AgentId("codex".into());
            assert!(!wake(&host, &idle, &inputs, NodeId(1), &codex));
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
                std::thread::spawn(move || {
                    wake(
                        &host,
                        &IdleWaits::default(),
                        &FirstInputs::default(),
                        NodeId(1),
                        &AgentId("cursor".into()),
                    )
                })
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

        /// 일하는 화면이나 사람이 쓰던 글이 있는 입력 칸에는 치지 않는다.
        #[test]
        fn a_busy_or_drafted_input_is_not_typed_into() {
            for screen in [BUSY, DRAFT, DRAFT_BELOW_LOOKALIKE] {
                let dir = tempfile::tempdir().unwrap();
                let host = fake_cursor(dir.path(), screen);
                let mut typed = host.attach(NodeId(1)).unwrap().output;
                let inputs = FirstInputs::default();
                let woke = wake(
                    &host,
                    &IdleWaits::default(),
                    &inputs,
                    NodeId(1),
                    &AgentId("cursor".into()),
                );
                assert!(!woke, "{screen}");
                assert!(!inputs.injected(NodeId(1)));
                assert_eq!(typed_until_mark(&host, &mut typed), "Z");
                host.kill(NodeId(1));
            }
        }
    }
}
