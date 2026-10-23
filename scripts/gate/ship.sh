#!/usr/bin/env bash
# 병합 관문 배포: `loom merge`가 원래 체크아웃에서 `bash scripts/gate/ship.sh <branch> <sha>`로 부른다. 검사는 check.sh가 이미 그 커밋에서 마쳤다.
# 검토한 커밋 `sha`를 병합한다. 브랜치 이름은 병합 메시지에만 쓴다: 그 사이 브랜치가 움직여도 검토하지 않은 커밋이 들어가지 않는다.
# 끝 코드는 src/protocol.rs가 정한다: 0 병합·push·배포 확인, 10 담당이 고칠 일(대상과 충돌, 대상이 움직임), 11 병합·push 뒤 배포 확인 실패,
# 12 멈춤(push 거절·fetch 실패처럼 다시 부르면 될 일, 또는 관문 밖에서 이미 병합됨: 관문이 다시 부르거나 루트에 알린다), 그 밖은 실패.
# 10과 12는 게시하지 않은 병합 커밋을 남기지 않는다. 0과 11이면 마지막 줄이 병합 커밋이다.
# 다시 불러도 된다(2026-10-23 코디 결정 7, 재작업 3 결정 1): 판단은 게시된 대상(origin)으로 한다.
# - `sha`가 이미 origin에 있으면 병합·push 없이 그 병합 커밋의 배포만 확인한다.
# - `sha`가 이 체크아웃에만 있으면(앞 실행이 push 전에 끝났다) 이 체크아웃을 origin으로 되돌리고 처음부터 병합·push한다.
set -uo pipefail
branch=$1
sha=$2
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
base=$(git symbolic-ref --quiet --short HEAD) || { echo "the main checkout is not on a branch"; exit 1; }
# 첫 부모 줄에서 `sha`를 둘째 이후 부모로 가진 병합 커밋(관문 병합).
merge_of() { git log --first-parent --format='%H %P' "$1" | awk -v h="$sha" '{ for (i = 3; i <= NF; i++) if ($i == h) { print $1; exit } }'; }

# 미커밋 변경이 있으면 되돌릴 때 잃는다. 루트가 정할 일이다.
if [ -n "$(git status --porcelain)" ]; then echo "the main checkout has uncommitted changes"; exit 1; fi
git fetch --quiet origin "$base" || { echo "git fetch failed; call loom merge again"; exit 12; }
published=origin/$base

if git merge-base --is-ancestor "$sha" "$published"; then
  merged=$(merge_of "$published")
  if [ -z "$merged" ]; then echo "$sha is already in $published without a gate merge commit (merged outside the gate); tell the root"; exit 12; fi
  echo "resume: $merged already merges $sha and is published"
  git merge --ff-only --quiet "$published" || { echo "$base and $published diverged"; exit 1; }
else
  if git merge-base --is-ancestor "$sha" HEAD; then
    # 앞 실행의 게시하지 않은 병합. 첫 부모 줄의 게시하지 않은 커밋이 그 병합 하나일 때만 되돌린다. 다른 커밋은 잃으면 안 된다.
    local_merge=$(merge_of HEAD)
    unpublished=$(git rev-list --first-parent "$published..HEAD")
    if [ -z "$local_merge" ] || [ "$unpublished" != "$local_merge" ]; then
      echo "$base has unpublished commits besides the merge of $sha: $(echo $unpublished)"
      exit 1
    fi
    echo "resume: dropping the unpublished merge $local_merge and merging again on $published"
    git reset --hard --quiet "$published"
  fi
  # 원격이 앞서면 따라간다. 갈라졌으면 사람이 정한다.
  git merge --ff-only --quiet "$published" || { echo "$base and $published diverged"; exit 1; }
  before=$(git rev-parse HEAD)
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
    # 게시하지 못한 병합을 남기지 않는다(위에서 미커밋 변경이 없음을 봤다). 관문이 다시 부르면 처음부터 한다.
    git reset --hard --quiet "$before"
    echo "push was rejected; call loom merge again"
    exit 12
  fi
fi

# 체크아웃이 바뀌면 git 훅(reference-transaction)이 host.sh로 사람이 쓰는 loom을 다시 빌드한다.
# 배포 확인: 빌드한 커밋이 이 병합을 담고(뒤 커밋이 먼저 배포됐어도 된다), host.sh가 그 빌드 뒤에 데몬을 다시 띄웠다.
# 15분: host.sh 릴리스 빌드가 보통 몇 분이다(CI 담당자 브리프 DEPLOY).
sleep 20
deployed() {
  local built=~/.loom/host-build
  git -C "$built" merge-base --is-ancestor "$merged" HEAD 2>/dev/null &&
    tail -1 ~/.loom/host.log | grep -q "^loom 0" &&
    [ ~/.loom/host.log -nt "$built/.git/HEAD" ]
}
for _ in $(seq 180); do
  if deployed; then
    tail -1 ~/.loom/host.log
    echo "$merged"
    exit 0
  fi
  sleep 5
done
tail -30 ~/.loom/host.log
echo "$merged"
exit 11
