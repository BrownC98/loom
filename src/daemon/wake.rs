use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::App;
use super::idle::IdleWaits;
use super::sessions::FirstInputs;
use crate::graph::NodeId;
use crate::node_host::NodeHost;
use crate::profile::{self, AgentId};

const WAKE_AFTER: Duration = Duration::from_secs(10);
const SUBMIT_AFTER: Duration = Duration::from_millis(300);

impl App {
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

struct Tools<'a> {
    host: &'a NodeHost<NodeId>,
    idle: &'a IdleWaits,
    first_inputs: &'a FirstInputs,
}

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
                .quiet_screen()
                .is_some_and(|screen| wake.input.shown(&screen.text));
            if !idle_input || !waking() {
                return Err(());
            }
            tools.first_inputs.typing(node);
            Ok(vec![wake.prompt.as_bytes().to_vec(), b"\r".to_vec()])
        })
        .is_ok()
}

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

fn idle_unwaited(host: &NodeHost<NodeId>, idle: &IdleWaits, node: NodeId, agent: &AgentId) -> bool {
    profile::wake(profile::profile_of(agent)).is_some_and(|wake| {
        !idle.waiting(node)
            && host
                .quiet_screen(node)
                .is_some_and(|screen| wake.input.shown(&screen.text))
    })
}

#[derive(Default)]
struct Stuck(HashMap<NodeId, Instant>);

impl Stuck {
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

    #[cfg(unix)]
    mod pty {
        use super::*;
        use crate::node_host;

        fn fake_cursor_then(dir: &std::path::Path, screen: &str, then: &str) -> NodeHost<NodeId> {
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

        fn fake_cursor(dir: &std::path::Path, screen: &str) -> NodeHost<NodeId> {
            fake_cursor_then(dir, screen, "cat")
        }

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
        const DRAFT_BELOW_LOOKALIKE: &str = "  → Add a follow-up\n  done\n\n  → half a sentence\n\n  Claude Sonnet 5 300K High  cur @ master\n";

        #[test]
        fn an_idle_cursor_without_a_wait_gets_the_wake_prompt_once() {
            let dir = tempfile::tempdir().unwrap();
            let host = fake_cursor(dir.path(), IDLE);
            let mut typed = host.attach(NodeId(1)).unwrap().output;
            let parts = Parts::default();
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
            assert!(!parts.wake(&host, "cursor", true));
            std::thread::sleep(node_host::KEY_QUIET);
            assert!(parts.wake(&host, "cursor", true));
            assert_eq!(
                typed_until_mark(&host, &mut typed),
                format!("{}\rZ", profile::cursor::WAKE.prompt)
            );

            let parts = Parts::default();
            let _ = parts.idle.open(NodeId(1));
            assert!(!parts.wake(&host, "cursor", true));
            parts.idle.end(NodeId(1));
            assert!(!parts.wake(&host, "codex", true));
            assert_eq!(typed_until_mark(&host, &mut typed), "Z");
            host.kill(NodeId(1));
        }

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

        #[test]
        fn a_screen_soon_after_a_key_is_not_trusted() {
            let dir = tempfile::tempdir().unwrap();
            let keys = dir.path().join("keys");
            let then = format!(
                "sh -c 'head -c 1 > \"{}\"; printf \"\\033[1;1Hnew transcript row\"; exec cat >/dev/null'",
                keys.display()
            );
            let host = fake_cursor_then(dir.path(), IDLE, &then);
            host.write_if(NodeId(1), |_| Ok::<_, ()>(b"h".to_vec()))
                .unwrap();
            while !host.screen(NodeId(1)).text.contains("new transcript row") {
                std::thread::sleep(Duration::from_millis(10));
            }
            std::thread::sleep(Duration::from_secs(1));
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

        #[test]
        fn a_key_that_changes_nothing_holds_the_wake_only_for_the_quiet() {
            let dir = tempfile::tempdir().unwrap();
            let host = fake_cursor_then(dir.path(), IDLE, "cat >/dev/null");
            host.write_if(NodeId(1), |_| Ok::<_, ()>(b"\x7f".to_vec()))
                .unwrap();
            let cursor = AgentId("cursor".into());
            assert!(!idle_unwaited(
                &host,
                &IdleWaits::default(),
                NodeId(1),
                &cursor
            ));
            std::thread::sleep(node_host::KEY_QUIET);
            assert!(idle_unwaited(
                &host,
                &IdleWaits::default(),
                NodeId(1),
                &cursor
            ));
            assert!(Parts::default().wake(&host, "cursor", true));
            host.kill(NodeId(1));
        }

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
