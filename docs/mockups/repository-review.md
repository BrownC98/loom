# 저장소 추가(기존 폴더·URL 복제·새 프로젝트) 조사와 점검 기록

목업: [repository.html](repository.html). 이 파일은 승인할 그림 밖의 조사·설계·점검 기록이다.

## 1. 확인한 사실

### Orca(설치본 화면 코드, 읽기만 함)

- `AddRepoDialog`의 `lt()`가 시작 동작 셋을 정한다. 주 동작은 「Browse folder」, 보조 동작은 「Clone from URL」, 「Create new project」(설명: "Start from an empty folder")다.
- 새 프로젝트는 `window.api.repos.create({parentPath, name, kind: 'git'})`를 부른다. 화면 코드는 `kind: 'git'`만 넘긴다. `git init`만 하는지, 첫 커밋도 만드는지는 메인 프로세스 코드에 있다. 받은 파일로는 확인하지 못했다.
- 복제는 `window.api.repos.clone({url, destination})`를 부른다. 칸 이름은 「Git URL」, 「Parent folder」다. 진행 상황은 `onCloneProgress`로 받는다.
- 복제 부모 폴더의 처음 값은 워크스페이스 폴더다. 그 폴더 이름이 `workspaces`이면 부모 폴더를 쓴다(`Tt()`). 새 프로젝트의 처음 값은 `getDefaultCreateProjectParent()`가 준다. 그 값은 화면 코드에 없다.
- 원격 호스트에서는 네이티브 폴더 창을 쓰지 못한다. 그래서 `RemoteFileBrowser`(경로 조각, 거르기·경로 입력, 폴더 목록, 「Select folder」)로 호스트 폴더를 고른다. 목록은 `files.browseServerDir`가 준다.
- git이 아닌 폴더는 Orca가 `addNonGitFolder`로 따로 더한다. 여러 저장소를 담은 폴더는 안쪽 저장소를 찾아 묶어 더한다.

### loom(현재 코드)

- [AddRepoPopover.tsx](../../web/src/AddRepoPopover.tsx)는 경로를 글로만 받는다. `POST /api/repos {path}`를 부른다.
- `Repos::add`(`src/worktree.rs`)는 있는 git 폴더만 더한다. 같은 저장소의 다른 워크트리를 적어도 원래 체크아웃 한 줄이다(`ON CONFLICT DO NOTHING`, 같은 id를 돌려준다).
- 커밋이 없는 저장소(`git init` 직후)도 지금 더해진다. `worktree list`는 `HEAD 000…`을 준다. 그러나 `branches`가 비고 [새 워크트리]가 `fatal: invalid reference: main`으로 실패한다(2026-10-20 직접 시험).
- `git clone`은 비지 않은 대상 폴더를 거절한다. 실패하면 자기가 만든 폴더를 지운다. 미리 있던 빈 폴더는 그대로 둔다(직접 시험).
- `git::run`은 표준 입력과 `GIT_TERMINAL_PROMPT`를 정하지 않는다. 그래서 복제는 따로 `git::clone`을 쓴다. host 데몬은 `setsid nohup … </dev/null`로 돌아 제어 터미널이 없다(`scripts/host.sh` 24줄).
- loom 화면은 브라우저 페이지다. 브라우저는 호스트 절대 경로를 돌려주지 않는다. 그래서 Orca 원격 호스트와 같은 처지다. 데몬이 폴더 목록을 주어야 한다.

## 2. 제안 구조

화면 하나(사이드바 [+] 창)에 탭 셋을 둔다. Orca는 시작 화면에서 갈래를 고른 뒤 단계로 들어간다. 탭은 한 번 덜 누른다. 세 갈래는 같은 폴더 목록을 함께 쓴다.

### 백엔드

