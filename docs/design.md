# loom 설계

2026-10-19 설계 인터뷰에서 정한 결정이다. 구현은 이 문서를 따른다. 결정을 바꾸면 이 문서를 같은 커밋에서 고친다.

## 목표

- loom은 Orca보다 가볍고 시각적인 멀티에이전트 개발 도구다.
- 에이전트 하나는 그래프의 노드 하나다. 노드 카드에는 그 에이전트의 터미널이 보인다.
- 핵심 목표는 협업할 때 에이전트가 받는 컨텍스트 부담을 최소로 만드는 것이다.
- 기능은 셋만 둔다: 에이전트 터미널, 워크트리 묶음, 에이전트 사이 메시징. 워크플로 DAG 엔진은 만들지 않는다.
- 사용자는 한 명이다. 데몬은 `127.0.0.1`에서만 듣는다.

## 그래프와 화면

화면의 모양과 토큰은 승인한 목업 [docs/mockups/main.html](mockups/main.html)이 명세다. 설정 창, 하단 상태 줄, 새 노드 창의 에이전트 칸은 [docs/mockups/settings.html](mockups/settings.html)이 명세다. 이 절은 동작만 적는다.

- 그래프 하나에는 저장소 하나만 보인다. 화면 왼쪽 저장소 목록에서 저장소를 바꾼다. 저장소는 경로를 글로 입력해 더한다. 최근 경로 목록을 함께 보인다.
- 워크트리는 묶음(React Flow group node)이다. 저장소의 원래 체크아웃도 묶음 하나다.
- 노드는 늘 묶음 하나 안에 있다.
  - 사람: [새 워크트리]로 묶음을 만든다(브랜치 이름, 기준 브랜치). 그 묶음 안에서 [새 노드]를 누른다(에이전트, 첫 지시문).
  - 에이전트: `loom spawn`으로 만든 자식은 부모와 같은 묶음에 들어간다. `--worktree <브랜치>`를 주면 새 묶음을 만들고 그 안에 넣는다.
- 간선은 부모-자식 관계다. 묶음 경계를 넘어도 된다. 메시지가 오가면 그 간선을 잠깐 강조한다.
- 배치: 묶음 안은 나무 모양으로 자동 배치한다. 자식은 부모 아래 줄에, 형제는 오른쪽 칸에 놓는다. 사람이 띄운 노드는 앞 노드의 나무 아래에 쌓는다. 묶음끼리는 나란히 놓는다. 사람이 끌어 옮긴 위치는 저장한다.
- 병합은 사람이나 부모 에이전트가 `git`으로 한다. loom은 병합하지 않는다.
- [워크트리 닫기]는 사람만 한다. 미커밋 변경이 있거나 안에 `running` 노드가 있으면 거부한다.
- 카드 터미널은 입력을 받는다. 고른 카드만 입력을 받는다. 카드를 두 번 누르면 확대한다.
- 노드 상태는 `running`, `done`, `exited(<코드>)` 셋이다. 카드에는 읽지 않은 메시지 수도 보인다.
- 데몬이 다시 시작하면 `running`이던 노드는 프로세스가 없다. 이 노드는 코드 없는 `exited(?)`로 바꾼다.

## 에이전트 실행

- 에이전트는 데몬이 쥔 PTY 하나에서 도는 실제 TUI다. tmux는 쓰지 않는다. 데몬이 죽으면 에이전트도 끝난다.
- CLI마다 실행 프로필을 둔다. 프로필은 메시지 연동(훅·app-server)과 계정이 있는 에이전트다. 지금은 Claude Code와 Codex 둘이다. 프로필이 바뀌는 이유는 CLI 버전이다.

| 프로필 | loom 사용법 지시 | 상태·전달 경로 |
|---|---|---|
| Claude Code | `--append-system-prompt` | `--settings`로 넘긴 훅 5개가 `loom hook`을 부른다(UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, Stop) |
| Codex | `-c developer_instructions=…` | `codex --remote ws://127.0.0.1:<포트>`로 loom 데몬이 띄운 app-server에 붙인다. 훅은 쓰지 않는다 |
| 프로필 없음 | 없음 | `loom wait`로만 메시지를 받는다 |

- 사용자의 `~/.claude`, `~/.claude.json`, `~/.codex` 파일은 고치지 않는다. loom이 그 폴더로 CLI(`claude auth status`, `codex app-server`)를 부를 때 CLI가 스스로 쓰는 것은 사용자가 CLI를 실행할 때와 같다.
- Codex app-server는 `[experimental]`이다. 빌드 때 `codex app-server generate-json-schema`로 받은 스키마를 저장소에 고정하고 Rust 타입을 그 스키마에서 만든다. 데몬은 시작할 때 Codex 버전을 확인한다. 고정한 버전과 다르면 화면에 경고를 띄우고 Codex 노드는 그대로 띄운다.

