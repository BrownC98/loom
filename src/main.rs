mod accounts;
mod cli;
mod codex_server;
mod commit_graph;
mod daemon;
mod db;
mod devices;
#[cfg(all(test, unix))]
mod fake_exe;
mod git;
mod graph;
mod launch;
mod node_host;
mod open;
mod pake;
mod paste;
mod process_tree;
mod profile;
mod roles;
mod screen;
mod settings;
mod skills;
mod spots;
mod tailscale;
mod update;
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
    /// 데몬과 화면을 띄운다. 데이터 폴더마다 하나만 돈다.
    Serve {
        /// 데몬이 듣는 주소.
        #[arg(long, default_value_t = launch::DEFAULT_LISTEN)]
        listen: SocketAddr,
    },
    /// 데몬이 있으면 붙고 없으면 띄운다. 도는 데몬이 옛 판이면 이 판으로 바꾼다. 주소를 `loom: <주소>`로 출력한다. 데스크톱 앱이 부른다.
    Start {
        /// 데몬 주소. 없으면 지난 데몬의 포트, 처음이면 7420이다.
        #[arg(long)]
        listen: Option<SocketAddr>,
    },
    /// `start` 뒤 화면을 기본 브라우저로 연다. 바탕 화면 아이콘이 부른다.
    Open {
        /// 데몬 주소. 없으면 지난 데몬의 포트, 처음이면 7420이다.
        #[arg(long)]
        listen: Option<SocketAddr>,
    },
    /// 데몬만 끝낸다. 노드는 살고, 다음 `start`·`open`·`serve`가 다시 붙는다.
    Stop {
        /// `daemon.json`이 없을 때 끝낼 데몬 주소.
        #[arg(long)]
        listen: Option<SocketAddr>,
    },
    /// 옛 WSL 데몬의 저장소·노드·설정을 이 Windows 데몬으로 옮기고 에이전트 노드를 같은 세션으로 다시 띄운다. `scripts/host.sh`가 부른다.
    Carry {
        /// 옛 데몬의 데이터 폴더(WSL `~/.loom`).
        from: PathBuf,
    },
    /// 자식 노드를 만든다. 지시문은 stdin이나 `--file`로 받는다.
    Spawn {
        name: String,
        /// 새 워크트리의 브랜치. 주면 내 워크트리의 HEAD에서 이 브랜치로 워크트리를 만들고 자식을 그 안에 띄운다.
        #[arg(long)]
        worktree: Option<String>,
        /// 카탈로그 에이전트 id(예: `claude`). 그 프로필과 계정으로 띄운다. `--role`을 주면 없어도 된다.
        #[arg(long, conflicts_with = "command", required_unless_present_any = ["command", "role"])]
        agent: Option<String>,
        /// 설정의 o-mode 역할(`loom roles`). 그 역할의 에이전트·모델·강도로 띄우고 지시문 앞에 책임을 붙인다. 따로 준 `--agent`·`--model`·`--effort`가 이긴다.
        #[arg(long, conflicts_with = "command")]
        role: Option<String>,
        /// 지시문 파일. 없으면 stdin을 읽는다(stdin이 터미널이면 지시문 없이 만든다).
        #[arg(long)]
        file: Option<PathBuf>,
        /// 모델. 없으면 CLI 설정의 모델로 뜬다. Claude·Codex만 받는다.
        #[arg(long, conflicts_with = "command")]
        model: Option<String>,
        /// 추론 강도(예: `medium`). 없으면 CLI 설정의 강도로 뜬다. Claude·Codex만 받는다.
        #[arg(long, conflicts_with = "command")]
        effort: Option<String>,
        /// 자식이 실행할 명령. ssh처럼 낱말을 빈칸으로 이어 셸이 해석한다.
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// 설정의 o-mode 역할을 한 줄씩 출력한다: 이름, 에이전트·모델·강도, 책임.
    Roles,
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
    Status {
        /// 그래프의 모든 노드를 번호, 이름, 부모, 자리, 상태, 생존 여부로 출력한다.
        #[arg(long)]
        all: bool,
    },
    /// 결과를 부모에게 보내고 이 노드를 done으로 바꾼다. 부모가 없으면 사람의 받은편지함에 완료를 건다.
    Done {
        #[arg(required_unless_present = "file")]
        summary: Option<String>,
        /// 결과 파일. 부모에게는 경로만 간다.
        #[arg(long, conflicts_with = "summary")]
        file: Option<PathBuf>,
        /// 그 일을 맡긴 사람의 지시(짧게). 부모 없는 노드는 꼭 준다. 받은편지함 완료 줄에 보인다.
        #[arg(long)]
        asked: Option<String>,
        /// 사람이 그 지시에 준 그림 파일. 여러 번 준다. 받은편지함 완료 줄에 작은 그림으로 보인다.
        #[arg(long = "image")]
        images: Vec<PathBuf>,
        /// 사람이 열 문서(user-doc HTML, `.md` 같은 글 파일). 받은편지함 완료 줄에 링크로 보인다. 부모 없는 노드만 쓴다.
        #[arg(long)]
        doc: Option<PathBuf>,
    },
    /// 사람에게 질문 한 줄을 묻는다. 받은편지함에 번호(`Q12`)와 함께 뜨고, 사람은 이 노드 터미널이나 받은편지함에서 답한다. 사람이 띄운 노드만 쓴다.
    Ask {
        question: String,
        /// 질문에 붙일 문서(user-doc 목업·보고서 HTML, `.md` 같은 글 파일). 사람은 질문 줄의 링크로 연다.
        #[arg(long)]
        doc: Option<PathBuf>,
    },
    /// 사람의 입력이 열린 `loom ask` 질문의 답이다. 받은편지함에서 그 질문을 지운다. 열린 질문이 여럿이면 번호(`Q12 Q13`)를 준다
    Answered { ids: Vec<String> },
    /// 이 노드가 `name` 노드의 새 부모가 된다. 그 노드가 보관한 메시지를 받는다.
    Adopt { name: String },
    /// 자기 자식 노드(이름이나 `#번호`)를 지우고 프로세스를 끝낸다. 그 자식의 자식은 이 노드가 데려간다.
    Rm { name: String },
    /// 에이전트 프로세스를 지금 설정으로 다시 띄우고 같은 대화를 잇는다. 턴 중이면 그 턴이 끝난 뒤 띄운다.
    /// 이름이 없으면 이 노드 자신이고, 있으면 자기 자식이다. 받은 메시지와 열린 질문은 남는다.
    Relaunch { name: Option<String> },
    /// user-doc 문서(HTML)를 점검하고 문서 ID를 붙인다. 브라우저는 열지 않는다. 사람에게는 `loom ask --doc`으로 보인다.
    Show { file: PathBuf },
    /// Claude 훅이 부른다. stdin의 훅 입력을 받아 넘길 메시지를 출력한다.
    Hook,
    /// Cursor 훅이 부른다. stdin의 훅 입력을 받아 넘길 메시지를 출력한다.
    CursorHook,
    /// Cursor 세션 시작 훅이 부른다. 상시 지시를 출력한다.
    CursorRules,
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
        Cli::Start { listen } => launch::start(listen),
        Cli::Open { listen } => launch::open(listen),
        Cli::Stop { listen } => launch::stop(listen),
        Cli::Carry { from } => launch::carry(&wsl::from_caller(from)),
        Cli::Spawn {
            name,
            worktree,
            agent,
            role,
            file,
            model,
            effort,
            command,
        } => cli::spawn(
            name,
            worktree,
            agent,
            role,
            file.map(wsl::from_caller),
            ModelChoice { model, effort },
            command,
        ),
        Cli::Roles => cli::roles(),
        Cli::Send { name, body } => cli::send(name, Kind::Queue, body),
        Cli::Steer { name, body } => cli::send(name, Kind::Steer, body),
        Cli::Interrupt { name, body } => cli::send(name, Kind::Interrupt, body),
        Cli::Hook => cli::hook(),
        Cli::CursorHook => cli::cursor_hook(),
        Cli::CursorRules => cli::cursor_rules(),
        Cli::Guard => cli::guard(),
        Cli::Wait {
            reply_to,
            timeout_secs,
            json,
        } => cli::wait(reply_to, timeout_secs, json),
        Cli::Reply { message, body } => cli::reply(message, body),
        Cli::Status { all: false } => cli::status(),
        Cli::Status { all: true } => cli::directory(),
        Cli::Done {
            summary,
            file,
            asked,
            images,
            doc,
        } => cli::done(
            summary,
            file.map(wsl::from_caller),
            asked,
            images.into_iter().map(wsl::from_caller).collect(),
            doc.map(wsl::from_caller),
        ),
        Cli::Ask { question, doc } => cli::ask(question, doc.map(wsl::from_caller)),
        Cli::Answered { ids } => cli::answered(ids),
        Cli::Adopt { name } => cli::adopt(name),
        Cli::Rm { name } => cli::remove(name),
        Cli::Relaunch { name } => cli::relaunch(name),
        Cli::Show { file } => user_doc::show(&wsl::from_caller(file)),
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
