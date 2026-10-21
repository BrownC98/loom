mod accounts;
mod cli;
mod codex_server;
mod commit_graph;
mod daemon;
mod db;
mod git;
mod graph;
mod node_host;
mod open;
mod paste;
mod process_tree;
mod profile;
mod settings;
mod skills;
mod spots;
mod usage;
mod user_doc;
mod worktree;
mod wsl;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use crate::graph::Kind;
use crate::profile::ModelChoice;

/// 제품 판. `scripts/host.sh`가 `scripts/version.sh` 값을 `LOOM_VERSION`으로 넣는다.
/// 릴리스 CI는 Cargo.toml 판을 바꿔 빌드한다. 근거: docs/design.md 「배포」.
const VERSION: &str = match option_env!("LOOM_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(name = "loom", version = VERSION)]
enum Cli {
    /// 데몬과 화면을 띄운다.
    Serve {
        /// 데몬이 듣는 주소. 사용자는 한 명이므로 기본값은 루프백이다.
        #[arg(long, default_value = "127.0.0.1:7420")]
        listen: SocketAddr,
    },
    /// 자식 노드를 만든다. 지시문은 stdin이나 `--file`로 받는다.
    Spawn {
        name: String,
        /// 새 워크트리의 브랜치. 주면 내 워크트리의 HEAD에서 이 브랜치로 워크트리를 만들고 자식을 그 안에 띄운다.
        #[arg(long)]
        worktree: Option<String>,
        /// 카탈로그 에이전트 id(예: `claude`). 그 프로필과 계정으로 띄운다.
        #[arg(long, conflicts_with = "command", required_unless_present = "command")]
        agent: Option<String>,
        /// 지시문 파일. 없으면 stdin을 읽는다(stdin이 터미널이면 지시문 없이 만든다).
        #[arg(long)]
        file: Option<PathBuf>,
        /// 모델. 없으면 CLI 설정의 모델로 뜬다. Claude·Codex만 받는다.
        #[arg(long, requires = "agent")]
        model: Option<String>,
        /// 추론 강도(예: `medium`). 없으면 CLI 설정의 강도로 뜬다. Claude·Codex만 받는다.
        #[arg(long, requires = "agent")]
        effort: Option<String>,
        /// 자식이 실행할 명령. ssh처럼 낱말을 빈칸으로 이어 셸이 해석한다.
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// 부모나 자식에게 메시지를 보낸다. 받는 노드의 턴을 끊지 않는다.
    Send { name: String, body: String },
    /// 일하는 Claude·Codex·OpenCode 2 노드의 지금 턴에 메시지를 넣는다. 턴을 멈추지 않는다.
    Steer { name: String, body: String },
    /// Claude·Codex·OpenCode 2 노드가 하던 일을 멈추게 하고 메시지를 따르게 한다. 돌던 셸 명령은 끝낸다.
    Interrupt { name: String, body: String },
    /// 메시지가 올 때까지 기다렸다가 본문을 출력한다.
    Wait {
        /// 이 메시지에 대한 답변만 찾는다. 이미 훅이 받은 답도 찾는다.
        #[arg(long)]
        reply_to: Option<u32>,
        /// 이 초 안에 답이 없으면 실패한다.
        #[arg(long = "timeout")]
        timeout_secs: Option<u64>,
        /// 메시지 번호와 답변 대상을 JSON으로 출력한다.
        #[arg(long)]
        json: bool,
    },
    /// 받은 메시지 번호를 지정해 보낸 노드에게 답한다.
    Reply { message: u32, body: String },
    /// 내 번호, 현재 부모와 자식, 프로세스 생존 여부를 출력한다.
    Status,
    /// 결과를 부모에게 보내고 이 노드를 done으로 바꾼다.
    Done {
        #[arg(required_unless_present = "file")]
        summary: Option<String>,
        /// 결과 파일. 부모에게는 경로만 간다.
        #[arg(long, conflicts_with = "summary")]
        file: Option<PathBuf>,
    },
    /// 이 노드가 `name` 노드의 새 부모가 된다. 그 노드가 보관한 메시지를 받는다.
    Adopt { name: String },
    /// 자기 자식 노드(이름이나 `#번호`)를 지우고 프로세스를 끝낸다. 그 자식의 자식은 이 노드가 데려간다.
    Rm { name: String },
    /// user-doc 문서(HTML)를 점검하고 문서 ID를 붙여 기본 브라우저로 연다.
    Show { file: PathBuf },
    /// Claude 훅이 부른다. stdin의 훅 입력을 받아 넘길 메시지를 출력한다.
    Hook,
    /// Claude 훅이 Bash 명령 앞에 붙인다. 부른 셸을 중단 대상으로 알린다.
    Guard,
    /// 데몬이 노드마다 띄운다. PTY 하나를 쥐고 데몬보다 오래 산다.
    #[command(hide = true)]
    PtyHost,
}

fn main() -> ExitCode {
    let result = match Cli::parse() {
        Cli::Serve { listen } => {
            forget_launcher_env();
            tokio::runtime::Runtime::new()
                .and_then(|rt| rt.block_on(daemon::serve(listen)))
                .map_err(|e| e.to_string())
        }
        Cli::Spawn {
            name,
            worktree,
            agent,
            file,
            model,
            effort,
            command,
        } => cli::spawn(
            name,
            worktree,
            agent,
            file,
            ModelChoice { model, effort },
            command,
        ),
        Cli::Send { name, body } => cli::send(name, Kind::Queue, body),
        Cli::Steer { name, body } => cli::send(name, Kind::Steer, body),
        Cli::Interrupt { name, body } => cli::send(name, Kind::Interrupt, body),
        Cli::Hook => cli::hook(),
        Cli::Guard => cli::guard(),
        Cli::Wait {
            reply_to,
            timeout_secs,
            json,
        } => cli::wait(reply_to, timeout_secs, json),
        Cli::Reply { message, body } => cli::reply(message, body),
        Cli::Status => cli::status(),
        Cli::Done { summary, file } => cli::done(summary, file),
        Cli::Adopt { name } => cli::adopt(name),
        Cli::Rm { name } => cli::remove(name),
        Cli::Show { file } => user_doc::show(&file),
        Cli::PtyHost => node_host::run(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("loom: {e}");
            ExitCode::FAILURE
        }
    }
}

/// 데몬을 띄운 터미널의 변수를 지운다(`profile::is_launcher_env`). 노드, Codex app-server, 사용량 조회가 모두 이 환경을 물려받는다.
fn forget_launcher_env() {
    let keys: Vec<_> = std::env::vars_os()
        .filter_map(|(key, _)| key.into_string().ok())
        .filter(|key| profile::is_launcher_env(key))
        .collect();
    for key in keys {
        // SAFETY: 런타임을 만들기 전이라 다른 스레드가 없다.
        unsafe { std::env::remove_var(key) };
    }
}
