<h1 align="center">
  <img src="docs/images/icon.png" alt="" width="64" valign="middle"> loom
</h1>

<p align="center">
  <a href="https://github.com/BrownC98/loom/releases/latest"><img src="https://img.shields.io/github/v/release/BrownC98/loom?style=flat&color=2E5BD6" alt="최신 릴리스"></a>
  <img src="https://img.shields.io/badge/license-MIT-2E5BD6?style=flat" alt="라이선스: MIT">
  <img src="https://img.shields.io/badge/Windows%20%7C%20macOS%20%7C%20Linux-2E5BD6?style=flat" alt="플랫폼: Windows, macOS, Linux">
</p>

<p align="center">
  <a href="README.md">English</a> · <b>한국어</b>
</p>

<p align="center">
  <strong>loom은 AI 코딩 에이전트(Claude Code, Codex, OpenCode 2, Cursor CLI)를 git 워크트리마다 띄워 한 팀으로 일하게 하고, 그 모습을 실제 터미널 카드의 그래프로 보여 주는 데스크톱·웹 앱입니다.</strong><br>
  에이전트 하나가 노드 카드 하나이고, 카드에 그 에이전트의 실제 터미널이 보입니다. 부모가 자식을 띄우고, 자식이 결과를 보고하고, 메시지는 둘 사이 간선에 보입니다.
</p>

<p align="center">
  <a href="docs/images/standard.png"><img src="docs/images/standard.png" alt="표준 보기의 loom: 노드 일곱 개의 세 단계 그래프이고, 카드마다 실시간 터미널이다. main의 Claude Code lead 아래에 코디 둘(Codex auth, Claude Code ui)이 각자 워크트리 묶음에서 워커 둘(Claude Code, Codex, OpenCode 2)을 둔다. 왼쪽에 저장소 트리, 오른쪽에 커밋 그래프가 펼쳐져 있다." width="960"></a>
</p>

## 기능

| 기능 | 설명 |
|---|---|
| **노드가 된 실제 터미널** | 에이전트는 실제 TUI에서 돕니다. 카드를 누르면 입력하고, 두 번 누르면 확대합니다. |
| **여러 에이전트 CLI를 한 트리에** | 다양한 에이전트가 한 트리에서 함께 일합니다. 카드 머리에 CLI와 모델이 보입니다. |
| **작은 메시징 CLI** | 에이전트는 `loom spawn`, `send`, `wait`, `done`으로 일을 나누고 결과를 보고합니다. |
| **워크트리 묶음** | 노드는 git 워크트리마다 묶입니다. `--worktree <브랜치>`로 새 묶음을 만듭니다. |
| **커밋 그래프와 이벤트 콘솔** | 모든 브랜치의 커밋과 노드 사이 메시지를 봅니다. |

<p align="center">
  <a href="docs/images/hero.png"><img src="docs/images/hero.png" alt="심플 보기의 loom: 같은 노드 일곱 개를 작은 카드로 보이고, 카드마다 요약, 모델, 상태가 있다. 간선에 lead → auth, auth → tests, form → ui 메시지가 보인다. 오른쪽 칸에 auth의 실시간 터미널과 loom message 머리말이 보인다." width="960"></a><br>
  <em>심플 보기: 작은 카드에 요약이 보이고, 옆 칸에 실시간 터미널 하나가 보입니다.</em>
</p>

## 에이전트끼리 말하는 법

에이전트는 자기 터미널에서 아래 명령을 씁니다. loom이 노드마다 주소와 토큰을 넣습니다.

```sh
loom spawn tests --role 워커 --file brief.md     # 이 워크트리에 자식을 만든다
loom note tests "Please stand by for review."    # 메시지를 큐에 넣는다
loom wait --json                                 # 다음 메시지를 기다린다
loom done "Added one toggle() test"              # 부모에게 보고한다
```

## 내장 스킬

loom은 스킬 다섯 개를 싣습니다: `o-mode`(orchestration mode), `ste-100`, `user-doc`, `raptor-3`, `ask-human`. 따로 설치할 것은 없습니다.

- 스킬은 이름으로 부릅니다. Claude Code는 `/loom:<이름>`, Codex는 `loom:<이름>`, OpenCode 2는 `<이름>`입니다.
- 부르지 않아도 늘 적용되는 글: 모든 노드가 `ask-human`(에이전트가 사람에게 묻는 방법: 질문함에 한 줄로 올린다), STE-100, 랩터-3 작업 규칙을 받습니다. `ask-human` 말고는 **설정 › 에이전트 › 내장 스킬**에서 끌 수 있습니다. 부모 없는 노드(사람이 띄운 노드)는 `o-mode` 절차 전체도 받아 총괄이 됩니다. 자식은 `o-mode`의 브리프 공통 규칙만 받습니다.
- 노드는 시작할 때 스킬 글을 읽습니다. loom을 올린 뒤에는 떠 있는 노드를 다시 시작해야 새 글을 받습니다.

