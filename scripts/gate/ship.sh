#!/usr/bin/env bash
# 병합 관문 배포: `loom merge`가 원래 체크아웃에서 `bash scripts/gate/ship.sh <branch> <sha>`로 부른다. 검사는 check.sh가 이미 그 커밋에서 마쳤다.
# 검토한 커밋 `sha`를 병합한다. 브랜치 이름은 병합 메시지에만 쓴다: 그 사이 브랜치가 움직여도 검토하지 않은 커밋이 들어가지 않는다.
# 끝 코드는 src/protocol.rs가 정한다: 0 병합·push·배포 확인, 10 담당이 고칠 일, 11 병합·push 뒤 배포 확인 실패, 그 밖은 실패.
# 10이면 병합 커밋을 남기지 않는다. 체크아웃이 origin을 빨리 감기로 따라갔을 수는 있다. 0과 11이면 마지막 줄이 병합 커밋이다.
set -uo pipefail
branch=$1
sha=$2
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
base=$(git symbolic-ref --quiet --short HEAD) || { echo "the main checkout is not on a branch"; exit 1; }

# 미커밋 변경이 있으면 되돌릴 때 잃는다. 루트가 정할 일이다.
if [ -n "$(git status --porcelain)" ]; then echo "the main checkout has uncommitted changes"; exit 1; fi
before=$(git rev-parse HEAD)
git fetch --quiet origin "$base" || { echo "git fetch failed"; exit 1; }
# 원격이 앞서면 따라간다. 갈라졌으면 사람이 정한다.
git merge --ff-only --quiet "origin/$base" || { echo "$base and origin/$base diverged"; exit 1; }
if ! git merge-base --is-ancestor HEAD "$sha"; then
  echo "$base moved: merge $base into $branch again"
  exit 10
fi
if ! git merge --no-ff -m "Merge branch '$branch'" "$sha"; then
  git merge --abort
  exit 10
fi
merged=$(git rev-parse HEAD)
if ! git push --quiet origin "$base"; then
  # 원격이 그 사이에 움직였다. 이 병합을 되돌린다(위에서 미커밋 변경이 없음을 봤다).
  git reset --hard --quiet "$before"
  echo "push was rejected: $base moved"
  exit 10
fi

# 체크아웃이 바뀌면 git 훅(reference-transaction)이 host.sh로 사람이 쓰는 loom을 다시 빌드한다. 그 빌드가 이 커밋에 닿을 때까지 기다린다.
# 15분: host.sh 릴리스 빌드가 보통 몇 분이다(CI 담당자 브리프 DEPLOY).
sleep 20
if timeout 900 bash -c 'until [ "$(git -C ~/.loom/host-build rev-parse HEAD 2>/dev/null)" = "$0" ] && tail -1 ~/.loom/host.log | grep -q "^loom 0"; do sleep 5; done' "$merged"; then
  tail -1 ~/.loom/host.log
  echo "$merged"
  exit 0
fi
tail -30 ~/.loom/host.log
echo "$merged"
exit 11