### 에이전트

- 카탈로그는 Orca의 에이전트 표를 옮긴 데이터 [src/agents.json](../src/agents.json)이다. 44개다. 목록을 지어내지 않는다. Orca가 표를 고치면 이 파일만 다시 옮긴다.
  - `claude-agent-teams`는 뺐다. Orca 안에서만 쓰는 Claude 팀 모드라 따로 띄울 CLI가 없다.
  - 한 줄은 `id`, `label`, `cmd`, `aliases`, `args`, `prompt`, `profile`, `homepage`다.
- `prompt`는 첫 지시문을 넘기는 방법이다. Orca의 `promptInjectionMode`를 옮겼다.

| `prompt` | 명령 | Orca 값 |
|---|---|---|
| `"argv"` | `<cmd> <args…> <지시문>` | `argv` |
| `{"argvAfter": "--"}` | `<cmd> <args…> -- <지시문>` | `argv` + 구분자 |
| `{"flag": "--prompt"}` | `<cmd> <args…> --prompt <지시문>` | `flag-prompt`, `flag-prompt-interactive`, `flag-interactive` |
| `"none"` | 지시문을 넘기지 않는다 | `stdin-after-start`, `hermes-query` |

- `"none"`인 에이전트는 Orca가 시작 뒤 입력창에 지시문을 써 넣는다. loom은 PTY에 글자를 쓰지 않으므로(「메시징」) 첫 지시문을 받지 않는다. 새 노드 창은 지시문 칸을 막는다.
- 탐지: 데몬이 자기 `PATH`에서 `cmd`, 그다음 `aliases`를 찾는다(`which`). 설정 창을 열 때와 [다시 찾기]를 누를 때 다시 찾는다. 노드를 띄울 때도 찾은 경로로 띄운다.
- 사용자는 설치한 에이전트를 켜고 끈다. 새 노드 창은 켠 에이전트와 「직접 입력」만 보인다.
- 기본 에이전트는 새 노드 창이 처음에 고르는 에이전트다. 고르지 않았거나 그 에이전트를 껐으면 켠 것 가운데 첫째(카탈로그 순서)다. Orca의 Auto와 같은 결과라 Auto 선택지는 두지 않는다.
- 노드 목록에는 에이전트 `id`를 남긴다. 「직접 입력」으로 띄운 노드와 `loom spawn` 자식은 에이전트가 없다. 카드 머리는 카탈로그의 `label`을 보인다.
- 넣지 않은 Orca 기능: 설치 버튼, 실행 파일 경로 재정의, 기본 인자·환경 변수, 권한 모드. 「직접 입력」에 명령 전체를 적으면 같은 일을 한다.

### 계정과 사용량

- 공급자(Claude, Codex)마다 계정을 여럿 둔다. 「시스템 기본」은 사용자의 `~/.claude`·`~/.codex`다. 지울 수 없다.
- loom이 만든 계정은 로그인 폴더 `~/.loom/accounts/<claude|codex>/<id>`를 쓴다. `id`는 만든 시각(밀리초)의 16진수 16자리라 이름 순서가 만든 순서다.
- 새 노드는 설정에서 고른 계정(공급자마다 하나, 「쓰는 중」)으로 뜬다. 데몬이 `CLAUDE_CONFIG_DIR`·`CODEX_HOME`을 그 폴더로 넣는다. 시스템 기본이면 넣지 않는다. 고른 계정의 폴더가 없으면 시스템 기본으로 띄운다. 이미 도는 노드는 계정을 바꿔도 그대로다.
- [계정 추가]: 데몬이 폴더를 만들고, 그 폴더로 `claude auth login`·`codex login`을 PTY에서 띄운다. 화면은 이 PTY를 노드 카드와 같은 터미널로 보인다. 그래프에는 나타나지 않는다.
  - 로그인 프로세스가 코드 0으로 끝나면 창이 닫힌다. 0이 아니면 실패를 보인다.
  - 새 계정은 로그인이 실패하거나 사용자가 창을 닫으면 폴더를 지운다. [다시 로그인]은 폴더를 지우지 않는다.
- 사용량 출처는 Orca와 같다.

