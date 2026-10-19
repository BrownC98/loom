mod accounts;
mod cli;
mod codex_server;
mod commit_graph;
mod daemon;
mod db;
mod graph;
mod node_host;
mod process_tree;
mod profile;
mod settings;
mod usage;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use crate::graph::Kind;

#[derive(Parser)]
#[command(name = "loom", version)]
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
        /// 카탈로그 에이전트 id(예: `claude`). 그 프로필과 계정으로 띄운다.
        #[arg(long, conflicts_with = "command", required_unless_present = "command")]
        agent: Option<String>,
        /// 지시문 파일. 없으면 stdin을 읽는다(stdin이 터미널이면 지시문 없이 만든다).
        #[arg(long)]
        file: Option<PathBuf>,
        /// 자식이 실행할 명령. ssh처럼 낱말을 빈칸으로 이어 셸이 해석한다.
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// 부모나 자식에게 메시지를 보낸다. 받는 노드의 턴을 끊지 않는다.
    Send { name: String, body: String },
    /// 일하는 Claude·Codex 노드의 지금 턴에 메시지를 넣는다. 턴을 멈추지 않는다.
    Steer { name: String, body: String },
    /// Claude·Codex 노드가 하던 일을 멈추게 하고 메시지를 따르게 한다. 돌던 셸 명령은 끝낸다.
    Interrupt { name: String, body: String },
    /// 메시지가 올 때까지 기다렸다가 본문을 출력한다.
    Wait,
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
    /// Claude 훅이 부른다. stdin의 훅 입력을 받아 넘길 메시지를 출력한다.
    Hook,
    /// Claude 훅이 Bash 명령 앞에 붙인다. 부른 셸을 중단 대상으로 알린다.
    Guard,
}

fn main() -> ExitCode {
    let result = match Cli::parse() {
        Cli::Serve { listen } => tokio::runtime::Runtime::new()
            .and_then(|rt| rt.block_on(daemon::serve(listen)))
            .map_err(|e| e.to_string()),
        Cli::Spawn {
            name,
            agent,
            file,
            command,
        } => cli::spawn(name, agent, file, command),
        Cli::Send { name, body } => cli::send(name, Kind::Queue, body),
        Cli::Steer { name, body } => cli::send(name, Kind::Steer, body),
        Cli::Interrupt { name, body } => cli::send(name, Kind::Interrupt, body),
        Cli::Hook => cli::hook(),
        Cli::Guard => cli::guard(),
        Cli::Wait => cli::wait(),
        Cli::Done { summary, file } => cli::done(summary, file),
        Cli::Adopt { name } => cli::adopt(name),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("loom: {e}");
            ExitCode::FAILURE
        }
    }
}
