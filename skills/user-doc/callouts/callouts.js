// Callout overlays for user-doc mockups (mockup 93 v17). Include at the end of <body>; runs on load and on resize (120 ms debounce).
// Markup: inside each .fig, targets <el class="mk" data-n="N"> (add .new for a dashed box), frames .page/.xlwin/.shotwrap,
// and <div class="calls"><div><b>1</b><span>text</span></div>...</div> (item N = text for data-n N).
// Output (direct children of .fig): .ov-box[data-n] (z 60), .lead-svg > polyline[data-n] + circle (z 61), .ov-badge[data-n] (z 62), .co[data-n] (z 63).
// .mk only marks the target; it has no outline and no ::after. fit() zooms .page/.shotwrap to min(0.8,(figW-290)/1360) and .xlwin to fit.
// 말풍선은 화면 글자·컨트롤과 겹치지 않는 빈 자리에 두고, 상자 테두리에서 말풍선까지 지시선을 긋는다.
// 지시선도 글자·컨트롤·다른 상자·다른 말풍선을 지나지 않는 자리 중 가장 짧은 것을 고른다. 화면 틀 안의 빈 바탕도 자리다.
// Boxes: target grown 5 px, clipped to the enclosing table cell and to the frame (inset 1). A td/th target = cell inset 1. An empty .shotwrap .mk = box exactly at the mark.
// Bubbles may lie on empty canvas inside a frame; if they do not fit, the .fig bottom padding grows by 80 px steps. A .fig without .calls (the clean copy) is only zoomed.
(() => {
  const GUT = 290; // 그림 오른쪽 빈 자리(px). 오른쪽 끝 상자의 말풍선이 여기에 선다. 화면 그림은 이 폭을 남기고 줄어든다.
  const fit = fig => {
    const room = fig.clientWidth - GUT;
    for (const e of fig.querySelectorAll('.page, .shotwrap')) e.style.zoom = Math.min(0.8, room / (e.matches('.shotwrap') ? 1394 : 1360));
    for (const e of fig.querySelectorAll('.xlwin')) { e.style.zoom = 1; e.style.zoom = Math.min(1, room / e.offsetWidth); }
  };
  const hit = (a, b) => a.l < b.r && b.l < a.r && a.t < b.b && b.t < a.b;
  const inside = (a, b) => a.l >= b.l && a.r <= b.r && a.t >= b.t && a.b <= b.b;
  const grow = (r, d) => ({ l: r.l - d, t: r.t - d, r: r.r + d, b: r.b + d });
  // 선분과 사각형이 만나는지(Liang-Barsky).
  const cross = (x1, y1, x2, y2, r) => {
    let t0 = 0, t1 = 1; const dx = x2 - x1, dy = y2 - y1;
    for (const [p, q] of [[-dx, x1 - r.l], [dx, r.r - x1], [-dy, y1 - r.t], [dy, r.b - y1]]) {
      if (p === 0) { if (q < 0) return false; continue; }
      const t = q / p; if (p < 0) { if (t > t1) return false; if (t > t0) t0 = t; } else { if (t < t0) return false; if (t < t1) t1 = t; }
    }
    return true;
  };
  // 제품마다 컨트롤 클래스가 다르다. 목업은 <body data-ui=".abtn, .seg">로 더한다. scripts/check-callouts.cjs도 같은 값을 읽는다.
  const UI = ['img, svg, button, input, select, textarea', document.body.dataset.ui].filter(Boolean).join(', ');
  const uiRects = (fig, rel) => {
    const out = [];
    for (const r of [...fig.children].filter(e => !e.matches('.co, .lead-svg, .calls'))) {
      const w = document.createTreeWalker(r, NodeFilter.SHOW_TEXT);
      for (let n; (n = w.nextNode());) {
        if (!n.textContent.trim()) continue;
        const g = document.createRange(); g.selectNodeContents(n);
        for (const q of g.getClientRects()) if (q.width && q.height) out.push(Object.assign(rel(q), { txt: 1 }));
      }
      for (const e of r.querySelectorAll(UI)) out.push(rel(e.getBoundingClientRect()));
    }
    return out;
  };
  // 자리가 모자라면 그림 아래 여백을 80px씩 늘려 다시 놓는다.
  const layout = () => { for (const fig of document.querySelectorAll('.fig')) {
    const src = fig.querySelector(':scope > .calls'); if (!src) { fit(fig); continue; } // 그림만 있는 칸도 같은 배율로 줄인다.
    fig.style.paddingBottom = ''; const base = parseFloat(getComputedStyle(fig).paddingBottom);
    for (let x = 80; place(fig, src) && x <= 1200; x += 80) fig.style.paddingBottom = base + x + 'px';
  } };
  const place = (fig, src) => {
    fig.querySelectorAll(':scope > .co, :scope > .lead-svg, :scope > .ov-box, :scope > .ov-badge').forEach(e => e.remove());
    fit(fig);
    const texts = [...src.querySelectorAll(':scope > div > span')].map(s => s.innerHTML);
    src.style.display = 'none';
    const F = fig.getBoundingClientRect();
    const rel = r => ({ l: r.left - F.left, t: r.top - F.top, r: r.right - F.left, b: r.bottom - F.top });
    const marks = [...fig.querySelectorAll('.mk[data-n]')];
    // 상자는 대상 둘레 5px. 표 칸이 대상이면 칸 안쪽 1px, 칸 안의 것이면 그 칸 안쪽 1px까지로 줄여 이웃 칸으로 넘지 않는다.
    // 캡처 위의 빈 표시(.shotwrap .mk)는 캡처의 고른 색 띠와 틀 여백에 맞춘 자리라 그대로 쓴다. 상자는 화면 틀(.page·.xlwin·.shotwrap) 안쪽 1px도 넘지 않는다.
    const clip = (g, e) => { if (!e || !fig.contains(e)) return g; const z = grow(rel(e.getBoundingClientRect()), -1); return { l: Math.max(g.l, z.l), t: Math.max(g.t, z.t), r: Math.min(g.r, z.r), b: Math.min(g.b, z.b) }; };
    const outl = marks.map(m => { const r = rel(m.getBoundingClientRect()), frame = m.closest('.page, .xlwin, .shotwrap');
      if (m.matches('td, th')) return clip(grow(r, -1), frame);
      if (m.matches('.shotwrap .mk')) return r;
      return clip(clip(grow(r, 5), m.parentElement.closest('td, th')), frame); });
    const ui = uiRects(fig, rel);
    // 상자와 배지는 대상의 레이아웃을 바꾸지 않게 그림 위에 따로 그린다(맨 위 층).
    const badges = [];
    marks.forEach((m, i) => {
      const o = outl[i], bx = document.createElement('div'); bx.className = 'ov-box' + (m.classList.contains('new') ? ' new' : ''); bx.dataset.n = m.dataset.n;
      Object.assign(bx.style, { left: o.l + 'px', top: o.t + 'px', width: o.r - o.l + 'px', height: o.b - o.t + 'px' }); fig.appendChild(bx);
      // 배지는 상자 모서리 중 글자·컨트롤·다른 배지를 덮지 않는 곳에 둔다.
      const spots = [[o.l, o.t], [o.r, o.t], [o.l, o.b], [o.r, o.b], [(o.l + o.r) / 2, o.t - 11], [(o.l + o.r) / 2, o.b + 11], [o.l - 11, o.t + 11], [o.r + 11, o.t + 11]].map(([x, y]) => ({ l: x - 11, t: y - 11, r: x + 11, b: y + 11 }));
      const ocx = (o.l + o.r) / 2, ocy = (o.t + o.b) / 2, cont = u => !u.txt && u.l <= ocx && ocx <= u.r && u.t <= ocy && ocy <= u.b; // 상자를 감싼 틀(캡처 그림 등)만 덮어도 된다. 글자는 늘 피한다.
      const area = (a, b) => Math.max(0, Math.min(a.r, b.r) - Math.max(a.l, b.l)) * Math.max(0, Math.min(a.b, b.b) - Math.max(a.t, b.t));
      const cost = r => (r.l < 0 || r.t < 0 || r.r > F.width ? 1e9 : 0) + ui.reduce((s, u) => s + (cont(u) ? 0 : area(r, u)), 0) + badges.reduce((s, u) => s + 10 * area(grow(r, 2), u), 0)
        + outl.reduce((s, u, j) => s + (j !== i && hit(r, u) && !(u.l <= r.l && r.r <= u.r && u.t <= r.t && r.b <= u.b) ? 50 : 0), 0);
      const sp = spots.reduce((a, r) => (cost(r) < cost(a) ? r : a)); badges.push(sp);
      const bd = document.createElement('div'); bd.className = 'ov-badge'; bd.dataset.n = bd.textContent = m.dataset.n;
      Object.assign(bd.style, { left: sp.l + 'px', top: sp.t + 'px' }); fig.appendChild(bd);
    });
    const boxes = outl;
    const frames = [...fig.querySelectorAll('.page, .xlwin, .shotwrap')];
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); svg.setAttribute('class', 'lead-svg');
    svg.setAttribute('width', F.width); svg.setAttribute('height', F.height); fig.appendChild(svg);
    // 순서를 바꿔 여러 번 놓아 본다. 막힌 말풍선이 가장 적고, 그다음 가장 긴 지시선이 가장 짧은 순서를 쓴다(점검의 지시선 길이 상한, Q643).
    const attempt = order => {
    fig.querySelectorAll(':scope > .co').forEach(e => e.remove()); svg.replaceChildren();
    const placed = [], segs = [], seen = new Set(), pad = 4; let fails = 0, longest = 0;
    order.forEach(i => { const m = marks[i];
      const n = +m.dataset.n; if (seen.has(n) || !texts[n - 1]) return; seen.add(n);
      const co = document.createElement('div'); co.className = 'co'; co.dataset.n = n; co.innerHTML = `<b>${n}</b><span>${texts[n - 1]}</span>`;
      // 잰 폭이 그린 폭과 같게 테두리까지 센다. 문서의 box-sizing이 content-box면 말풍선이 잰 자리보다 넓어 상자를 덮었다.
      Object.assign(co.style, { left: '0px', top: '0px', boxSizing: 'border-box' }); fig.appendChild(co);
      const own = m.closest('.page, .xlwin, .shotwrap');
      const other = frames.filter(e => e !== own).map(e => rel(e.getBoundingClientRect()));
      const k = outl[i];
      // 지시선이 피할 것: 이 상자 안의 것과 이 상자를 감싼 큰 틀(캡처 그림, 엑셀 칸 전체)은 뺀다. 그 안의 글자는 따로 들어 있어 피한다.
      const cx = (k.l + k.r) / 2, cy = (k.t + k.b) / 2, holds = o => o.l <= cx && cx <= o.r && o.t <= cy && cy <= o.b;
      const lineObs = ui.filter(o => !inside(o, k) && !holds(o)).concat(outl.filter((o, j) => j !== i && !holds(o)), badges.filter(b => !hit(b, grow(k, 1))), other);
      // 2026-10-23 사람(Q643) "선이 너무 길어서 시선이 움직이는 길이가 길어져 피로함": 말풍선은 틀 밖 열이 아니라 상자 가까운 빈 자리(틀 안 바탕 포함)에 둔다.
      const bubObs = ui.concat(boxes, badges);
      const segFree = (x1, y1, x2, y2) => { const d = Math.hypot(x2 - x1, y2 - y1); if (d < 3) return true;
        const ux = (x2 - x1) / d, uy = (y2 - y1) / d; return ![...lineObs, ...placed].some(o => cross(x1 + ux * 2, y1 + uy * 2, x2 - ux * 2, y2 - uy * 2, o)); };
      const plen = p => p.reduce((s, q, z) => z ? s + Math.hypot(q[0] - p[z - 1][0], q[1] - p[z - 1][1]) : 0, 0);
      // 상자 테두리에서 말풍선까지의 길: 곧은 선, 막히면 상자 위·아래로 나가 가로로 꺾는 선.
      const route = c => {
        const x1 = Math.min(Math.max((c.l + c.r) / 2, k.l), k.r), y1 = Math.min(Math.max((c.t + c.b) / 2, k.t), k.b);
        const st = [[x1, y1], [Math.min(Math.max(x1, c.l), c.r), Math.min(Math.max(y1, c.t), c.b)]];
        if (segFree(...st[0], ...st[1])) return st;
        // 막히면 상자 테두리의 다른 점(8px 간격)에서 말풍선의 가장 가까운 점까지 곧은 선을 본다. 이웃 상자 옆의 작은 상자에서 선이 이웃을 피한다.
        let line = null;
        for (const [a, b] of [[k.l, k.t, k.r, k.t], [k.l, k.b, k.r, k.b], [k.l, k.t, k.l, k.b], [k.r, k.t, k.r, k.b]].flatMap(([xa, ya, xb, yb]) => { const L = Math.hypot(xb - xa, yb - ya), m = Math.max(1, Math.round(L / 8)); return Array.from({ length: m + 1 }, (_, z) => [xa + (xb - xa) * z / m, ya + (yb - ya) * z / m]); })) {
          const p = [[a, b], [Math.min(Math.max(a, c.l), c.r), Math.min(Math.max(b, c.t), c.b)]];
          if ((!line || plen(p) < plen(line)) && segFree(...p[0], ...p[1])) line = p;
        }
        if (line) return line;
        const yc = c.t + 12, side = c.l >= k.r ? c.l : c.r <= k.l ? c.r : null; if (side === null) return null;
        if (yc >= k.t && yc <= k.b) return null;
        const ye = yc < k.t ? k.t : k.b; let best = null;
        for (let xv = k.l + 6; xv <= k.r - 6; xv += 10) {
          const p = [[xv, ye], [xv, yc], [side, yc]];
          if (segFree(...p[0], ...p[1]) && segFree(...p[1], ...p[2]) && (!best || plen(p) < plen(best))) best = p;
        }
        return best;
      };
      const free = c => c.l >= 0 && c.r <= F.width && c.t >= 40 && c.b <= F.height - 4 && ![...bubObs, ...placed].some(o => hit(grow(c, pad), o)) && !segs.some(q => cross(...q, grow(c, 2)));
      // 좁은 틈에도 들어가게 말풍선 폭을 세 가지로 재 본다. 좁으면 높아진다.
      const w0 = co.offsetWidth, sizes = [...new Set([w0, Math.round(w0 * 0.75), Math.round(w0 * 0.55)])].map(w => { co.style.width = w + 'px'; return [w, co.offsetHeight]; });
      const cands = [];
      for (const [w, h] of sizes) {
        for (let d = 6; d <= 120; d += 6)
          for (const [x, y] of [[k.r + d, k.t], [k.l, k.b + d], [k.l - w - d, k.t], [k.l, k.t - h - d], [k.r + d, k.b - h], [k.r - w, k.b + d], [k.r - w, k.t - h - d], [k.l - w - d, k.b - h]])
            cands.push({ l: x, t: y, r: x + w, b: y + h });
        // 가까운 자리가 막히면 그림 안의 다른 빈 자리(16px 격자)도 본다.
        for (let y = 40; y + h < F.height; y += 16) for (let x = 0; x + w < F.width; x += 16) cands.push({ l: x, t: y, r: x + w, b: y + h });
      }
      // 상자와 말풍선 사이 거리는 지시선 길이의 하한이다. 가까운 자리부터 보고, 하한이 지금 가장 짧은 선보다 길면 멈춘다.
      const gap = c => Math.hypot(Math.max(0, c.l - k.r, k.l - c.r), Math.max(0, c.t - k.b, k.t - c.b));
      const pick = list => { let bp = null, bc = null; for (const c of list.map(c => [gap(c), c]).sort((a, b) => a[0] - b[0]).map(([, c]) => c)) { if (bp && gap(c) >= plen(bp)) break; if (!free(c)) continue; const p = route(c); if (p && (!bp || plen(p) < plen(bp))) { bp = p; bc = c; } } return bc && { c: bc, p: bp }; };
      const best = pick(cands);
      // 빈 자리가 없으면 말풍선을 만들지 않는다. 그림 아래 여백을 늘려 다시 놓고, 그래도 없으면 점검이 빠진 번호로 실패시킨다.
      if (!best) { fails++; co.remove(); console.error(`callouts: no place for bubble ${n}`); return; }
      const { c: bc, p: path } = best; longest = Math.max(longest, plen(path));
      Object.assign(co.style, { left: bc.l + 'px', top: bc.t + 'px', width: bc.r - bc.l + 'px' }); placed.push(bc);
      const ln = document.createElementNS('http://www.w3.org/2000/svg', 'polyline');
      ln.setAttribute('points', path.map(q => q.join(',')).join(' ')); ln.dataset.n = n; svg.appendChild(ln);
      const dot = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
      dot.setAttribute('cx', path[0][0]); dot.setAttribute('cy', path[0][1]); dot.setAttribute('r', 2.5); svg.appendChild(dot);
      for (let z = 1; z < path.length; z++) segs.push([...path[z - 1], ...path[z]]);
    });
    return { fails, longest }; };
    const idx = marks.map((_, i) => i);
    const orders = [idx, [...idx].sort((a, b) => outl[b].r - outl[a].r), [...idx].sort((a, b) => outl[a].t - outl[b].t), [...idx].reverse()];
    const cmp = (a, b) => a.fails - b.fails || a.longest - b.longest;
    let bestO = null, bestR = null;
    for (const o of orders) { const r = attempt(o); if (!bestR || cmp(r, bestR) < 0) { bestR = r; bestO = o; } }
    if (bestO !== orders[orders.length - 1]) attempt(bestO);
    return bestR.fails;
  };
  // 그림이 다 읽힌 뒤에 놓고, 창 폭이 바뀌거나 웹 글꼴이 늦게 들어와 글자 폭이 바뀌면 다시 놓는다.
  // 글꼴: 93 목업 실측(960px)에서 load 1초 뒤 글꼴이 들어와 상자가 대상에서 2~3px 어긋났다.
  document.fonts?.addEventListener('loadingdone', () => dispatchEvent(new Event('resize')));
  let tm; addEventListener('resize', () => { clearTimeout(tm); tm = setTimeout(layout, 120); });
  if (document.readyState === 'complete') layout(); else addEventListener('load', layout);
})();