| 공급자 | 계정 정보 | 사용량 |
|---|---|---|
| Claude | `claude auth status --json`(`loggedIn`, `email`, `subscriptionType`, `configDirectory`) | `<configDirectory>/.credentials.json`의 OAuth 토큰으로 `GET https://api.anthropic.com/api/oauth/usage`(`anthropic-beta: oauth-2025-04-20`). `five_hour`·`seven_day`의 `utilization`(사용한 %)과 `resets_at` |
| Codex | `codex app-server`의 `account/read`(`email`, `planType`) | 같은 서버의 `account/rateLimits/read`. `primary`·`secondary`의 `usedPercent`, `windowDurationMins`, `resetsAt`. 창 길이 300분 이하가 5시간 창이다 |

- 화면은 남은 비율만 보인다(100 − 사용한 %). 막대도 남은 양만큼 차고, 25% 이하면 노란색이다.
- 사용량은 화면이 읽을 때마다 데몬이 그 자리에서 읽는다(`GET /api/accounts`). 화면은 5분마다, 사용량 팝오버를 열 때, [새로 고침]을 누를 때 읽는다. 데몬은 저장하지 않는다.
- 읽는 상한은 계정마다 15초다. 넘거나 실패하면 그 계정만 오류 한 줄을 보인다.
- 한계: Claude 토큰을 macOS 키체인에 둔 로그인과 API 키 로그인은 사용량을 볼 수 없다. 토큰이 만료돼 401이면 「이 계정으로 Claude를 한 번 실행하면 갱신된다」를 보인다. loom은 토큰을 갱신하지 않는다. 갱신은 로그인 폴더를 고치는 일이다.
- 넣지 않은 Orca 기능: Codex 한도 초기화 크레딧, 추가 사용량, 상태 줄 표시 방식(간단·자세히, 사용·남음) 바꾸기, 상태 줄 항목 고르기, Codex 계정을 바꾼 뒤 세션 다시 시작.

### 설정

- 설정은 `settings` 테이블 한 줄(JSON)이다: 기본 에이전트, 끈 에이전트 목록, 공급자마다 쓰는 계정.
- 화면 하단 상태 줄은 늘 보인다. 공급자 순서는 Claude, Codex다. 오른쪽 끝의 [설정]이 설정 창을 연다. 열린 탭은 URL 해시(`#settings/agents`, `#settings/accounts`)가 갖는다.

## 메시징

### 원칙

- PTY 입력은 사용자만 쓴다. loom은 PTY에 글자를 쓰지 않는다. 그래서 사용자가 입력하는 중에도 메시지가 그 글과 섞이지 않는다.
- 메시지는 모두 디스크 큐에 쓴다. 상태는 `queued`와 `delivered`다. 아래 전달 지점이 본문을 넘긴 순간 `delivered`로 바꾼다.
- 메시지는 부모와 자식 사이에만 오간다. 형제끼리는 부모를 거친다.
- 자식 이름은 형제 이름, 부모 이름과 겹칠 수 없다. 그래서 `send <이름>`은 내 부모와 내 자식 가운데 한 노드로만 정해진다.

### 명령

| 명령 | 누가 부르나 | 뜻 |
|---|---|---|
| `loom spawn <이름> [--worktree <브랜치>] -- <명령>` | 에이전트 | 자식 노드를 만든다. 지시문은 stdin이나 `--file`로 받는다. `<명령>`은 ssh처럼 낱말을 빈칸으로 이어 셸이 해석한다 |
| `loom send <이름> <본문>` | 에이전트 | 큐: 받는 노드의 턴을 끊지 않는다 |
| `loom steer <이름> <본문>` | 에이전트 | 조정: 턴을 멈추지 않고 다음 도구 호출 사이에 넣는다 |
| `loom interrupt <이름> <본문>` | 에이전트 | 중단: 지금 동작을 멈추고 메시지를 바로 따르게 한다 |
| `loom wait` | 에이전트 | 메시지가 올 때까지 기다렸다가 본문을 출력한다 |
| `loom done <요약>` / `loom done --file <경로>` | 에이전트 | 결과를 부모에게 보내고 노드를 `done`으로 바꾼다 |
| `loom hook` | Claude 훅 | stdin의 훅 이벤트를 받아 전달할 메시지를 돌려준다 |
| `loom run -- <명령>` | Claude PreToolUse 훅 | 셸 명령을 감싼다. 중단이 오면 loom이 이 프로세스 묶음을 끝낸다 |
| `loom serve` | 사람 | 데몬과 화면을 띄운다 |

- 프로필이 없는 노드에 `steer`나 `interrupt`를 보내면 오류를 내고 보내지 않는다. 큐로 바꿔 보내지 않는다.

