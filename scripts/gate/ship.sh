#!/usr/bin/env bash
# 병합 관문 배포: `loom merge`가 원래 체크아웃(main)에서 `ship.sh <branch> <worktree>`로 부른다. 검사는 check.sh가 이미 마쳤다.
# 끝 코드는 src/protocol.rs가 정한다: 0 병합·push·배포 확인, 10 담당이 고칠 일(아무것도 바꾸지 않음), 11 병합·push 뒤 배포 확인 실패, 그 밖은 실패.
# 0과 11이면 마지막 줄이 병합 커밋이다.
set -uo pipefail
branch=$1
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

# 미커밋 변경이 있으면 되돌릴 때 잃는다. 루트가 정할 일이다.
if [ -n "$(git status --porcelain)" ]; then echo "the main checkout has uncommitted changes"; exit 1; fi
before=$(git rev-parse HEAD)
git fetch --quiet origin main || { echo "git fetch failed"; exit 1; }
# 원격이 앞서면 따라간다. 갈라졌으면 사람이 정한다.
git merge --ff-only --quiet origin/main || { echo "main and origin/main diverged"; exit 1; }
if ! git merge-base --is-ancestor HEAD "$branch"; then
  echo "main moved: merge main into $branch again"
  exit 10
fi
if ! git merge --no-ff --no-edit "$branch"; then
  git merge --abort
  exit 10
fi
sha=$(git rev-parse HEAD)
if ! git push --quiet origin main; then
  # 원격이 그 사이에 움직였다. 이 병합을 되돌린다(위에서 미커밋 변경이 없음을 봤다).
  git reset --hard --quiet "$before"
  echo "push was rejected: main moved"
  exit 10
fi

# main이 바뀌면 git 훅(reference-transaction)이 host.sh로 사람이 쓰는 loom을 다시 빌드한다. 그 빌드가 이 커밋에 닿을 때까지 기다린다.
# 15분: host.sh 릴리스 빌드가 보통 몇 분이다(CI 담당자 브리프 DEPLOY).
sleep 20
if timeout 900 bash -c 'until [ "$(git -C ~/.loom/host-build rev-parse HEAD 2>/dev/null)" = "$0" ] && tail -1 ~/.loom/host.log | grep -q "^loom 0"; do sleep 5; done' "$sha"; then
  tail -1 ~/.loom/host.log
  echo "$sha"
  exit 0
fi
tail -30 ~/.loom/host.log
echo "$sha"
exit 11
