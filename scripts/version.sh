#!/bin/sh
root=$(dirname "$0")/..
base=$(sed -n 's/^version = "\([0-9]*\.[0-9]*\)\.[0-9]*"$/\1/p' "$root/Cargo.toml")
if bumped=$(git -C "$root" log -1 --format=%H -G'^version = ' -- Cargo.toml 2>/dev/null) && [ -n "$bumped" ]; then
  echo "$base.$(git -C "$root" rev-list --count "$bumped..HEAD")"
else
  sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml"
fi
