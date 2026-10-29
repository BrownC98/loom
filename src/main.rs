mod accounts;
mod choices;
mod cli;
mod commit_graph;
mod daemon;
mod db;
mod devices;
mod distro;
mod git;
mod graph;
mod launch;
mod link_text;
mod node_host;
mod open;
mod pake;
mod paste;
mod process_tree;
mod profile;
mod protocol;
mod roles;
mod screen;
mod search_path;
mod settings;
mod skills;
mod spots;
mod tailscale;
mod task;
mod timed;
mod tunnel;
mod update;
mod usage;
mod user_doc;
mod worktree;
mod wsl;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use crate::daemon::SpawnRun;
use crate::graph::Kept;
use crate::profile::HookEntry;
use crate::protocol::{Kind, ModelChoice};

#[derive(clap::Subcommand)]
enum RolesAction {
    /// 역할 하나의 에이전트·모델·강도를 바꾼다. 이름과 책임은 그대로 둔다. --model·--effort를 빼면 그 칸은 CLI 기본이 된다(설정 화면과 같다). 부모 없는 노드만 부른다. 바꾼 표를 출력한다.
    Set {
        role: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        effort: Option<String>,
    },
}

#[derive(Parser)]
#[command(name = "loom", version = protocol::VERSION)]
enum Cli {
    /// 데몬과 화면을 띄운다. 데이터 폴더마다 하나만 돈다.
    Serve {
        /// 데몬이 듣는 주소.
        #[arg(long, default_value_t = launch::DEFAULT_LISTEN)]
        listen: SocketAddr,
        #[arg(long, hide = true)]
        ready_to: Option<SocketAddr>,
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
    /// 자식 노드를 만든다. 지시문은 `--file`로 받는다.
    Spawn {
        /// 노드 이름. `--review`면 주지 않는다: loom이 `<역할> <브랜치>`로 짓는다.
        #[arg(required_unless_present = "review", conflicts_with = "review")]
        name: Option<String>,
        /// 새 워크트리의 브랜치. 주면 내 워크트리의 HEAD에서 이 브랜치로 워크트리를 만들고 자식을 그 안에 띄운다.
        #[arg(long)]
        worktree: Option<String>,
        /// 설정의 o-mode 역할(`loom roles`). 그 역할의 에이전트·모델·강도로 띄우고 지시문 앞에 책임을 붙인다.
        #[arg(long)]
        role: String,
        /// 자식이 검토할 브랜치. 내가 관문인 그 브랜치의 검토자로 띄운다. 그 브랜치의 판정은 이렇게 띄운 자식만 한다.
        #[arg(long, conflicts_with = "worktree")]
        review: Option<String>,
        /// 지시문 파일. `--review` 없이 만드는 노드는 꼭 준다.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// 설정의 o-mode 역할을 한 줄씩 출력한다: 이름, 에이전트·모델·강도, 책임.
    Roles {
        #[command(subcommand)]
        action: Option<RolesAction>,
    },
    /// 메모(종류 note)를 보낸다. 다른 종류(질문, 답, 완료, 관문)에 맞지 않는 메시지다. 종류 낱말(QUESTION, DONE, READY 등)로 시작하면 거절한다.
    Note {
        name: String,
        body: String,
        /// 일하는 노드의 지금 턴에 넣는다. 턴을 멈추지 않는다.
        #[arg(long, conflicts_with = "interrupt")]
        steer: bool,
        /// 노드가 하던 일을 멈추게 하고 메시지를 따르게 한다. 돌던 셸 명령은 끝낸다.
        #[arg(long)]
        interrupt: bool,
    },
    /// 받은 `loom ask` 메시지에 답한다(종류 answer). 묻는 노드가 `loom wait --reply-to`로 기다릴 수 있다.
    Answer { message: u32, body: String },
    /// 메시지가 올 때까지 기다렸다가 본문을 출력한다.
    Wait {
        /// 이 `loom ask` 메시지에 대한 답(종류 answer)만 찾는다. 이미 훅이 받은 답도 찾는다.
        #[arg(long)]
        reply_to: Option<u32>,
        /// 이 초 안에 답이 없으면 실패한다.
        #[arg(long = "timeout")]
        timeout_secs: Option<u64>,
    },
    /// 내 번호, 현재 부모와 자식, 프로세스 생존 여부를 출력한다.
    Status {
        /// 그래프의 모든 노드를 번호, 이름, 부모, 자리, 상태, 생존 여부로 출력한다.
        #[arg(long)]
        all: bool,
    },
    /// 결과(끝난 것이나 중간 것)를 부모에게 보내고(종류 done) 이 노드를 done으로 바꾼다. 부모가 없으면 사람의 받은편지함에 완료를 건다.
    /// 관문이 자신인 노드의 브랜치가 병합되지 않았고 `loom ready`도 없으면 경고를 붙인다(거절하지 않는다).
    Done {
        #[arg(required_unless_present = "file")]
        summary: Option<String>,
        /// 결과 파일. 부모에게는 경로가 `file` 칸으로 간다.
        #[arg(long)]
        file: Option<PathBuf>,
        /// 그 일을 맡긴 사람의 지시(짧게). 부모 없는 노드는 꼭 준다. 받은편지함 완료 줄에 보인다.
        #[arg(long)]
        asked: Option<String>,
        /// 사람이 그 지시에 준 그림 파일. 여러 번 준다. 받은편지함 완료 줄에 작은 그림으로 보인다.
        #[arg(long = "image")]
        images: Vec<PathBuf>,
        /// 사람이 열 문서(`show.sh`를 지난 user-doc HTML). 받은편지함 완료 줄에 링크로 보인다. 부모 없는 노드만 쓴다.
        #[arg(long)]
        doc: Option<PathBuf>,
        /// 일을 끝내고 프로세스가 끝난 자식을 일부러 남긴다: `<이름>=<이유>`. 여러 번 준다. 이유는 보고 끝에 `Kept:`로 붙는다. 그런 자식이 남으면 `loom done`은 거부된다.
        #[arg(long, value_parser = cli::kept)]
        keep: Vec<Kept>,
    },
    /// 질문 한 줄을 묻는다(종류 ask). 부모 없는 노드는 사람에게 묻는다: 받은편지함에 번호(`Q12`)와 함께 뜨고, 사람은 이 노드 터미널이나 받은편지함에서 답한다.
    /// 부모 있는 노드는 부모에게 묻는다. 답은 종류 answer 메시지로 온다.
    Ask {
        question: String,
        /// 질문에 붙일 문서(`show.sh`를 지난 user-doc 목업·보고서 HTML). 사람은 질문 줄의 링크로 연다.
        #[arg(long)]
        doc: Option<PathBuf>,
    },
    /// 사람의 입력이 열린 `loom ask` 질문의 답이다. 받은편지함에서 그 질문을 지운다. 열린 질문이 여럿이면 번호(`Q12 Q13`)를 준다
    Answered { ids: Vec<String> },
    /// 병합 관문: 자기 브랜치를 관문에 보낸다(종류 ready). 워크트리가 그 브랜치에 있는 담당만 부른다. 커밋하지 않은 변경이 없어야 한다.
    /// 병합 큐가 가져가기 전의 열린 차례는 새 커밋의 `loom ready`가 갈음한다. 앉은 검토자는 새 차례를 받는다.
    /// 관문은 loom이 고른다: 부모 워크트리가 원래 체크아웃의 브랜치가 아닌 브랜치에 있으면 부모(대상은 그 브랜치), 아니면 담당 자신(대상은 원래 체크아웃의 브랜치).
    Ready {
        branch: String,
        /// 원래 체크아웃이 대상이면 이 브랜치의 병합 커밋 메시지에 `[skip ci]`를 붙여 원격 CI(배포 포함)를 건너뛴다. 큐는 이 브랜치를 배포하는 브랜치와 한 묶음에 넣지 않는다. `scripts/gate/ship.sh`는 그대로 돈다.
        #[arg(long)]
        no_deploy: bool,
    },
    /// 병합 관문: 판정을 넣는다. 원래 체크아웃이 대상이면 그 브랜치의 검토자만 부른다. 데몬이 `loom ready` 때 검토자 역할마다 하나씩 관문의 자식으로 띄운다. 부모 브랜치에서 만든 브랜치는 검토자 없이 부모가 완료 조건을 보고 부른다. block은 결함 파일이 꼭 있고 담당에게 반려로 간다.
    /// pass는 브랜치를 데몬의 병합 큐에 세운다. 큐가 쌓고, 저장소에 `scripts/gate/check.sh`가 있으면 그것으로 검사하고, 병합(main이면 push)하고, merged·rework·failed로 알린다. main이고 `scripts/gate/ship.sh`가 있으면 그 뒤 그것이 따로 배포를 확인하고 실패만 루트에 알린다.
    Verdict {
        branch: String,
        /// 판정한 커밋. loom이 검토자에게 보낸 차례 메시지의 `head`를 적는다(앞 7자 이상). 열린 차례의 커밋이 아니면 거절한다.
        head: String,
        judgment: protocol::Judgment,
        /// 결함 파일. block이면 꼭 준다.
        #[arg(long)]
        findings: Option<PathBuf>,
    },
    /// 이 노드가 `name` 노드의 새 부모가 된다. 그 노드가 보관한 메시지를 받는다.
    Adopt { name: String },
    /// `name` 노드(이름이나 `#번호`)의 부모를 비워 다시 뿌리 노드로 만든다. 그 노드의 이후 질문은 사람 받은편지함으로 간다.
    Detach { name: String },
    /// 노드(이름이나 `#번호`)를 지우고 프로세스를 끝낸다. 그 노드의 자식은 이 노드가 데려간다.
    Rm { name: String },
    /// 에이전트 프로세스를 지금 설정으로 다시 띄우고 같은 대화를 잇는다. 턴 중이면 그 턴이 끝난 뒤 띄운다.
    /// 대상이 없으면 이 노드 자신이고, 있으면 어느 트리의 어느 노드든 된다(이름이나 `#번호`). 받은 메시지와 열린 질문은 남는다.
    /// `--agent`를 주면 그 에이전트의 새 세션이 노드를 이어받는다. 번호·이름·역할·부모·자식·메일함·열린 질문은 남는다.
    Relaunch {
        name: Option<String>,
        /// 바꿀 카탈로그 에이전트 id. 새 세션은 옛 세션 기록 경로를 첫 입력으로 받는다.
        #[arg(long)]
        agent: Option<String>,
        /// 새 에이전트의 모델. 없으면 노드 역할의 에이전트가 같을 때 역할의 모델, 아니면 CLI 설정의 모델이다.
        #[arg(long, requires = "agent")]
        model: Option<String>,
        /// 새 에이전트의 추론 강도. 없으면 `--model`과 같은 규칙이다.
        #[arg(long, requires = "agent")]
        effort: Option<String>,
    },
    /// user-doc 문서(HTML)를 점검하고 문서 ID를 붙인다. 브라우저는 열지 않는다. 사람에게는 `loom ask --doc`으로 보인다.
    Show {
        file: PathBuf,
        /// `show.sh`가 렌더·말풍선 점검 뒤에 부른다. 지금 내용을 `--doc` 통과로 남긴다.
        #[arg(long, hide = true)]
        checked: bool,
    },
    /// loom이 에이전트에 건 훅과 확장이 부른다. stdin의 훅 입력을 받아 넘길 메시지를 출력한다.
    Hook {
        /// 훅을 건 에이전트 진입점.
        entry: HookEntry,
    },
    /// 훅이 셸 명령 앞에 붙인다. 부른 셸을 중단 대상으로 알린다.
    Guard,
    /// 데몬이 노드마다 띄운다. PTY 하나를 쥐고 데몬보다 오래 산다.
    #[command(hide = true)]
    PtyHost,
    /// Windows 데몬이 WSL 배포판마다 하나 띄운다. 그 배포판의 노드 pty-host를 띄우고 잇는다.
    #[command(hide = true)]
    DistroHost {
        /// 배포판 호스트가 배포판 안에서 띄운다. stdin·stdout의 스트림이 Windows 쪽 배포판 호스트로 간다.
        #[arg(long)]
        inner: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    take_shell_token();
    let result = match cli {
        Cli::Serve { listen, ready_to } => {
            forget_launcher_env();
            tokio::runtime::Runtime::new()
                .and_then(|rt| rt.block_on(daemon::serve(listen, ready_to)))
                .map_err(|e| e.to_string())
        }
        Cli::Start { listen } => launch::start(listen),
        Cli::Open { listen } => launch::open(listen),
        Cli::Stop { listen } => launch::stop(listen),
        Cli::Carry { from } => launch::carry(&wsl::from_caller(from)),
        Cli::Spawn {
            name,
            worktree,
            role,
            review,
            file,
        } => cli::spawn(
            match (name, review) {
                (_, Some(branch)) => SpawnRun::Review { role, branch },
                (Some(name), None) => SpawnRun::Role { name, role },
                (None, None) => unreachable!("clap requires a name without --review"),
            },
            worktree,
            file.map(wsl::from_caller),
        ),
        Cli::Roles { action: None } => cli::roles(),
        Cli::Roles {
            action:
                Some(RolesAction::Set {
                    role,
                    agent,
                    model,
                    effort,
                }),
        } => cli::set_role(roles::SetRole {
            role,
            agent: protocol::AgentId(agent),
            model,
            effort,
        }),
        Cli::Note {
            name,
            body,
            steer,
            interrupt,
        } => {
            let mode = match (steer, interrupt) {
                (true, _) => Kind::Steer,
                (_, true) => Kind::Interrupt,
                _ => Kind::Queue,
            };
            cli::note(name, mode, body)
        }
        Cli::Answer { message, body } => cli::answer(message, body),
        Cli::Hook { entry } => cli::hook(entry),
        Cli::Guard => cli::guard(),
        Cli::Wait {
            reply_to,
            timeout_secs,
        } => cli::wait(reply_to, timeout_secs),
        Cli::Status { all: false } => cli::status(),
        Cli::Status { all: true } => cli::directory(),
        Cli::Done {
            summary,
            file,
            asked,
            images,
            doc,
            keep,
        } => cli::done(
            summary,
            file.map(wsl::from_caller),
            asked,
            images.into_iter().map(wsl::from_caller).collect(),
            doc.map(wsl::from_caller),
            keep,
        ),
        Cli::Ask { question, doc } => cli::ask(question, doc.map(wsl::from_caller)),
        Cli::Answered { ids } => cli::answered(ids),
        Cli::Ready { branch, no_deploy } => cli::ready(
            branch,
            match no_deploy {
                true => protocol::Deploy::Skip,
                false => protocol::Deploy::Run,
            },
        ),
        Cli::Verdict {
            branch,
            head,
            judgment,
            findings,
        } => cli::verdict(branch, head, judgment, findings.map(wsl::from_caller)),
        Cli::Adopt { name } => cli::adopt(name),
        Cli::Detach { name } => cli::detach(name),
        Cli::Rm { name } => cli::remove(name),
        Cli::Relaunch {
            name,
            agent,
            model,
            effort,
        } => cli::relaunch(name, agent, ModelChoice { model, effort }),
        Cli::Show {
            file,
            checked: false,
        } => user_doc::show(&wsl::from_caller(file)),
        Cli::Show {
            file,
            checked: true,
        } => user_doc::record_checked(&wsl::from_caller(file)),
        Cli::PtyHost => node_host::run(),
        Cli::DistroHost { inner: false } => distro::outer(),
        Cli::DistroHost { inner: true } => distro::inner(node_host::start_in_distro),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("loom: {e}");
            ExitCode::FAILURE
        }
    }
}

fn take_shell_token() {
    if std::env::var_os(open::NODE_TOKEN_ENV).is_some() {
        return;
    }
    if let Some(token) = profile::SHELL_TOKEN_ENVS.iter().find_map(std::env::var_os) {
        unsafe { std::env::set_var(open::NODE_TOKEN_ENV, token) };
    }
}

fn forget_launcher_env() {
    let keys: Vec<_> = std::env::vars_os()
        .filter_map(|(key, _)| key.into_string().ok())
        .filter(|key| profile::is_launcher_env(key))
        .collect();
    for key in keys {
        unsafe { std::env::remove_var(key) };
    }
}
