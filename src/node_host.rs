//! 노드 호스트: 키마다 PTY 하나를 쥐고, 출력을 보는 화면 모두에 나눠 준다.
//! 노드 이름·상태는 모른다. 그것은 graph가 갖는다. 키는 노드 id나 로그인 창 id다.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, RwLock};

use bytes::Bytes;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::broadcast;

use crate::process_tree::{self, Snapshot};

/// 다시 연 카드에 먼저 보내는 출력의 상한. 근거: docs/design.md 「규모」.
const BACKLOG_BYTES: usize = 256 * 1024;
/// 화면 하나가 밀려도 버티는 출력 조각 수. 넘으면 그 화면의 연결을 끊고, 화면은 다시 붙어 backlog를 받는다.
const VIEWER_QUEUE_CHUNKS: usize = 1024;
/// 화면이 크기를 보내기 전에 쓰는 PTY 크기.
const INITIAL_SIZE: PtySize = PtySize {
    rows: 24,
    cols: 80,
    pixel_width: 0,
    pixel_height: 0,
};

pub struct Attached {
    pub backlog: Bytes,
    pub output: broadcast::Receiver<Bytes>,
}

pub struct NodeHost<K> {
    nodes: RwLock<HashMap<K, Arc<Pty>>>,
}

impl<K> Default for NodeHost<K> {
    fn default() -> Self {
        Self {
            nodes: RwLock::default(),
        }
    }
}

struct Pty {
    /// PTY에서 띄운 프로세스. 중단할 셸이 이 프로세스의 자손인지 볼 때 쓴다.
    pid: Option<u32>,
    /// 중단이 오면 끝낼 셸(`loom guard`가 알린 Bash 도구 셸).
    guarded: Mutex<Vec<u32>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: Mutex<Box<dyn Write + Send>>,
    output: Mutex<Output>,
}

struct Output {
    backlog: VecDeque<u8>,
    tx: broadcast::Sender<Bytes>,
}

impl<K: Copy + Eq + Hash> NodeHost<K> {
    /// 명령을 PTY에서 띄운다. 프로세스가 끝나면 `on_exit`에 종료 코드를 넘긴다.
    pub fn spawn(
        &self,
        id: K,
        cmd: CommandBuilder,
        on_exit: impl FnOnce(u32) + Send + 'static,
    ) -> Result<(), String> {
        let pair = native_pty_system()
            .openpty(INITIAL_SIZE)
            .map_err(|e| e.to_string())?;
        let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let input = pair.master.take_writer().map_err(|e| e.to_string())?;
        let pty = Arc::new(Pty {
            pid: child.process_id(),
            guarded: Mutex::default(),
            killer: Mutex::new(child.clone_killer()),
            master: Mutex::new(pair.master),
            input: Mutex::new(input),
            output: Mutex::new(Output {
                backlog: VecDeque::with_capacity(BACKLOG_BYTES),
                tx: broadcast::channel(VIEWER_QUEUE_CHUNKS).0,
            }),
        });
        self.nodes.write().unwrap().insert(id, pty.clone());

        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                pty.output.lock().unwrap().push(&buf[..n]);
            }
        });
        std::thread::spawn(move || {
            let code = child.wait().map(|s| s.exit_code()).unwrap_or(u32::MAX);
            on_exit(code);
        });
        Ok(())
    }

    /// backlog와 그 뒤의 출력을 빈틈없이 받는다.
    pub fn attach(&self, id: K) -> Option<Attached> {
        let pty = self.pty(id)?;
        let output = pty.output.lock().unwrap();
        Some(Attached {
            backlog: Bytes::from(output.backlog.iter().copied().collect::<Vec<u8>>()),
            output: output.tx.subscribe(),
        })
    }

    pub fn write(&self, id: K, bytes: &[u8]) {
        if let Some(pty) = self.pty(id) {
            // 끝난 프로세스에 쓰면 실패한다. 화면에 따로 알리지 않는다. 상태 이벤트가 exited를 보인다.
            let _ = pty.input.lock().unwrap().write_all(bytes);
        }
    }

    pub fn resize(&self, id: K, cols: u16, rows: u16) {
        if let Some(pty) = self.pty(id) {
            let size = PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            };
            let _ = pty.master.lock().unwrap().resize(size);
        }
    }

    /// 프로세스를 끝내고 PTY를 놓는다. `spawn`의 `on_exit`는 그대로 불린다.
    pub fn kill(&self, id: K) {
        if let Some(pty) = self.nodes.write().unwrap().remove(&id) {
            // 이미 끝난 프로세스면 실패한다. 할 일이 없다.
            let _ = pty.killer.lock().unwrap().kill();
        }
    }

    /// 노드 안의 셸 하나를 중단 대상으로 둔다. 이미 끝난 셸은 이때 목록에서 지운다.
    pub fn guard(&self, id: K, shell: u32) {
        let Some(pty) = self.pty(id) else { return };
        let mut shells = pty.guarded.lock().unwrap();
        shells.push(shell);
        process_tree::alive(&mut shells);
    }

    /// 중단 대상 셸과 그 자손 프로세스를 모두 끝내고, 끝낸 프로세스 수를 돌려준다.
    /// 노드 프로세스의 자손인 셸만 끝낸다. 셸이 끝난 뒤 그 PID를 다른 프로세스가 받았을 수 있다.
    pub fn stop_guarded(&self, id: K) -> usize {
        let Some(pty) = self.pty(id) else { return 0 };
        let shells = std::mem::take(&mut *pty.guarded.lock().unwrap());
        let Some(root) = pty.pid.filter(|_| !shells.is_empty()) else {
            return 0;
        };
        let snapshot = Snapshot::now();
        snapshot.kill_trees(
            shells
                .into_iter()
                .filter(|&shell| snapshot.descends(shell, root)),
        )
    }

    fn pty(&self, id: K) -> Option<Arc<Pty>> {
        self.nodes.read().unwrap().get(&id).cloned()
    }
}

