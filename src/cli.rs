//! 상호작용 레이어: 노드 안의 에이전트가 부르는 명령. 데몬에 HTTP로 요청만 보낸다.
//! 데몬 주소와 자기 토큰은 데몬이 노드를 띄울 때 넣은 `LOOM_URL`, `LOOM_TOKEN`에서 읽는다.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::daemon::{DoneRequest, SendRequest, SpawnRequest};
use crate::graph::{Delivery, NodeId};

pub fn spawn(name: String, file: Option<PathBuf>, command: Vec<String>) -> Result<(), String> {
    let brief = match file {
        Some(path) => {
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?
        }
        None if std::io::stdin().is_terminal() => String::new(),
        None => {
            let mut brief = String::new();
            std::io::stdin()
                .read_to_string(&mut brief)
                .map_err(|e| e.to_string())?;
            brief
        }
    };
    let request = SpawnRequest {
        name,
        command: command.join(" "),
        brief,
    };
    let _: NodeId = call("spawn", &request)?;
    println!("노드를 만들었다: {}", request.name);
    Ok(())
}

pub fn send(to: String, body: String) -> Result<(), String> {
    call::<()>("send", &SendRequest { to, body })
}

pub fn wait() -> Result<(), String> {
    let Delivery { from, body } = call("wait", &())?;
    println!("보낸 노드: {from}\n{body}");
    Ok(())
}

pub fn done(summary: Option<String>, file: Option<PathBuf>) -> Result<(), String> {
    let summary = match file {
        // 부모가 다른 폴더에서 읽어도 찾도록 절대 경로로 보낸다. 파일이 없으면 여기서 실패한다.
        Some(path) => std::fs::canonicalize(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .display()
            .to_string(),
        None => summary.unwrap_or_default(),
    };
    call::<()>("done", &DoneRequest { summary })?;
    println!("부모에게 결과를 보냈다.");
    Ok(())
}

fn call<T: DeserializeOwned>(action: &str, body: &impl Serialize) -> Result<T, String> {
    let var = |name| {
        std::env::var(name).map_err(|_| format!("{name}가 없다. loom 노드 안에서만 부른다."))
    };
    let (url, token) = (var("LOOM_URL")?, var("LOOM_TOKEN")?);
    // 거부 문장을 본문에서 읽으려고 4xx·5xx를 오류로 바꾸지 않는다. `wait`는 메시지가 올 때까지 끝나지 않으므로 시간 상한을 두지 않는다.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut res = agent
        .post(format!("{url}/api/agent/{action}"))
        .header("authorization", format!("Bearer {token}"))
        .send_json(body)
        .map_err(|e| format!("데몬에 연결하지 못했다: {e}"))?;
    if !res.status().is_success() {
        return Err(res.body_mut().read_to_string().unwrap_or_default());
    }
    res.body_mut().read_json().map_err(|e| e.to_string())
}
