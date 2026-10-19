//! OpenCode 2 프로필: 노드마다 `opencode serve` 하나를 띄우고, 메일함 메시지를 세션 입력함(`/api/session/{id}/prompt`)에 넣는다.
//! TUI는 `--server <주소> --session <id>`로 같은 세션에 붙는다. 바뀌는 이유는 OpenCode 2 API다.
//! 서버 프로세스와 HTTP 연결은 `server.rs`에 둔다. 근거: docs/design.md 「에이전트 실행」, 「전달 지점」.

mod server;

use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::graph::{Delivery, Kind};

use super::{Mailbox, VersionMismatch};
use server::Server;

/// 실측과 `docs/design.md`의 API 근거를 본 OpenCode 버전.
pub const PINNED_VERSION: &str = "2.0.32";
/// TUI와 서버가 같은 값을 읽는 비밀번호 환경 변수. 서버는 HTTP Basic(`opencode:<값>`)만 받는다.
pub const PASSWORD_ENV: &str = "OPENCODE_PASSWORD";
/// 카탈로그 `opencode2` 항목의 실행 파일.
pub const CMD: &str = "opencode2";
/// 같은 메시지 id로 다시 넣는 횟수와 간격. 서버는 같은 id를 한 번만 넣는다(실측). 그래서 다시 넣어도 두 번 들어가지 않는다.
const PROMPT_ATTEMPTS: u32 = 5;
const PROMPT_RETRY_DELAY: Duration = Duration::from_secs(1);

/// 세션 지시 항목(`instructions/entries/loom`). 모델이 모든 턴에 본다(실측).
const USAGE: &str = "\
너는 loom 그래프의 노드다. loom은 부모·자식 노드 사이 메시지를 나른다. 명령은 셸로 부른다.
- `loom spawn <이름> --agent opencode2` (지시문은 stdin, `--file <경로>`): 자식 노드를 만든다. `--agent claude`, `--agent codex`도 된다.
- `loom send <이름> <본문>`: 부모나 자식에게 보낸다. 받는 노드의 턴을 끊지 않는다.
- `loom steer <이름> <본문>`: 일하는 자식의 턴에 넣는다.
- `loom interrupt <이름> <본문>`: 자식이 하던 일을 멈추게 하고 이 본문을 따르게 한다.
- `loom done <요약>` / `loom done --file <경로>`: 결과를 부모에게 보내고 끝낸다. 요약은 1KB, 본문은 4KB 이하다.
받은 메시지는 「loom 메시지」로 시작하는 입력으로 들어온다. 중단 메시지를 받으면 하던 일을 멈추고 그 본문을 따른다.
자식의 결과를 기다리는 동안 할 일이 없으면 턴을 끝낸다. 자식의 결과는 새 입력으로 들어온다. `loom wait`는 부르지 않는다.";

/// `opencode` TUI 시작 인자. 카탈로그의 `--standalone`(비공개 서버)은 `--server`와 함께 쓸 수 없다.
pub fn args(url: &str, session: &str) -> Vec<String> {
    vec![
        "--server".into(),
        url.into(),
        "--session".into(),
        session.into(),
    ]
}

/// `opencode2 --version` 출력(예: `opencode v2.0.32`)이 고정 버전과 다르면 화면에 보일 경고다.
pub fn version_warning(version_output: &str) -> Option<VersionMismatch> {
    let last = version_output.split_whitespace().last()?;
    let installed = last.strip_prefix('v').unwrap_or(last);
    (installed != PINNED_VERSION).then(|| VersionMismatch {
        installed: installed.to_owned(),
        pinned: PINNED_VERSION.to_owned(),
    })
}

/// 노드 하나의 서버와 세션. 데몬이 노드를 띄우기 전에 만든다.
pub struct Node {
    server: Server,
    session: String,
}