### 전달 지점

| 방식 | 받는 노드 상태 | Claude Code | Codex |
|---|---|---|---|
| 큐 | `loom wait` 안 | `loom wait`가 본문을 돌려준다 | 같다 |
| 큐 | 일하는 중 | Stop 훅이 `block`과 본문을 돌려준다. 같은 턴을 이어 간다 | `turn/completed` 뒤에 `turn/start` |
| 큐 | 쉬는 중 | 다음 턴의 UserPromptSubmit 훅 맥락. 부모가 결과를 기다리면 백그라운드 `loom wait`로 깨어난다 | `turn/start`로 깨운다 |
| 조정 | 일하는 중 | PostToolUse 훅 맥락 | `turn/steer` |
| 중단 | 일하는 중 | 다음 PreToolUse 훅이 도구 호출을 거부하고 본문을 돌려준다. 셸 명령 실행 중이면 `loom run` 프로세스 묶음을 끝내고 PostToolUseFailure 훅이 본문을 돌려준다 | `turn/interrupt` 뒤에 `turn/start` |
| 조정·중단 | 쉬는 중 | 큐와 같다 | 큐와 같다 |

### 맥락과 크기

- 자식은 새 맥락에서 시작한다. 지시문(4KB 이하)과 부모 이름만 받는다. 부모 대화를 넘기는 fork는 두지 않는다.
- 지시문은 자식 메일함의 첫 메시지(보낸 노드는 부모)로 넣는다. 프로필이 없는 자식은 `loom wait`로 받는다.
- `loom send` 본문도 4KB 이하다. 4KB를 넘는 자료는 파일에 쓰고 경로를 적는다.
- `loom done` 요약은 1KB 이하다. 더 길면 `--file`을 쓰고 부모에게는 경로만 간다.
- `done` 없이 PTY가 끝나면 부모 메일함에 `exited(<코드>)` 한 줄만 넣는다. 이 줄은 메일함 상한을 넘어도 넣는다. 버리면 부모가 끝없이 기다린다.

### 상한

| 값 | 기본값 | 근거 |
|---|---|---|
| 메일함 큐 | 50 | Claude Code 세션 사이 메시지의 받는 쪽 큐 상한과 같다 |
| 중첩 깊이 | 2 | 사람이 띄운 노드가 0이다. 부모가 받는 결과 요약의 양을 묶는다 |
| 동시 노드 | 없음 | 상한을 두지 않는다 |

## 저장

- SQLite 파일 하나: `~/.loom/loom.db`, WAL 모드. 라이브러리는 `rusqlite`다.
- 메일함, 노드 목록, 워크트리 묶음, 카드 위치, 설정을 모두 여기에 둔다. 데몬이 다시 시작해도 메시지는 남는다.
- 계정 로그인 폴더는 같은 폴더의 `~/.loom/accounts/`에 둔다.
- 개발 컨테이너는 `~/.loom`을 이름 있는 볼륨(`loom_home`)에 둔다. 컨테이너를 다시 만들어도 DB와 계정 폴더가 남는다.

## 규모

- 화면에 보이는 카드만 터미널 스트림을 연다. 화면 밖 카드는 상태와 마지막 한 줄만 받는다.
- 노드마다 출력 링 버퍼를 256KB까지 둔다. 카드를 다시 열면 버퍼를 먼저 보내고 실시간 출력을 잇는다.
- 화면 갱신은 노드 상태 이벤트 하나로 보낸다. 노드 목록 전체를 다시 보내지 않는다.

## 기술 스택

