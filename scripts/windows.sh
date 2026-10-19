#!/usr/bin/env sh
# 컨테이너 안에서 Windows용 loom.exe와 앱(loom-app.exe)을 검사하고 빌드한다. `npm run build`를 먼저 돌린다.
# 설치 파일(NSIS)은 GitHub Actions가 만든다. 근거: docs/design.md 「Windows 앱」.
set -eu
cd "$(dirname "$0")/.."
target=x86_64-pc-windows-msvc
cargo xwin clippy --release --target $target --all-targets -- -D warnings
cargo xwin build --release --target $target
# 앱 빌드(tauri-build)는 옆에 놓을 데몬 실행 파일이 있어야 한다.
mkdir -p app/binaries
cp target/$target/release/loom.exe app/binaries/loom-$target.exe
cargo xwin clippy --release --target $target --manifest-path app/Cargo.toml --target-dir target -- -D warnings
cargo xwin build --release --target $target --manifest-path app/Cargo.toml --target-dir target