impl Output {
    fn push(&mut self, chunk: &[u8]) {
        let overflow = (self.backlog.len() + chunk.len()).saturating_sub(BACKLOG_BYTES);
        self.backlog.drain(..overflow.min(self.backlog.len()));
        let keep = chunk.len().min(BACKLOG_BYTES);
        self.backlog.extend(&chunk[chunk.len() - keep..]);
        // 보는 화면이 없으면 send가 실패한다. backlog에 남았으므로 버려도 된다.
        let _ = self.tx.send(Bytes::copy_from_slice(chunk));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn stop_guarded_ends_the_shell_and_its_children() {
        let host = NodeHost::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut cmd = CommandBuilder::new("sh");
        // 노드(sh) 안의 셸(sh)이 자식 둘을 띄운다. 셸이 자기 PID를 출력한다.
        cmd.args(["-c", "sh -c 'echo $$; sleep 30 & sleep 30' ; sleep 30"]);
        host.spawn(1, cmd, move |code| tx.send(code).unwrap())
            .unwrap();
        let mut output = host.attach(1).unwrap();
        let mut text = String::from_utf8_lossy(&output.backlog).into_owned();
        while !text.contains('\n') {
            let chunk = output.output.blocking_recv().unwrap();
            text.push_str(&String::from_utf8_lossy(&chunk));
        }
        let shell: u32 = text.trim().parse().unwrap();
        // 다른 노드의 셸로 알리면 끝내지 않는다.
        host.guard(1, std::process::id());
        host.guard(1, shell);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(host.stop_guarded(1), 3);
        assert_eq!(host.stop_guarded(1), 0);
        host.kill(1);
        rx.recv().unwrap();
    }

    #[test]
    fn backlog_keeps_only_the_newest_bytes() {
        let mut out = Output {
            backlog: VecDeque::new(),
            tx: broadcast::channel(1).0,
        };
        out.push(&vec![b'a'; BACKLOG_BYTES]);
        out.push(b"xyz");
        assert_eq!(out.backlog.len(), BACKLOG_BYTES);
        assert_eq!(
            out.backlog
                .iter()
                .rev()
                .take(4)
                .copied()
                .collect::<Vec<_>>(),
            b"zyxa"
        );
    }
}
