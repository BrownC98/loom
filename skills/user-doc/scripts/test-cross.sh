#!/usr/bin/env bash
set -eu
here=$(dirname "$(realpath "$0")"); tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
page() { cat <<HTML
<!doctype html><html><head><meta charset="utf-8"><style>
body { margin: 20px; font: 14px sans-serif }
.page { position: relative; width: 800px; height: 260px; border: 1px solid #999 }
$(cat "$here/../callouts/callouts.css")
</style></head><body>
<div class="fig"><h2>그림</h2>
<div class="page"><div class="mk" data-n="1" style="position:absolute; top:20px; left:60px; width:120px; height:30px">첫째</div><div class="mk" data-n="2" style="position:absolute; top:20px; left:600px; width:120px; height:30px">둘째</div></div>
<div class="calls"><div><b>1</b><span>첫째 설명</span></div><div><b>2</b><span>둘째 설명</span></div></div></div>
<script>$(cat "$here/../callouts/callouts.js")</script>
$1
</body></html>
HTML
}
page '' > "$tmp/ok.html"
page '<script>setTimeout(() => { const [a, b] = document.querySelectorAll(".lead-svg polyline"), pa = a.points, pb = b.points, f = p => p.getItem(0), l = p => p.getItem(p.numberOfItems - 1), s = (p, q) => `${f(p).x},${f(p).y} ${l(q).x},${l(q).y}`; const x = s(pa, pb), y = s(pb, pa); a.setAttribute("points", x); b.setAttribute("points", y); }, 300)</script>' > "$tmp/cross.html"
node "$here/check-callouts.cjs" "$tmp/ok.html" >&2 || { echo "test-cross: 정상 문서가 실패했다" >&2; exit 1; }
if out=$(node "$here/check-callouts.cjs" "$tmp/cross.html"); then echo "test-cross: 엇갈린 문서가 통과했다" >&2; exit 1; fi
echo "$out" | grep -q 'crosses leader' || { echo "test-cross: 실패 이유에 crosses leader가 없다: $out" >&2; exit 1; }
echo "test-cross: ok"
