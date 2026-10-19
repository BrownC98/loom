# loom 설계

2026-10-19 설계 인터뷰에서 정한 결정이다. 구현은 이 문서를 따른다. 결정을 바꾸면 이 문서를 같은 커밋에서 고친다.

## 목표

- loom은 Orca보다 가볍고 시각적인 멀티에이전트 개발 도구다.
- 에이전트 하나는 그래프의 노드 하나다. 노드 카드에는 그 에이전트의 터미널이 보인다.
- 핵심 목표는 협업할 때 에이전트가 받는 컨텍스트 부담을 최소로 만드는 것이다.
- 기능은 넷만 둔다: 에이전트 터미널, 워크트리 묶음, 에이전트 사이 메시징, 커밋 그래프. 워크플로 DAG 엔진은 만들지 않는다.
  - 커밋 그래프는 읽기만 한다. loom은 커밋하지 않는다.
- 새 설계는 에이전트 사이 통신에 집중한다. 설정(터미널, 테마, 에이전트 프로필, 계정 등)은 새로 설계하지 않고 Orca의 같은 설정을 먼저 보고 따른다. Orca와 다르게 하면 이유를 결정 표에 적는다(2026-10-19 사람 지시 "제공해야할 각종 설정들은 orca를 최대한 참고한다. 우리는 에이전트간 통신에 집중한다").
- 사용자는 한 명이다. 데몬은 `127.0.0.1`에서만 듣는다.

## 그래프와 화면

화면의 모양과 토큰은 승인한 목업 [docs/mockups/main.html](mockups/main.html)이 명세다. 설정 창, 하단 상태 줄, 새 노드 창의 에이전트 칸은 [docs/mockups/settings.html](mockups/settings.html)이 명세다. 사이드바, 미니맵, 커밋 그래프 칸은 [docs/mockups/graph-panels.html](mockups/graph-panels.html)이 명세다. 이 절은 동작만 적는다.

- 그래프 하나에는 저장소 하나만 보인다. 화면 왼쪽 사이드바의 트리에서 저장소를 바꾼다. 저장소는 경로를 글로 입력해 더한다. 최근 경로 목록을 함께 보인다.
- 워크트리는 묶음(React Flow group node)이다. 저장소의 원래 체크아웃도 묶음 하나다.
- 노드는 늘 묶음 하나 안에 있다.
  - 사람: [새 워크트리]로 묶음을 만든다(브랜치 이름, 기준 브랜치). 그 묶음 안에서 [새 노드]를 누른다(에이전트, 첫 지시문).
  - 에이전트: `loom spawn`으로 만든 자식은 부모와 같은 묶음에 들어간다. `--worktree <브랜치>`를 주면 새 묶음을 만들고 그 안에 넣는다.
