#!/usr/bin/env bash
set -euo pipefail

home=${1:?usage: stop-test-daemon.sh <test HOME>}
home=$(realpath -m "$home")
if [ "$home" = "$(realpath -m "$HOME")" ]; then
    echo "stop-test-daemon: $home is your own HOME" >&2
    exit 2
fi

holders() {
    grep -lzxF -e "HOME=$home" -e "HOME=$home/" /proc/[0-9]*/environ 2>/dev/null |
        cut -d/ -f3 || true
}

show() {
    ps -o pid=,args= -p "$(IFS=,; echo "$*")" | cut -c1-160 || true
}

mapfile -t pids < <(holders)
if [ "${#pids[@]}" -gt 0 ]; then
    echo "stopping:"
    show "${pids[@]}"
    kill -TERM "${pids[@]}" 2>/dev/null || true
    for _ in $(seq 50); do
        mapfile -t pids < <(holders)
        [ "${#pids[@]}" -eq 0 ] && break
        sleep 0.1
    done
    if [ "${#pids[@]}" -gt 0 ]; then
        kill -KILL "${pids[@]}" 2>/dev/null || true
        sleep 0.2
    fi
fi

mapfile -t left < <(holders)
if [ "${#left[@]}" -gt 0 ]; then
    echo "stop-test-daemon: processes with HOME=$home remain:" >&2
    show "${left[@]}" >&2
    exit 1
fi
echo "stop-test-daemon: no process has HOME=$home"