impl Node {
    /// 서버를 띄우고 `cwd`에 세션을 만든 뒤 사용법을 지시 항목으로 넣는다.
    /// `node_env`는 노드 프로세스와 같은 환경이다. 셸 명령은 이 서버 아래에서 돌므로 `loom`이 그 값을 읽는다.
    /// `password`는 노드마다 새로 만든 서버 비밀번호다.
    pub async fn start(
        cwd: &Path,
        node_env: &[(&'static str, OsString)],
        password: String,
    ) -> Result<Self, String> {
        let server = Server::start(CMD, cwd, node_env, password).await?;
        let session = match server.create_session(cwd).await {
            Ok(session) => session,
            Err(e) => {
                server.stop();
                return Err(e);
            }
        };
        if let Err(e) = server.put_instructions(&session, "loom", USAGE).await {
            server.stop();
            return Err(e);
        }
        Ok(Self { server, session })
    }

    /// TUI가 붙을 서버와 세션.
    pub fn remote(&self) -> super::Remote<'_> {
        super::Remote::OpenCode2 {
            url: self.server.url(),
            session: &self.session,
            password: self.server.password(),
        }
    }

    /// 세션에서 돌고 있는 셸 명령과 그 자손을 끝낸다. 데몬이 중단 메시지를 넣기 전에 부른다.
    /// 실행만 멈추면 `setsid`로 띄운 셸 자손이 남는다(실측).
    pub async fn stop_shells(&self) {
        self.server.stop_shells(&self.session).await;
    }

    /// 서버와 그 아래 프로세스를 모두 끝낸다. `run`도 끝난다.
    pub fn stop(&self) {
        self.server.stop();
    }

    /// 메일함을 세션에 넘긴다. 서버가 끝나면 끝난다. 노드마다 이 작업 하나만 꺼내므로 같은 메시지를 두 번 넘기지 않는다.
    pub async fn run(&self, mailbox: impl Mailbox) {
        let api = Session {
            server: &self.server,
            id: &self.session,
        };
        let mut stopped = self.server.stopped();
        loop {
            // 메일함을 보기 전에 만든다. 그래야 그 사이에 온 메시지를 놓치지 않는다.
            let changed = mailbox.changed();
            match next(&mailbox) {
                Ok(Some(batch)) => {
                    match deliver(&api, &batch, PROMPT_RETRY_DELAY).await {
                        Ok(()) => {
                            if let Err(e) = mailbox.ack(&batch.taken) {
                                eprintln!("loom: OpenCode 2 노드의 받음 확인을 쓰지 못했다: {e}");
                            }
                        }
                        // 확인하지 않았으므로 메일함이 나중에 다시 꺼낸다.
                        Err(e) => eprintln!(
                            "loom: OpenCode 2 세션 {}에 메시지를 넘기지 못했다: {e}",
                            self.session
                        ),
                    }
                    continue;
                }
                Ok(None) => {}
                Err(e) => eprintln!("loom: OpenCode 2 노드의 메일함을 읽지 못했다: {e}"),
            }
            tokio::select! {
                _ = changed => {}
                _ = stopped.wait_for(|s| *s) => return,
            }
        }
    }
}

/// 세션 입력함의 전달 방식. 서버가 정한 이름이다.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Inbox {
    /// 다음 단계 사이(도구 호출 뒤)에 넣는다. 쉬는 중이면 새로 시작한다.
    Steer,
    /// 지금 실행이 끝난 뒤 넣는다. 쉬는 중이면 새로 시작한다.
    Queue,
}

impl Inbox {
    fn as_str(self) -> &'static str {
        match self {
            Inbox::Steer => "steer",
            Inbox::Queue => "queue",
        }
    }
}

/// 메일함에서 한 번에 꺼낸 메시지와 넣을 방식.
#[derive(Debug, PartialEq)]
struct Batch {
    taken: Vec<Delivery>,
    inbox: Inbox,
    interrupt: bool,
}

/// 조정·중단을 먼저 꺼낸다. 없으면 큐를 꺼낸다. 서버 입력함이 상태(일하는 중·쉬는 중)를 스스로 가린다.
fn next(mailbox: &impl Mailbox) -> Result<Option<Batch>, String> {
    let urgent = mailbox.take(&[Kind::Steer, Kind::Interrupt], super::BATCH_LIMIT_BYTES)?;
    if !urgent.is_empty() {
        let interrupt = urgent.iter().any(|d| d.kind == Kind::Interrupt);
        return Ok(Some(Batch {
            taken: urgent,
            inbox: Inbox::Steer,
            interrupt,
        }));
    }
    let queued = mailbox.take(&[Kind::Queue], super::BATCH_LIMIT_BYTES)?;
    Ok((!queued.is_empty()).then_some(Batch {
        taken: queued,
        inbox: Inbox::Queue,
        interrupt: false,
    }))
}