| 바꿀 곳 | 내용 |
|---|---|
| `src/worktree.rs` | `folders(path) -> FolderList`: 자식 폴더 이름과 `git` 표시(자식 안에 `.git`이 있음). 점으로 시작하는 폴더는 뺀다. 경로가 없으면 `~/loom`이다 |
| `src/worktree.rs` | `Repos::clone(url, parent)`: 대상은 `<parent>/<URL 마지막 마디에서 .git을 뺀 이름>`. 검사 뒤 `git clone -- <url> <대상>`, 그 뒤 `Repos::add` |
| `src/worktree.rs` | `Repos::create(parent, name)`: 이름 검사, 대상이 없어야 함, 폴더 만들기, `git init`, 그 뒤 `Repos::add`. 첫 커밋은 만들지 않는다 |
| `src/worktree.rs` `RepoView` | 더함: `has_commits: bool`(`git rev-parse --verify HEAD` 성공). 화면이 [새 워크트리]를 끄는 근거다. 빈 `branches`로 짐작하지 않는다 |
| `src/worktree.rs` `create` | 원래 체크아웃에 커밋이 없으면 git을 부르기 전에 `NoCommit`으로 거절한다. [새 워크트리]와 `loom spawn --worktree`(`daemon.rs` 946줄)가 같은 글을 받는다 |
| `src/worktree.rs` `RepoProblem` | 더함: `NotEmpty(path)`(대상이 있고 비어 있지 않음), `Exists(path)`(새 프로젝트 대상이 이미 있음), `BadName(name)`(빈 이름, `/`·`\`, `.`, `..`), `NoCommit`(커밋이 없는 저장소에서 워크트리 만들기). 복제·init 실패는 `Git(String)`을 다시 쓴다 |
| `src/git.rs` | `clone(url, target)`을 더한다. `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS` 빈 값, `SSH_ASKPASS_REQUIRE=never`, `GCM_INTERACTIVE=never`, 빈 표준 입력. 인증이 필요하면 창·질문 없이 실패 글을 돌려준다. SSH 호스트 키 확인은 그대로다. 다른 git 읽기(`run`)는 바꾸지 않는다 |
| `src/daemon.rs` | `GET /api/folders?path=`, `POST /api/repos/clone {url, parent}`, `POST /api/repos/create {parent, name}`. 둘 다 `RepoView`를 돌려준다. `POST /api/repos {path}`는 그대로 둔다 |
| `src/daemon.rs` 요청 타입 | `CloneRepo`, `CreateRepo`, `FolderList`를 `NewRepo` 옆에 두고 `ts-rs`로 내보낸다 |

폴더 목록을 `worktree.rs`에 둔 이유: 쓰는 곳이 저장소 추가 하나뿐이다. `git` 표시는 저장소 개념이다. 다른 쓰는 곳이 생기면 기반 모듈로 옮긴다.

### 화면

| 바꿀 곳 | 내용 |
|---|---|
| `web/src/AddRepoPopover.tsx` | 탭 셋과 탭마다 칸. 성공하면 지금처럼 `onAdded(repo)` |
| `web/src/FolderPicker.tsx`(새 파일) | 경로 칸과 폴더 목록. 행을 두 번 누르면 들어가고 `..`는 위로 간다. 세 탭이 함께 쓴다 |
| `web/src/App.tsx` 도구 줄 | `has_commits`가 거짓이면 [새 워크트리]를 끄고 옆에 이유 글을 보인다(`aria-describedby`) |
| `web/src/Tree.tsx` | 커밋이 없는 원래 체크아웃의 설명을 「원래 체크아웃」 대신 「커밋 없음」으로 보인다 |
| `web/src/api.ts` | `getFolders`, `cloneRepo`, `createRepo` |
| `web/src/locales/ko.ts`, `en.ts` | 탭 이름, 칸 이름, 오류 문구 |
| `docs/design.md` 「저장소와 워크트리」 | 「더하기」 줄을 세 갈래로 고친다. 20줄의 "경로를 글로 입력해 더한다"를 고친다 |

### 랩터-3 부품 목록

- 새 부품: 위 표의 함수 셋, 라우트 셋, 문제 종류 넷, `RepoView.has_commits` 필드 하나, 화면 파일 하나. 요구는 사용자 요청(Orca 흐름 도입)과 빈 저장소의 [새 워크트리] 실패(확인한 사실)다.
- 다시 쓰는 부품: `Repos::add`·`insert`(중복 막기), `worktree::view`, `git::run`, `default_workspace`, `popover.ts`, `.pop`·`.field` 모양.
- 지우는 부품: 글로만 받는 경로 칸은 `FolderPicker`의 경로 칸으로 바뀐다. `repo.pathHint` 문구를 지운다. 묶음의 `original: boolean`은 `kind`('original'·'unborn'·'linked') 하나로 바뀐다.
- 넣지 않는 부품: SSH·런타임 호스트, git이 아닌 폴더 추가, 안쪽 저장소 찾기, 복제 진행률 흐름, 최근 경로 목록.

### 부모 폴더 처음 값

`~/loom`(`worktree::loom_home`)이다. 사용자 승인(2026-10-20)으로 정했다. loom이 이미 만드는 폴더(`~/loom/workspaces`의 부모)다. 사람이 설정에서 고른 워크스페이스 폴더를 잘라 짐작하지 않는다. 데몬이 `GET /api/folders`(경로 없음)로 이 폴더를 준다. 화면에는 사본을 두지 않는다.

### 복제 상태와 기존 내용 보존

- 진행 중: 칸과 다른 탭을 끄고 버튼에 「복제하는 중…」을 보인다. 진행률 흐름은 더하지 않는다.
- 실패: 오류 글(git 오류 글 포함)을 창 안에 보인다. 칸 값은 남는다.
- 성공: 창을 닫고 새 저장소를 트리에 더해 연다(지금 `onAdded`와 같다).
- 보존: 대상 폴더가 있고 비어 있지 않으면 git을 부르지 않고 `NotEmpty`로 거절한다. loom은 실패 뒤 아무 폴더도 지우지 않는다. git이 자기가 만든 폴더만 지운다(확인한 사실). 미리 있던 빈 폴더는 남는다.
- 새 프로젝트도 같다. 대상이 있으면 `Exists`로 거절하고 그 폴더를 건드리지 않는다. 폴더를 만든 뒤 `git init`이 실패하면 가설: 빈 폴더가 남는다. 빈 폴더만 지울지는 구현 때 테스트로 정한다(`std::fs::remove_dir`는 빈 폴더만 지운다).

## 3. 필요한 검증

`src/worktree.rs` 테스트(`tempfile`, 네트워크 없이 로컬 경로를 복제 원본으로 쓴다):

- 폴더 목록: git 폴더 표시, 점 폴더 빼기, 없는 경로는 `NotFound`, 상대 경로는 `NotAbsolute`.
- 복제: 성공하면 `RepoView` 한 줄. 같은 URL을 다른 부모에 복제하면 다른 저장소다. 대상이 비지 않았으면 `NotEmpty`이고 원래 파일이 남는다. 원본이 없으면 `Git(...)`이고 새 폴더가 남지 않는다. 미리 있던 빈 폴더는 남는다.
- 새 프로젝트: 성공하면 git 저장소 하나. 대상이 있으면 `Exists`이고 그 폴더를 건드리지 않는다. `BadName` 경우들.
- 중복 등록: 이미 더한 폴더를 「기존 폴더」로 다시 더하면 같은 id다(지금 테스트가 덮는다).
- 빈 저장소: 새 프로젝트의 `has_commits`가 거짓이다. `create`가 `NoCommit`을 돌려주고 폴더를 만들지 않는다. 첫 커밋 뒤에는 참이고 워크트리가 만들어진다. 「기존 폴더」로 더한 빈 저장소도 같다.

검사 명령: `cargo fmt`, `memcap -m 8G cargo clippy --all-targets`, `memcap -m 8G cargo test`, `npm --prefix web run check`, `npm --prefix web run build`. 화면은 다른 `HOME`으로 데몬을 띄워 세 갈래를 손으로 시험한다.

## 4. 결정(사용자 승인, 2026-10-20)

- 탭 셋 창과 목업 1~6을 승인했다.
- 새 프로젝트는 `git init`만 한다. 첫 커밋 전 저장소는 [새 워크트리]를 끄고 이유를 보인다.
- 부모 폴더 처음 값은 `~/loom`이다.
- 복제는 진행 중 버튼만 보인다. 진행률 흐름은 넣지 않는다.
- Git 주소 칸은 `type="text"`다. `git@host:org/repo.git` 형식도 받는다.
- 폴더 이동은 두 번 누르기 대신 [›]·[..] 버튼으로 한다. 모두 Tab·Enter로 쓴다.

## 5. 점검 기록

design-stack 경로 B(제품 UI)다. 제품 토큰([styles.css](../../web/src/styles.css))과 [workspace.html](workspace.html) 모양이 방향이다. 그래서 Hallmark 단계를 건너뛰었다. `impeccable context`는 앞 코디가 이 파일에 돌렸다. Impeccable `operate.md`와 `craft-floor.md`를 읽었다.

- `impeccable detect`(첫 판): 7건. 고친 판에는 다시 돌리지 않았다(같은 파일에 한 번 규칙). 새로 더한 것은 같은 토큰의 도구 줄·묶음·끈 버튼이다.
  - `cramped-padding` 3건: 탭 묶음의 2px 안쪽 여백이다. 분할 버튼 모양이라 안쪽 탭 버튼이 여백을 가진다. 오탐으로 남겼다.
  - `gpt-thin-border-wide-shadow` 3건: 제품의 기존 `.pop`(1px 테두리, 28px 그림자)과 같다. 우선순위 1(제품 계약)이라 남겼다.
  - `flat-type-hierarchy` 1건: 12·13·14px는 제품 글자 크기와 같다. 남겼다.
- WIG: 고친 것은 장식 SVG의 `aria-hidden`, 캡처 그림의 `width`·`height`, URL 칸의 `type="url"`, 경로의 `translate="no"`, 끈 버튼 이유의 `aria-describedby`, 진행 중 창의 `aria-busy`다. 남긴 것은 탭 URL 동기화(작은 창이라 요구가 없음)와 Title Case(한국어 예외)다. 칸마다 `<label>`이 있다. 오류는 `role="alert"`, 진행 중 문구는 `…`로 끝난다.
- 렌더: 헤드리스 Chromium(Playwright 1243)으로 1480px 폭을 두 번 찍었다. 두 번째 판에서 4번 표시가 트리 글자에 붙어 옮겼다. 다른 겹침·잘림은 없다.
- 키보드(구현 때 지킬 것): 탭은 좌우 화살표로 옮긴다. 폴더 목록은 위아래 화살표로 고르고 Enter로 들어간다. Escape로 창을 닫는다(지금과 같다).
- 현재 화면: [sidebar-before.png](sidebar-before.png)을 다시 썼다. 지금 창(경로 칸 하나)을 연 캡처는 저장소에 없어 새로 그리지 않았다.