자세한 내용은 [docs/design.md 「내장 스킬」](docs/design.md#내장-스킬)에 있습니다.

## 설치

[가장 새 릴리스](https://github.com/BrownC98/loom/releases/latest)에서 받습니다.

- **Windows:** `loom_<판>_x64-setup.exe`를 실행합니다. 지울 때는 Windows **설정 › 앱**을 씁니다.
- **macOS(Apple Silicon):** `loom_<판>_aarch64.dmg`를 열고 loom을 응용 프로그램 폴더로 끕니다. 앱에 Apple 서명이 없어 macOS가 「손상되었기 때문에 열 수 없습니다」라고 합니다. 처음 열기 전에 터미널에서 아래 명령을 한 번 실행합니다.

  ```sh
  xattr -cr /Applications/loom.app
  ```

  지울 때는 응용 프로그램 폴더의 loom을 휴지통으로 옮깁니다.
- **웹 버전(Linux·WSL·macOS·Windows, 앱 없이):** 아래 「웹 버전」을 봅니다.

에이전트 CLI(Claude Code, Codex, OpenCode 2, Cursor CLI)는 따로 설치합니다. loom은 `PATH`에서 찾습니다.

### 웹 버전(모든 OS)

데몬 하나가 화면도 함께 냅니다. 브라우저로 씁니다. 앱 없이 설치할 수 있습니다. 앱을 함께 쓰면 앱 창과 브라우저 탭이 같은 데몬을 봅니다. 노드, 카드 자리, 설정, 터미널이 같습니다.

1. 한 줄을 실행합니다. loom을 받고, 바탕 화면에 loom 아이콘을 만들고, 브라우저에 loom을 엽니다.
   - Linux·WSL·macOS: 터미널(WSL이면 WSL 터미널)에서

     ```sh
     curl -fsSL https://raw.githubusercontent.com/BrownC98/loom/main/scripts/install.sh | sh
     ```

     macOS에 loom 앱이 있으면 앱에 든 loom을 쓰고 따로 받지 않습니다.
   - Windows: PowerShell에서

     ```powershell
     irm https://raw.githubusercontent.com/BrownC98/loom/main/scripts/install.ps1 | iex
     ```

     Windows 앱을 설치했으면 시작 메뉴의 **loom (browser)**도 같은 일을 합니다.
2. 다음부터는 바탕 화면의 **loom** 아이콘을 누릅니다. loom이 꺼져 있으면 켜고 브라우저를 엽니다. 터미널은 필요 없습니다.
3. 브라우저 탭을 닫아도 loom은 계속 돕니다. 끄려면 `loom stop`을 실행합니다. 노드(에이전트)는 꺼지지 않고, 다음에 켜면 다시 붙습니다.
4. 왼쪽 사이드바에서 저장소를 더하고, 워크트리 묶음의 [새 노드]로 에이전트를 띄웁니다.
5. 카드를 누르면 그 터미널에 글을 쓸 수 있습니다. 에이전트가 사람에게 물으면 그래프 위 가운데의 **질문** 알약과 오른쪽 칸의 **질문** 탭에 뜹니다. 누르면 그 노드로 갑니다. 답은 그 노드 터미널에 씁니다.

손으로 설치하려면: 릴리스에서 OS의 파일(`loom-web-linux-x64`, `loom-web-macos-arm64`, `loom-web-windows-x64.exe`)을 받아 이름을 `loom`(Windows는 `loom.exe`)으로 바꾸고 `loom open`을 실행합니다. 이름은 `loom`이어야 합니다. 노드 안의 에이전트가 `loom` 명령을 부릅니다. `loom` 프로세스를 모두 끝내지 않습니다. 노드마다 따로 `loom pty-host` 프로세스에서 돌아서 노드도 끝납니다.

- **데이터:** `~/.loom`(Windows는 `%APPDATA%\loom`)에 DB, 로그, 내장 스킬이 있습니다. 지우면 노드 기록이 사라집니다. 한 기계에 데몬은 하나만 돕니다.
- **업데이트:** 새 판이 나오면 오른쪽 아래에 카드가 뜹니다. [업데이트]를 누르면 받아 바꾸고 다시 뜹니다. **설정 › 정보**에서 판을 보고 [업데이트 확인]으로 바로 확인할 수도 있습니다. 앱에 든 loom이면 앱이 업데이트를 설치합니다. 노드(에이전트 프로세스)는 데몬과 따로 살아 새 데몬이 다시 붙습니다. 1단계의 한 줄을 다시 실행해도 됩니다.
- **주소:** 기본은 이 기계에서만 열립니다(루프백). 사용자는 한 명이라는 가정이라 인증이 없으므로, `--listen`으로 바깥에 열지 않습니다.

설계와 결정은 [docs/design.md](docs/design.md)에 있습니다.

## Privacy policy

loom은 사람이 정하지 않은 곳에 데이터를 보내지 않습니다. 밖으로 나가는 요청은 아래뿐입니다.

- **업데이트 확인(데스크톱 앱):** 창을 열 때 GitHub의 이 저장소 릴리스에서 `latest.json`을 읽습니다. 보내는 개인 데이터는 없습니다.
- **사용량 표시:** 사람이 loom 설정에서 로그인한 Claude 계정의 사용량을 그 계정의 토큰으로 Anthropic(`api.anthropic.com`)에 묻습니다. 다른 곳에는 보내지 않습니다.
- **에이전트 CLI:** 사람이 설치하고 고른 에이전트(Claude Code, Codex 등)는 각 제공자의 정책에 따라 통신합니다. loom은 그 통신에 데이터를 더하지 않습니다.

## 라이선스

[MIT](LICENSE)