/// 세션 하나에 보내는 요청. 테스트는 가짜 서버로 바꾼다.
trait Api {
    /// 실행을 멈춘다. 셸은 데몬이 이미 끝냈다(`Node::stop_shells`).
    async fn interrupt(&self) -> Result<(), String>;
    /// `id`는 메시지 id다. 같은 id로 다시 보내도 서버는 한 번만 넣는다.
    async fn prompt(&self, id: &str, text: &str, inbox: Inbox) -> Result<(), String>;
}

struct Session<'a> {
    server: &'a Server,
    id: &'a str,
}

impl Api for Session<'_> {
    async fn interrupt(&self) -> Result<(), String> {
        self.server.interrupt(self.id).await
    }

    async fn prompt(&self, id: &str, text: &str, inbox: Inbox) -> Result<(), String> {
        self.server.prompt(self.id, id, text, inbox.as_str()).await
    }
}

/// 꺼낸 메시지를 세션에 넣는다. 중단이면 실행을 멈춘 뒤 조정으로 넣는다. 멈춘 실행에 남은 큐는 그 뒤에 이어진다(실측).
/// 넣기가 실패하면 같은 id로 `PROMPT_ATTEMPTS`번까지 다시 넣는다. 그래도 실패하면 호출자가 받음을 확인하지 않는다.
async fn deliver(api: &impl Api, batch: &Batch, retry_delay: Duration) -> Result<(), String> {
    if batch.interrupt
        && let Err(e) = api.interrupt().await
    {
        // 멈추지 못해도 본문은 조정으로 들어간다.
        eprintln!("loom: OpenCode 2 실행을 멈추지 못했다: {e}");
    }
    let id = message_id();
    let text = super::message_text(&batch.taken);
    let mut attempt = 1;
    loop {
        match api.prompt(&id, &text, batch.inbox).await {
            Ok(()) => return Ok(()),
            Err(e) if attempt == PROMPT_ATTEMPTS => return Err(e),
            Err(_) => {
                attempt += 1;
                tokio::time::sleep(retry_delay).await;
            }
        }
    }
}