- 간선은 부모-자식 관계다. 묶음 경계를 넘어도 된다. 메시지가 오가면 그 간선을 잠깐 강조한다.
- 배치: 묶음 안은 나무 모양으로 자동 배치한다. 자식은 부모 아래 줄에, 형제는 오른쪽 칸에 놓는다. 사람이 띄운 노드는 앞 노드의 나무 아래에 쌓는다. 묶음끼리는 나란히 놓는다. 사람이 끌어 옮긴 위치는 저장한다.
- 병합은 사람이나 부모 에이전트가 `git`으로 한다. loom은 병합하지 않는다.
- [워크트리 닫기]는 사람만 한다. 미커밋 변경이 있거나 안에 `running` 노드가 있으면 거부한다.
- 카드 터미널은 입력을 받는다. 고른 카드만 입력을 받는다. 카드를 두 번 누르면 확대한다.
- 노드 상태는 `running`, `done`, `exited(<코드>)` 셋이다. 카드에는 읽지 않은 메시지 수도 보인다. 부모 없는 카드에는 보관한 메시지 수(「보관 N」)를 보인다.
- 노드를 지우고 부모를 바꾸는 규칙은 「[노드 제거와 입양](#노드-제거와-입양)」에 있다.
- 데몬이 다시 시작하면 `running`이던 노드는 프로세스가 없다. 이 노드는 코드 없는 `exited(?)`로 바꾼다.
- 사이드바는 트리와 맨 아래 [설정]이다. [설정]이 설정 창을 연다. Orca도 설정 단추를 사이드바 맨 아래에 둔다.
  - 트리는 저장소 > 워크트리 묶음 > 노드 > 자식 노드다. 그래프와 같은 노드 목록을 그린다.
  - 노드 줄을 누르면 그래프가 그 카드로 옮겨 가고 그 카드를 고른다. 고른 카드의 터미널이 입력을 받는다.
  - 키보드: ↑↓로 줄을 옮긴다. →는 펴고 ←는 접거나 부모 줄로 간다.
- 미니맵은 그래프 오른쪽 아래에 늘 보인다. 누르거나 끌면 그 자리로 시점을 옮긴다. 켜고 끄는 설정은 없다.
- 커밋 그래프는 도구 줄의 [커밋 그래프]로 여는 오른쪽 칸이다.
  - 모든 브랜치의 최근 커밋 1,000개를 차선 그림으로 보인다. 차선 계산과 그리기는 VS Code `scmHistory.ts`를 옮겼다(MIT).
  - 워크트리가 체크아웃한 브랜치에는 폴더 표시가 붙는다. 분리된 HEAD는 `HEAD` 표시로 보인다.
  - 칸이 열린 동안 3초마다 다시 읽는다. 사람이 터미널에서 한 커밋도 보여야 하기 때문이다.

## 에이전트 실행

- 에이전트는 데몬이 쥔 PTY 하나에서 도는 실제 TUI다. tmux는 쓰지 않는다. 데몬이 죽으면 에이전트도 끝난다.
- CLI마다 실행 프로필을 둔다. 프로필은 메시지 연동(훅·app-server)과 계정이 있는 에이전트다. 지금은 Claude Code와 Codex 둘이다. 프로필이 바뀌는 이유는 CLI 버전이다.

| 프로필 | loom 사용법 지시 | 상태·전달 경로 |
|---|---|---|
| Claude Code | `--append-system-prompt` | `--settings`로 넘긴 훅 5개가 `loom hook`을 부른다(UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, Stop). 형식과 인자는 [src/profile/claude.rs](../src/profile/claude.rs)에 있다 |
| Codex | `-c developer_instructions=…` | 데몬이 노드마다 `codex app-server --listen ws://127.0.0.1:0`을 띄우고 TUI를 `codex --remote <주소>`로 붙인다. 데몬도 같은 서버에 연결해 스레드 상태를 보고 `turn/*` 요청으로 전달한다. 훅은 쓰지 않는다. 연결은 [src/codex_server.rs](../src/codex_server.rs), 요청 내용은 [src/profile/codex.rs](../src/profile/codex.rs)에 있다 |
| 프로필 없음 | 없음 | `loom wait`로만 메시지를 받는다 |

- Claude `--settings`는 권한 허용 목록에 `loom guard`와 메시지 명령(`send`, `steer`, `interrupt`, `wait`, `done`)만 넣는다. 기본 권한 모드에서 사람이 없는 노드가 승인 창에서 멈추지 않게 한다. `loom spawn`은 `-- <명령>`으로 아무 명령이나 띄울 수 있어 넣지 않는다.
- Codex 샌드박스(`workspace-write`)는 셸 명령의 네트워크를 막는다. 그래서 데몬은 노드 문을 하나 더 연다. 노드 문은 노드 전용 루프백 주소 `127.76.77.1:<OS가 고른 포트>`에서 듣고 `/api/agent/*` 가운데 `spawn`을 뺀 라우트만 둔다.
  - app-server에 `-c sandbox_workspace_write.network_access=true`, `-c features.network_proxy.enabled=true`, `-c features.network_proxy.domains={"127.76.77.1"="allow"}`를 넘긴다. 셸 명령은 Codex 프록시로만 밖에 닿는다. 프록시는 노드 문 주소 말고 모든 주소를 막는다. 파일 권한은 그대로다. 인자는 [src/profile/codex.rs](../src/profile/codex.rs)의 `sandbox_args`에 있다.
  - `loom` 명령은 노드 문(`LOOM_URL`)에 Codex가 넣은 프록시 환경 변수(`ALL_PROXY`)로 닿는다. `loom spawn`만 데몬 주소(`LOOM_SPAWN_URL`)에 프록시 없이 간다. 샌드박스 안에서는 연결이 실패한다. 실패 메시지가 샌드박스 밖 실행 승인을 받으라고 적고, Codex 모델은 그 출력을 보고 승인을 요청한다. 사용법 지시(`developer_instructions`)에만 적었을 때는 요청하지 않았다.
  - 권한 프로필(`default_permissions`, `permissions.<이름>.network`)은 쓰지 않는다. TUI가 `thread/start`에 `sandbox: "workspace-write"`를 넣어 보내 프로필을 덮는다.
  - 설치한 Codex 버전이 고정 버전과 다르면 위 인자를 넘기지 않는다. 프록시가 뜨지 않는 버전에서 `network_access`만 켜지면 모든 주소가 열린다. 그때 Codex 노드는 loom 명령마다 승인을 묻고, 설정 화면의 버전 경고가 이 사실을 보인다.
  - 노드 문 주소에서 듣지 못하면(예: 이 루프백 주소가 없는 OS) 노드 문 없이 데몬 주소를 쓴다. 인자도 넘기지 않는다. Codex 노드는 loom 명령마다 승인을 묻는다.
  - 위험: 모든 주소(`0.0.0.0`)에서 듣는 다른 서비스는 샌드박스 안에서 `127.76.77.1`로 닿는다. 데몬을 `--listen 0.0.0.0:<포트>`로 띄우면 `spawn`도 그 길로 닿는다. Codex 도메인 규칙은 포트를 가리지 않고, unix 소켓 허용은 macOS만 된다(「실측 근거」). 그래서 더 좁힐 수 없다.
  - 신뢰하지 않은 폴더에서는 TUI가 `read-only`를 보낸다. `read-only`는 네트워크를 열지 않아 loom 명령마다 승인을 묻는다.
- 사용자의 `~/.claude`, `~/.claude.json`, `~/.codex` 파일은 고치지 않는다. loom이 그 폴더로 CLI(`claude auth status`, `codex app-server`)를 부를 때 CLI가 스스로 쓰는 것은 사용자가 CLI를 실행할 때와 같다.
- Codex app-server는 `[experimental]`이다. 빌드 때 `codex app-server generate-json-schema`로 받은 스키마를 저장소에 고정하고 Rust 타입을 그 스키마에서 만든다. 데몬은 시작할 때 Codex 버전을 확인한다. 고정한 버전과 다르면 화면에 경고를 띄우고 Codex 노드는 그대로 띄운다.
- 고정한 스키마는 쓰는 요청·알림 7개만 둔다([src/profile/codex/schema/](../src/profile/codex/schema/)). `build.rs`가 `typify`로 Rust 타입을 만든다.
- 데몬 연결은 스레드를 구독하지 않는다(`thread/resume`을 부르지 않는다). 구독하면 승인 요청도 데몬 연결로 와서 사람이 TUI에서 승인하지 못한다. 구독하지 않아도 `thread/started`·`thread/status/changed`는 온다. 서버가 따로 만드는 제목용 임시 스레드(`ephemeral`)는 거른다.
- Codex 셸 명령은 TUI가 아니라 app-server 아래에서 돈다. 그래서 app-server도 노드 환경(`LOOM_URL`, `LOOM_SPAWN_URL`, `LOOM_TOKEN`, `PATH`)을 받는다. Codex 셸 스냅샷은 로그인 셸의 `PATH`를 다시 적용한다. 그래서 `-c shell_environment_policy.set.PATH=<노드 PATH>`로 노드 `PATH`를 다시 넣는다.
- 노드가 끝나면 데몬이 그 app-server를 끝낸다. Linux에서는 데몬이 끝나도 app-server가 TERM을 받는다(`PR_SET_PDEATHSIG`).

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

- 설정은 `settings` 테이블 한 줄(JSON)이다: 기본 에이전트, 끈 에이전트 목록, 공급자마다 쓰는 계정, 화면 테마, 터미널 테마.
- 화면 하단 상태 줄은 늘 보인다. 공급자 순서는 Claude, Codex다. 열린 탭은 URL 해시(`#settings/agents`, `#settings/accounts`, `#settings/ui`, `#settings/terminal`)가 갖는다.
- 화면 테마는 「시스템」「라이트」「다크」 가운데 하나다. 처음 값은 「시스템」이고, OS 설정(`prefers-color-scheme`)을 따른다.
  - 데몬이 첫 페이지(`/`)의 `<html>`에 `data-theme`을 넣어 보낸다. 화면 스크립트가 돌기 전에 테마가 정해져 다른 테마가 잠깐 보이지 않는다. 브라우저 저장소에 사본을 두지 않는다.
  - 색 토큰은 `light-dark(라이트, 다크)` 한 벌이다. 테마는 `color-scheme` 하나만 바꾼다.
- 터미널 테마는 loom 테마 2개와 Orca 내장 테마 30개 가운데 다크 모드용과 라이트 모드용을 하나씩 고른다. 기본값은 지금 loom 모양인 「Loom Dark」와 「Loom Light」다. 「라이트 모드에서도 다크 모드 테마를 쓴다」를 켜면 라이트 모드에서도 다크 모드용을 쓴다.
  - 색 값은 `web/src/terminalThemes.ts` 한 곳에만 있다. Orca 테마는 Orca 번들에서 기계로 꺼냈다. 데몬은 이름만 저장한다. 이름이 없거나 목록에 없으면 화면이 기본값을 쓴다. [터미널 테마 초기화]는 저장한 이름을 비운다.
  - 열린 터미널(xterm)은 모드나 고른 테마가 바뀌면 바로 다시 칠한다. 명세는 [settings.html 6번](mockups/settings.html#u-theme)이다.

## 메시징

### 원칙

- PTY 입력은 사용자만 쓴다. loom은 PTY에 글자를 쓰지 않는다. 그래서 사용자가 입력하는 중에도 메시지가 그 글과 섞이지 않는다.
- 메시지는 모두 디스크 큐에 쓴다. 상태는 `queued`, 전달 중, `delivered` 셋이다. 아래 전달 지점이 본문을 꺼내면 전달 중이다. 받는 쪽이 받음을 확인하면 `delivered`다.
- 메시지는 부모와 자식 사이에만 오간다. 형제끼리는 부모를 거친다.
- 자식 이름은 형제 이름, 부모 이름과 겹칠 수 없다. 그래서 `send <이름>`은 내 부모와 내 자식 가운데 한 노드로만 정해진다.

### 명령

| 명령 | 누가 부르나 | 뜻 |
|---|---|---|
| `loom spawn <이름> [--worktree <브랜치>] (--agent <id> \| -- <명령>)` | 에이전트 | 자식 노드를 만든다. 지시문은 stdin이나 `--file`로 받는다. `--agent`는 카탈로그 에이전트를 그 프로필·계정으로 띄운다. `<명령>`은 ssh처럼 낱말을 빈칸으로 이어 셸이 해석한다 |
| `loom send <이름> <본문>` | 에이전트 | 큐: 받는 노드의 턴을 끊지 않는다 |
| `loom steer <이름> <본문>` | 에이전트 | 조정: 턴을 멈추지 않고 다음 도구 호출 사이에 넣는다 |
| `loom interrupt <이름> <본문>` | 에이전트 | 중단: 지금 동작을 멈추고 메시지를 바로 따르게 한다 |
| `loom wait` | 에이전트 | 메시지가 올 때까지 기다렸다가 본문을 출력한다 |
| `loom done <요약>` / `loom done --file <경로>` | 에이전트 | 결과를 부모에게 보내고 노드를 `done`으로 바꾼다 |
| `loom adopt <이름>` | 에이전트 | 부르는 노드가 그 노드의 새 부모가 된다. 이름은 그래프 전체에서 찾는다. 같은 이름이 둘 이상이면 거절한다 |
| `loom hook` | Claude 훅 | stdin의 훅 이벤트를 받아 전달할 메시지를 돌려준다 |
| `loom guard` | Claude PreToolUse 훅 | 훅이 포그라운드 Bash 명령 앞에 이 줄을 붙인다. 부른 셸을 중단 대상으로 데몬에 알린다 |
| `loom serve` | 사람 | 데몬과 화면을 띄운다 |

- 프로필이 없는 노드에 `steer`나 `interrupt`를 보내면 오류를 내고 보내지 않는다. 큐로 바꿔 보내지 않는다.

### 노드 제거와 입양

사람 지시(2026-10-19): 부모를 지워도 자식은 지우지 않는다. 다른 노드가 언제든 부모 역할을 맡는다.

- 사람은 카드의 [노드 제거]로 노드 하나를 지운다. 데몬은 그 노드 프로세스를 끝낸다. Codex 노드면 app-server도 끝낸다. DB에서 노드와 그 노드가 받은 메시지, 보관한 메시지를 지운다.
- 자식은 계속 돈다. 자식의 부모는 비고, 자식은 뿌리가 된다.
- 지운 노드가 `running`이었고 부모가 남으면 부모 메일함에 `removed` 한 줄을 넣는다. 이 줄은 메일함 상한을 넘어도 넣는다. 버리면 부모가 끝없이 기다린다.
- 메시지는 보낸 노드 이름을 함께 저장한다. 보낸 노드를 지워도 받는 노드는 보낸 이름을 읽는다.
- 입양: 노드의 부모를 다른 노드로 바꾼다. 부모가 있는 노드도 된다.
  - 에이전트: `loom adopt <이름>`. Claude·Codex 승인 없이 부른다. `loom spawn`만 승인을 받는다.
  - 사람: 부모가 될 카드의 아래 잇기 점을 끌어 자식이 될 카드에 놓는다.
  - 거절: 자기 자신이나 자기 자손을 부모로 정하면 순환이다. 입양 뒤 중첩 깊이가 2를 넘으면 거절한다. 데려간 노드의 이름이 새 부모나 새 형제와 겹치면 거절한다.
  - 데려간 노드가 `running`이면 옛 부모 메일함에 `adopted: <새 부모>` 한 줄을 넣는다. 옛 부모가 끝없이 기다리지 않게 한다.
- 보관: 부모 없는 노드가 부모에게 보내는 메시지는 버리지 않는다. 그 노드가 보관한다.
  - 보관하는 것: `done` 요약, `exited(<코드>)`, 그래프에 없는 이름으로 보낸 `loom send`. 지운 부모의 이름이 그래프에 없는 이름이다.
  - 부모를 지울 때 자식이 그 부모에게 보냈지만 아직 전하지 않은 큐 메시지도 자식이 보관한다.
  - 조정·중단은 보관하지 않고 거절한다. 받을 노드의 프로필에 따라 넣는 방법이 다르기 때문이다.
  - 새 부모가 생기면 보관한 메시지를 보낸 순서대로 그 부모 메일함에 넣는다. 보관할 때 상한(50)을 봤으므로 이때는 메일함 상한을 보지 않는다.

### 전달 지점

| 방식 | 받는 노드 상태 | Claude Code | Codex |
|---|---|---|---|
| 큐 | `loom wait` 안 | `loom wait`가 본문을 돌려준다 | 같다 |
| 큐 | 일하는 중 | Stop 훅이 `block`과 본문을 돌려준다. 같은 턴을 이어 간다 | `turn/completed` 뒤에 `turn/start` |
| 큐 | 쉬는 중 | 다음 턴의 UserPromptSubmit 훅 맥락. 부모가 결과를 기다리면 백그라운드 `loom wait`로 깨어난다 | `turn/start`로 깨운다. 부모는 `loom wait` 대신 턴을 끝내고 기다린다 |
| 조정 | 일하는 중 | PostToolUse 훅 맥락 | `turn/steer` |
| 중단 | 일하는 중 | 다음 PreToolUse 훅이 도구 호출을 거부하고 본문을 돌려준다. 셸 명령 실행 중이면 데몬이 `loom guard`로 알린 셸과 그 자손 프로세스를 끝내고 PostToolUseFailure 훅이 본문을 돌려준다 | `turn/interrupt` 뒤에 `turn/start` |
| 조정·중단 | 쉬는 중 | 큐와 같다 | 큐와 같다 |

- Claude: 데몬은 노드가 일하는지 쉬는지 추적하지 않는다. 불린 훅이 그 상태다. 훅마다 꺼내는 방식: UserPromptSubmit·Stop은 모두, PostToolUse는 조정, PreToolUse·PostToolUseFailure는 중단.
- 훅 한 번에 꺼내는 본문 합은 8KB 이하다. Claude는 `additionalContext` 한 개를 10,000자에서 자른다. 남은 메시지는 다음 훅이 꺼낸다.
- PreToolUse 훅은 중단 메시지가 없으면 포그라운드 Bash 명령 앞에 `loom guard` 줄을 붙인다(`updatedInput`). 명령을 감싸지 않는 이유: Claude는 권한 규칙을 바꾼 명령으로 판정한다. 감싸면 사용자의 Bash 허용 규칙이 맞지 않는다. 줄을 앞에 붙이면 Claude가 줄마다 판정해 규칙이 그대로 맞는다.
- 백그라운드 Bash 명령(`run_in_background`)에는 붙이지 않는다. 결과를 기다리는 백그라운드 `loom wait`를 중단이 끝내면 안 된다.
- 중단을 보내면 데몬은 셸을 먼저 끝내고 메시지를 넣는다. 반대 순서면 그 셸 안의 `loom wait`가 본문을 꺼낸 뒤 죽어 본문을 잃을 수 있다. 데몬은 셸이 그 노드 프로세스의 자손일 때만 끝낸다. 끝난 셸의 PID를 다른 프로세스가 받았을 수 있다.
- 전달 확정(최소 한 번 전달): 데몬은 꺼낸 메시지에 꺼낸 시각을 적는다. 받는 쪽이 본문을 넣은 뒤 받음을 확인하면 `delivered`로 바꾼다.
  - `loom wait`: 본문을 출력한 뒤 `/api/agent/ack`로 확인한다. `loom hook`: 훅 출력을 쓴 뒤 확인한다. Codex 전달 작업: `turn/*` 요청이 성공한 뒤 확인한다.
  - 확인 없이 30초(`REDELIVER_AFTER`)가 지나면 다음 꺼내기가 같은 메시지를 먼저 다시 준다. 데몬이 다시 시작해도 꺼낸 시각이 DB에 남아 다시 준다.
  - 다음 꺼내기에서 바로 다시 주지 않는다. Claude 노드는 백그라운드 `loom wait`와 훅이 같은 메일함을 함께 꺼낸다. 바로 다시 주면 한쪽이 확인하기 전에 다른 쪽이 같은 메시지를 또 받는다. 30초는 가장 오래 걸리는 Claude 훅 `timeout`(10초)의 3배다.
  - 한계: 받는 쪽이 본문을 넣은 뒤 확인하기 전에 죽으면 같은 메시지가 두 번 간다. 그래서 본문 머리에 메시지 번호(`loom 메시지 #12`)를 넣는다. 에이전트는 같은 번호로 같은 메시지임을 안다.
- `loom hook`의 응답 상한(5초)을 훅 `timeout`(10초)보다 짧게 둔다. 그래서 Claude가 훅을 먼저 끝내 꺼낸 본문을 버리는 일이 드물다. 버려도 확인 전이므로 다시 간다.
- Codex: 데몬은 노드마다 전달 작업 하나를 둔다. 이 작업은 메일함이나 스레드 상태가 바뀔 때 꺼낸다. 쉬면 모두, 일하면 조정·중단만 꺼낸다. 한 번에 꺼내는 본문 합은 Claude와 같은 8KB다.
- Codex: `turn/start`를 보낸 뒤 상태 알림이 오기 전까지는 일하는 중으로 본다. 그 사이에 큐로 턴을 하나 더 시작하지 않는다.
- Codex: `turn/start`가 막 시작된 다른 턴과 겹쳐 실패하면 본문을 `turn/steer`로 넣는다. `turn/steer`가 이미 끝난 턴을 겨눠 실패하면(`expectedTurnId` 불일치) `turn/start`로 보낸다. 꺼낸 메시지를 잃지 않는다.
- Codex 중단: 데몬은 app-server 아래의 세션 리더(셸 명령)와 그 자손을 먼저 끝낸다. MCP 서버와 `codex-code-mode-host`는 서버 세션 안에 있어 남는다. 그 뒤 지금 턴에 `turn/interrupt`를 보내고 `turn/start`로 본문을 보낸다.

### 맥락과 크기

- 자식은 새 맥락에서 시작한다. 지시문(4KB 이하)과 부모 이름만 받는다. 부모 대화를 넘기는 fork는 두지 않는다.
- `--agent`로 띄운 자식은 카탈로그의 `prompt` 방법으로 지시문을 시작 명령에 받는다. 쉬는 상태로 뜬 TUI는 지시문을 읽을 턴이 없기 때문이다.
- `-- <명령>`으로 띄운 자식은 지시문을 메일함의 첫 메시지(보낸 노드는 부모)로 받는다. 자식은 `loom wait`로 읽는다.
- `loom send` 본문도 4KB 이하다. 4KB를 넘는 자료는 파일에 쓰고 경로를 적는다.
- `loom done` 요약은 1KB 이하다. 더 길면 `--file`을 쓰고 부모에게는 경로만 간다.
- `done` 없이 PTY가 끝나면 부모 메일함에 `exited(<코드>)` 한 줄만 넣는다. 부모가 없으면 보관한다. 이 줄은 메일함 상한을 넘어도 넣는다. 버리면 부모가 끝없이 기다린다.

### 상한

| 값 | 기본값 | 근거 |
|---|---|---|
| 메일함 큐 | 50 | Claude Code 세션 사이 메시지의 받는 쪽 큐 상한과 같다 |
| 보관 | 50 | 새 부모가 한꺼번에 받으므로 메일함 큐와 같은 값이다 |
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
- 데몬: `tokio`, `axum`, `portable-pty`(Windows ConPTY 포함), `rusqlite`, `rust-embed`, `sysinfo`(중단할 셸의 프로세스 나무), `tokio-tungstenite`(Codex app-server 연결), `typify`(빌드 때 Codex 스키마에서 타입 생성)
- 화면: React, React Flow, xterm.js. 빌드한 화면을 실행 파일에 넣는다. 사람은 브라우저로 연다. Tauri는 지금 만들지 않는다.
- API 타입은 Rust에서 한 번만 선언한다. TypeScript 타입은 `ts-rs`로 만든다.
- 워크트리와 커밋 그래프는 `git` CLI를 부른다.
- 개발 환경은 도커 컨테이너(`compose.yaml`)다. 호스트에는 Rust·Node를 설치하지 않는다. 실행 명령은 [AGENTS.md 「개발 환경」](../AGENTS.md#개발-환경)에 있다.
- 노드를 띄울 때 `LOOM_URL`(노드 문 주소), `LOOM_SPAWN_URL`(데몬 주소), `LOOM_TOKEN`(노드마다 무작위 값) 환경 변수를 넣는다. 에이전트용 명령은 이 값으로 데몬의 `/api/agent/*`에 HTTP로 요청한다. 데몬은 토큰으로 보낸 노드를 찾는다. 토큰은 메모리에만 둔다. 데몬이 다시 시작하면 노드 프로세스도 모두 끝나기 때문이다.
- 노드의 `PATH` 앞에 `loom` 실행 파일의 폴더를 넣는다. 노드는 설치 없이 `loom`을 부른다.

## 레이어

기능 역할로 나눈다. 의존은 위에서 아래로만 둔다. 기능 모듈끼리는 서로 부르지 않는다.

| 레이어 | 모듈 |
|---|---|
| 상호작용 | 화면, CLI 명령(`src/cli.rs`) |
| 조합 | 데몬 라우트(`src/daemon.rs`, 설정·계정·사용량은 `src/daemon/settings.rs`). 아래 모듈을 부르고 결과를 엮기만 한다 |
| 기능 | 그래프(`src/graph/`: 노드 목록과 메일함), 노드 호스트(`src/node_host.rs`: PTY, 중단할 셸), 워크트리, 커밋 그래프(`src/commit_graph.rs`: `git log`·`git worktree list` 읽기), 프로필(`src/profile.rs`: 에이전트 카탈로그와 시작 명령, `src/profile/claude.rs`: Claude 시작 인자와 훅 입출력, `src/profile/codex.rs`: Codex 시작 인자와 메일함→`turn/*` 규칙), Codex app-server(`src/codex_server.rs`: 서버 프로세스와 JSON-RPC 연결), 계정(`src/accounts.rs`), 사용량(`src/usage.rs`), 설정(`src/settings.rs`) |
| 기반 | DB(`src/db.rs`: 파일 열기와 스키마 변경 목록), 프로세스 나무(`src/process_tree.rs`: 자손 찾기와 끝내기) |

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
- stdio로 띄운 Codex `app-server`는 stdin이 닫히면 답하기 전에 끝난다. 데몬은 답을 받을 때까지 stdin을 열어 두고, 받은 뒤 프로세스를 끝낸다.
- Claude 권한(기본 권한 모드, `claude -p`): PreToolUse `updatedInput`만 넣고 `permissionDecision`을 비우면, 허용 규칙을 바꾼 명령으로 판정했다. `Bash(touch /tmp/original-file)`이 있어도 바꾼 `touch /tmp/rewritten-file`은 승인을 요구했다. 앞에 줄을 붙인 `id -u && touch /tmp/original-file`은 사용자 규칙대로 허용됐다.
- Claude Bash 도구 셸은 자기 세션의 리더다. 셸 안 명령은 다른 프로세스 그룹으로 떴다. 그래서 그룹 하나가 아니라 셸의 프로세스 나무를 끝낸다.
- loom 3단위(Claude 노드 부모·자식): 조정은 자식의 다음 PostToolUse 맥락으로 들어갔다. 중단은 120초 셸 명령을 보낸 뒤 0.08초 만에 끝냈고 PostToolUseFailure가 본문을 전했다. 큐는 자식의 Stop `block`으로 들어갔다. 자식 `done` 뒤 0.03초 만에 쉬던 부모의 백그라운드 `loom wait`가 끝나 부모가 새 턴을 시작했다.
- loom 4단위(Claude 부모 → Codex 자식): 셸 명령을 돌리는 자식에게 보낸 큐는 메일함에 남았고 조정은 바로 꺼내졌다. 중단은 120초 셸 명령을 끝내고 0.06초 만에 돌아왔다(`loom interrupt` 왕복). 끊긴 턴 뒤의 새 턴이 중단 본문을 따랐다. 그 턴이 끝나자 큐가 `turn/start`로 들어갔다. 자식이 `done`으로 보낸 본문에 조정·중단·큐 본문이 모두 있었다. 쉬던 Claude 부모가 바로 깨어났다.
- loom 4단위(Codex 부모 → Claude 자식): `loom spawn --agent claude`가 동작했다. 자식 `done`이 쉬던 Codex 부모를 `turn/start`로 깨웠다. 부모 입력창에 사람이 쓰다 만 글은 그대로 남았고 턴에 섞이지 않았다.
- Codex 셸 명령은 app-server 아래의 `/bin/sh -c` 세션 리더다. `item/started`의 `processId`는 OS PID가 아니다. 그래서 프로세스 나무에서 셸을 찾는다.
- Codex 셸 스냅샷이 로그인 셸의 `PATH`를 다시 적용해 노드 `PATH` 앞의 폴더가 빠졌다. `-c shell_environment_policy.set.PATH`를 주면 남았다. `LOOM_URL`·`LOOM_TOKEN`은 그대로 전해졌다.
- Codex 샌드박스(`codex sandbox`, Linux): 기본 `workspace-write`는 루프백 TCP를 막고(`curl` 종료 코드 7) unix 소켓은 만들지도 못한다(EPERM). 권한 프로필의 `network.unix_sockets` 허용은 Linux에서 동작하지 않았다. 프록시 도메인 규칙에 `127.0.0.1:47801`을 넣어도 같은 주소의 다른 포트가 열렸다.
- `network_access=true`와 프록시 도메인 `127.76.77.1`: `127.76.77.1`은 200, `127.0.0.1`의 다른 포트는 프록시 403, `example.com`은 실패했다. 프록시를 건너뛴 직접 연결은 주소와 관계없이 종료 코드 7이었다. 홈 쓰기는 막히고 `/tmp` 쓰기는 됐다. `read-only`에서는 프록시가 뜨지 않았다.
- `default_permissions`를 app-server와 TUI 모두에 `-c`로 넘겨도 스레드는 `workspace-write`(네트워크 꺼짐)였다. TUI가 `thread/start`에 `sandbox: "workspace-write"`, `permissions: null`을 보냈다.
- loom 노드(Codex 0.159.2, 기본 승인 정책 `on-request`): `loom send`와 `loom done`이 승인 창 없이 부모에게 닿았다. `curl`로 `127.0.0.1`의 다른 포트, 데몬 주소, `example.com`은 프록시가 막았다. `loom spawn`은 샌드박스 안에서 연결이 거부됐고, 샌드박스 밖 실행을 승인하자 자식을 띄웠다.
- Codex 허용 규칙(`prefix_rule`, `allow`)에 맞은 명령은 샌드박스 밖에서 돌았다. 규칙은 설정 폴더의 `rules/`에서만 읽고 `-c`로 넘길 수 없다. 시스템 기본 계정의 설정 폴더는 사용자의 `~/.codex`라 이 방법을 쓰지 않았다.
- 개발 컨테이너에 `bubblewrap`을 넣어도 Docker 기본 seccomp가 사용자 네임스페이스를 막아 Codex 샌드박스가 동작하지 않았다. `seccomp=unconfined`이면 동작했다.
- `--listen ws://`로 띄운 app-server는 stdin이 닫혀도, TUI와 데몬 연결이 끊겨도 끝나지 않았다. `codex` wrapper(node)에 보낸 TERM은 실행 파일까지 끝냈다.

## 아직 확인하지 않은 것

- Windows에서 `loom guard`가 자기를 부른 셸을 찾는 방법. 지금 Windows의 `loom guard`는 아무것도 하지 않아 중단이 셸 명령을 끝내지 못한다. 첫 Windows 구현 때 확인한다.
- macOS에서 노드 문(`127.76.77.1`). macOS는 `127.0.0.1` 말고 루프백 주소가 기본으로 없어 듣지 못할 수 있다. 그러면 Codex 노드는 loom 명령마다 승인을 묻는다. 첫 macOS 구현 때 확인한다.
- macOS·Windows에서 데몬이 신호로 죽으면 Codex app-server가 남는지. Linux만 `PR_SET_PDEATHSIG`로 막았다. Windows에서 `sysinfo`가 세션 ID를 주는지도 확인하지 않았다. 주지 않으면 Codex 중단이 셸을 끝내지 못한다. 첫 macOS·Windows 구현 때 확인한다.
- Codex app-server를 unix 소켓으로 쓸 때의 메시지 틀. 지금은 `ws://127.0.0.1`만 쓴다.
