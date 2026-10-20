#!/bin/sh
# 제품 버전을 찍는다. MAJOR.MINOR는 루트 Cargo.toml, PATCH는 그 version 줄을 마지막으로 바꾼 커밋 뒤의 커밋 수다.
# MAJOR.MINOR를 올리면 PATCH는 0부터 다시 센다. git 기록을 읽지 못하면 Cargo.toml 값을 그대로 쓴다.
root=$(dirname "$0")/..
base=$(sed -n 's/^version = "\([0-9]*\.[0-9]*\)\.[0-9]*"$/\1/p' "$root/Cargo.toml")
if bumped=$(git -C "$root" log -1 --format=%H -G'^version = ' -- Cargo.toml 2>/dev/null) && [ -n "$bumped" ]; then
  echo "$base.$(git -C "$root" rev-list --count "$bumped..HEAD")"
else
  sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml"
fi