/// OpenCode 메시지 id와 같은 모양(`msg_` + 시간 12자리 16진수 + 무작위 14자). 서버는 `msg_` 앞머리만 검사한다.
/// 시간 부분(밀리초 × 4096)은 앞 id보다 늘 크다. 그래서 OpenCode가 만드는 id와 같은 순서로 정렬된다.
fn message_id() -> String {
    static LAST: AtomicU64 = AtomicU64::new(0);
    const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let now = ms * 0x1000;
    let next = |last: u64| now.max(last + 1);
    let last = LAST
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
            Some(next(last))
        })
        .expect("늘 Some을 돌려준다");
    let time = next(last) & 0xFFFF_FFFF_FFFF;
    let mut random = [0u8; 14];
    // 무작위 값을 얻지 못하면 0이 남는다. 시간과 카운터가 이미 id를 가른다.
    let _ = getrandom::fill(&mut random);
    let tail: String = random
        .iter()
        .map(|b| BASE62[*b as usize % BASE62.len()] as char)
        .collect();
    format!("msg_{time:012x}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn delivery(kind: Kind, body: &str) -> Delivery {
        Delivery {
            id: crate::graph::MessageId(1),
            from: "부모".into(),
            kind,
            body: body.into(),
        }
    }

    /// 꺼낼 때 받은 방식과 상한을 적는 메일함. `kinds`에 맞는 메시지를 온 순서로 모두 꺼낸다.
    #[derive(Default)]
    struct FakeMailbox {
        queued: Mutex<Vec<Delivery>>,
        takes: Mutex<Vec<(Vec<Kind>, usize)>>,
    }

    impl Mailbox for FakeMailbox {
        fn changed(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(std::future::pending())
        }

        fn take(&self, kinds: &[Kind], limit_bytes: usize) -> Result<Vec<Delivery>, String> {
            self.takes
                .lock()
                .unwrap()
                .push((kinds.to_vec(), limit_bytes));
            let mut queued = self.queued.lock().unwrap();
            let (taken, kept) = queued.drain(..).partition(|d| kinds.contains(&d.kind));
            *queued = kept;
            Ok(taken)
        }

        fn ack(&self, _taken: &[Delivery]) -> Result<(), String> {
            Ok(())
        }
    }

    /// 보낸 요청을 적는 가짜 서버. 처음 `fail`번의 넣기는 실패한다.
    #[derive(Default)]
    struct FakeApi {
        fail: u32,
        calls: Mutex<Vec<String>>,
    }

    impl Api for FakeApi {
        async fn interrupt(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push("interrupt".into());
            Ok(())
        }

        async fn prompt(&self, id: &str, text: &str, inbox: Inbox) -> Result<(), String> {
            let mut calls = self.calls.lock().unwrap();
            calls.push(format!("{} {id} {text}", inbox.as_str()));
            if calls.len() as u32 <= self.fail {
                return Err("연결이 끊겼다".into());
            }
            Ok(())
        }
    }

    #[test]
    fn steer_and_interrupt_go_before_the_queue_and_share_the_batch_limit() {
        let mailbox = FakeMailbox::default();
        *mailbox.queued.lock().unwrap() = vec![
            delivery(Kind::Queue, "a"),
            delivery(Kind::Steer, "b"),
            delivery(Kind::Interrupt, "c"),
        ];
        let first = next(&mailbox).unwrap().unwrap();
        assert_eq!(first.inbox, Inbox::Steer);
        assert!(first.interrupt);
        assert_eq!(
            first.taken,
            [delivery(Kind::Steer, "b"), delivery(Kind::Interrupt, "c")]
        );
        let second = next(&mailbox).unwrap().unwrap();
        assert_eq!(second.inbox, Inbox::Queue);
        assert!(!second.interrupt);
        assert_eq!(second.taken, [delivery(Kind::Queue, "a")]);
        assert_eq!(next(&mailbox).unwrap(), None);
        let limit = crate::profile::BATCH_LIMIT_BYTES;
        assert!(
            mailbox
                .takes
                .lock()
                .unwrap()
                .iter()
                .all(|(_, l)| *l == limit)
        );
    }

    #[tokio::test]
    async fn interrupt_stops_the_run_then_steers_the_body() {
        let api = FakeApi::default();
        let batch = Batch {
            taken: vec![delivery(Kind::Interrupt, "멈춰라")],
            inbox: Inbox::Steer,
            interrupt: true,
        };
        deliver(&api, &batch, Duration::ZERO).await.unwrap();
        let calls = api.calls.lock().unwrap();
        assert_eq!(calls[0], "interrupt");
        assert!(calls[1].starts_with("steer msg_"), "{}", calls[1]);
        assert!(calls[1].ends_with(
            "loom 메시지 #1 (보낸 노드: 부모, 방식: 중단. 하던 일을 멈추고 이 본문을 따른다)\n멈춰라"
        ));
        assert_eq!(calls.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_prompt_retries_with_the_same_id() {
        let api = FakeApi {
            fail: 2,
            ..FakeApi::default()
        };
        let batch = Batch {
            taken: vec![delivery(Kind::Queue, "a")],
            inbox: Inbox::Queue,
            interrupt: false,
        };
        deliver(&api, &batch, PROMPT_RETRY_DELAY).await.unwrap();
        let calls = api.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|c| *c == calls[0]), "{calls:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn prompt_gives_up_after_bounded_attempts() {
        let api = FakeApi {
            fail: u32::MAX,
            ..FakeApi::default()
        };
        let batch = Batch {
            taken: vec![delivery(Kind::Queue, "a")],
            inbox: Inbox::Queue,
            interrupt: false,
        };
        let err = deliver(&api, &batch, PROMPT_RETRY_DELAY).await.unwrap_err();
        assert_eq!(err, "연결이 끊겼다");
        assert_eq!(api.calls.lock().unwrap().len(), PROMPT_ATTEMPTS as usize);
    }

    #[test]
    fn message_ids_match_the_server_pattern_and_sort_by_time() {
        let a = message_id();
        let b = message_id();
        assert!(a.starts_with("msg_") && a.len() == 4 + 12 + 14, "{a}");
        assert!(a[4..16] < b[4..16], "{a} {b}");
    }

    #[test]
    fn version_warning_ignores_the_leading_v() {
        assert_eq!(version_warning("opencode v2.0.32\n"), None);
        assert_eq!(
            version_warning("opencode v2.0.33\n"),
            Some(VersionMismatch {
                installed: "2.0.33".into(),
                pinned: "2.0.32".into()
            })
        );
    }

    #[test]
    fn args_attach_the_tui_to_the_node_session() {
        assert_eq!(
            args("http://127.0.0.1:1", "ses_1"),
            ["--server", "http://127.0.0.1:1", "--session", "ses_1"]
        );
    }
}
