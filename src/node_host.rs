//! 노드 호스트: 노드마다 PTY 하나를 쥐고, 출력을 보는 화면 모두에 나눠 준다.
//! 노드 이름·상태는 모른다. 그것은 graph가 갖는다.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, RwLock};

use bytes::Bytes;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::broadcast;

use crate::graph::NodeId;

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

#[derive(Default)]
pub struct NodeHost {
    nodes: RwLock<HashMap<NodeId, Arc<Pty>>>,
}

struct Pty {
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: Mutex<Box<dyn Write + Send>>,
    output: Mutex<Output>,
}

struct Output {
    backlog: VecDeque<u8>,
    tx: broadcast::Sender<Bytes>,
}

impl NodeHost {
    /// 명령을 PTY에서 띄운다. 프로세스가 끝나면 `on_exit`에 종료 코드를 넘긴다.
    pub fn spawn(
        &self,
        id: NodeId,
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
    pub fn attach(&self, id: NodeId) -> Option<Attached> {
        let pty = self.pty(id)?;
        let output = pty.output.lock().unwrap();
        Some(Attached {
            backlog: Bytes::from(output.backlog.iter().copied().collect::<Vec<u8>>()),
            output: output.tx.subscribe(),
        })
    }

    pub fn write(&self, id: NodeId, bytes: &[u8]) {
        if let Some(pty) = self.pty(id) {
            // 끝난 프로세스에 쓰면 실패한다. 화면에 따로 알리지 않는다. 상태 이벤트가 exited를 보인다.
            let _ = pty.input.lock().unwrap().write_all(bytes);
        }
    }

    pub fn resize(&self, id: NodeId, cols: u16, rows: u16) {
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

    fn pty(&self, id: NodeId) -> Option<Arc<Pty>> {
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