- `loom` Rust 실행 파일 하나가 데몬과 모든 명령을 맡는다. Windows, macOS, Linux에서 같은 코드가 돈다.
- 데몬: `tokio`, `axum`, `portable-pty`(Windows ConPTY 포함), `rusqlite`, `rust-embed`
- 화면: React, React Flow, xterm.js. 빌드한 화면을 실행 파일에 넣는다. 사람은 브라우저로 연다. Tauri는 지금 만들지 않는다.
- API 타입은 Rust에서 한 번만 선언한다. TypeScript 타입은 `ts-rs`로 만든다.
- 워크트리는 `git` CLI를 부른다.
- 개발 환경은 도커 컨테이너(`compose.yaml`)다. 호스트에는 Rust·Node를 설치하지 않는다. 실행 명령은 [AGENTS.md 「개발 환경」](../AGENTS.md#개발-환경)에 있다.
- 노드를 띄울 때 `LOOM_URL`(데몬 주소), `LOOM_TOKEN`(노드마다 무작위 값) 환경 변수를 넣는다. 에이전트용 명령은 이 값으로 데몬의 `/api/agent/*`에 HTTP로 요청한다. 데몬은 토큰으로 보낸 노드를 찾는다. 토큰은 메모리에만 둔다. 데몬이 다시 시작하면 노드 프로세스도 모두 끝나기 때문이다.
- 노드의 `PATH` 앞에 `loom` 실행 파일의 폴더를 넣는다. 노드는 설치 없이 `loom`을 부른다.

## 레이어

기능 역할로 나눈다. 의존은 위에서 아래로만 둔다. 기능 모듈끼리는 서로 부르지 않는다.

| 레이어 | 모듈 |
|---|---|
| 상호작용 | 화면, CLI 명령(`src/cli.rs`) |
| 조합 | 데몬 라우트(`src/daemon.rs`, 설정·계정·사용량은 `src/daemon/settings.rs`). 아래 모듈을 부르고 결과를 엮기만 한다 |
| 기능 | 그래프(`src/graph/`: 노드 목록과 메일함), 노드 호스트(`src/node_host.rs`, PTY), 워크트리, 프로필(`src/profile.rs`: 에이전트 카탈로그와 시작 명령), 계정(`src/accounts.rs`), 사용량(`src/usage.rs`), 설정(`src/settings.rs`) |
| 기반 | DB(`src/db.rs`: 파일 열기와 스키마 변경 목록) |

- 기능 모듈은 서로 부르지 않는다. 다른 기능 모듈의 값 타입(`NodeId`, `AgentId`, `Profile`, `AccountId`)만 쓴다.
- 노드 호스트는 키에 대해 제네릭이다. 노드 PTY(`NodeId`)와 로그인 창 PTY(`LoginId`)가 같은 코드를 쓴다.
- 노드 목록과 메일함은 한 모듈이다. 부모-자식 규칙(깊이, 이름, 메시지 방향)이 둘을 함께 보고, `done`처럼 둘을 한 트랜잭션에서 바꿔야 하기 때문이다.
- 새 테이블은 `src/db.rs`의 `STEPS` 끝에 항목 하나로 더한다. 마이그레이션은 `rusqlite_migration`(`PRAGMA user_version`)이 한다.

## 실측 근거

2026-10-19 Claude Code 2.1.303, Codex 0.159.2에서 확인했다.

- Claude `--settings` 훅: Stop `block`, UserPromptSubmit·PostToolUse 맥락, PreToolUse `updatedInput`이 모두 동작했다.
- Claude 중단: 90초 셸 명령을 8초에 끊었다. PostToolUseFailure 훅이 본문을 전했고, 전체가 15초에 끝났다.
- Claude 깨우기: 쉬는 TUI가 백그라운드 명령 완료 알림으로 새 턴을 시작했다.
- Codex `-c`로 넘긴 훅은 실행되지 않았다. Codex는 사람이 검토해 신뢰한 훅만 실행한다.
- Codex app-server: TUI를 `--remote`로 붙인 상태에서 다른 클라이언트가 `turn/steer`, `turn/interrupt`, `turn/start`를 보냈다. 셋 다 동작했고 TUI 화면에도 보였다.
- 두 TUI 모두 위 전달 중에 사용자가 쓰다 만 입력창 글이 그대로 남았다.
- Codex `turn/interrupt`는 턴만 멈춘다. 실행 중이던 셸 명령은 끝까지 돌았다.
- tmux로 글자와 Enter를 한 번에 보내면 Codex는 Enter를 줄바꿈으로 받았다. PTY에 글자를 쓰지 않는 원칙의 근거다.
- 사용량 출처(「계정과 사용량」): Claude `api/oauth/usage`가 `five_hour`·`seven_day`를 돌려줬다. Codex `account/rateLimits/read`는 `primary`에 주간 창(10080분)을 주고 `secondary`는 `null`이었다. 그래서 창은 위치가 아니라 길이로 가른다.
- Codex `app-server`는 stdin이 닫히면 답하기 전에 끝난다. 데몬은 답을 받을 때까지 stdin을 열어 두고, 받은 뒤 프로세스를 끝낸다.

## 아직 확인하지 않은 것

- Windows에서 `loom run`이 프로세스 묶음을 끝내는 방법(Job Object). 첫 Windows 구현 때 확인한다.
- Codex app-server를 unix 소켓으로 쓸 때의 메시지 틀. 지금은 `ws://127.0.0.1`만 쓴다.
