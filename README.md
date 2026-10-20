# loom

loom은 여러 코딩 에이전트를 한 화면에서 함께 쓰는 개발 도구입니다. 에이전트 하나가 그래프의 노드 하나이고, 노드 카드에 그 에이전트의 터미널이 보입니다.

- 에이전트 터미널: Claude Code, Codex, OpenCode 2 같은 CLI를 노드로 띄웁니다. 에이전트 CLI는 사람이 따로 설치합니다.
- 워크트리 묶음: git 워크트리마다 노드를 묶어 보입니다.
- 에이전트 사이 메시징: 부모와 자식 노드가 `loom` 명령으로 일을 나누고 결과를 보고합니다.
- 커밋 그래프와 이벤트 콘솔: 저장소의 커밋과 노드 사이 메시지를 봅니다.

설계와 결정은 [docs/design.md](docs/design.md)에 있습니다.

## 설치

[가장 새 Release](https://github.com/BrownC98/loom/releases/latest)에서 받습니다.

- Windows: `loom_<판>_x64-setup.exe`를 실행합니다. 설치 파일에 제거 프로그램이 들어 있습니다. Windows 「설정 › 앱」에서 지웁니다.
- Linux(x86-64): `loom-x86_64-linux`를 받아 `loom serve`로 띄우고, 브라우저로 출력된 주소를 엽니다.

## Code signing policy

Windows 설치 파일과 실행 파일의 서명 정책입니다.

Free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by [SignPath Foundation](https://signpath.org/).

- 서명하는 파일은 이 저장소의 소스에서 GitHub Actions([.github/workflows/release.yml](.github/workflows/release.yml))가 빌드한 파일뿐입니다.
- 서명 요청마다 승인 담당이 손으로 승인합니다.

| 역할 | 담당 |
|---|---|
| Authors(커밋) | [@BrownC98](https://github.com/BrownC98) |
| Reviewers(외부 기여 리뷰) | [@BrownC98](https://github.com/BrownC98) |
| Approvers(서명 승인) | [@BrownC98](https://github.com/BrownC98) |

## Privacy policy

loom은 사람이 정하지 않은 곳에 데이터를 보내지 않습니다. 밖으로 나가는 요청은 아래뿐입니다.

- 업데이트 확인(Windows 앱): 창을 열 때 한 번 GitHub의 이 저장소 Release에서 `latest.json`을 읽습니다. 보내는 개인 데이터는 없습니다.
- 사용량 표시: 사람이 loom 설정에서 로그인한 Claude 계정의 사용량을 그 계정의 토큰으로 Anthropic(`api.anthropic.com`)에 묻습니다. 다른 곳에는 보내지 않습니다.
- 에이전트 CLI: 사람이 설치하고 고른 에이전트(Claude Code, Codex 등)는 각 제공자의 정책에 따라 통신합니다. loom은 그 통신에 데이터를 더하지 않습니다.

## 라이선스

[MIT](LICENSE)
