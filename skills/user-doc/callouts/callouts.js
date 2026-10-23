// Callout overlays for user-doc mockups (mockup 93 v17). Include at the end of <body>; runs on load and on resize (120 ms debounce).
// Markup: inside each .fig, targets <el class="mk" data-n="N"> (add .new for a dashed box), frames .page/.xlwin/.shotwrap,
// and <div class="calls"><div><b>1</b><span>text</span></div>...</div> (item N = text for data-n N).
// Output (direct children of .fig): .ov-box[data-n] (z 60), .lead-svg > polyline[data-n] + circle (z 61), .ov-badge[data-n] (z 62), .co[data-n] (z 63).
// .mk only marks the target; it has no outline and no ::after. fit() zooms .page/.shotwrap to min(0.8,(figW-290)/1360) and .xlwin to fit.
// 말풍선은 화면 글자·컨트롤과 겹치지 않는 빈 자리에 두고, 상자 테두리에서 말풍선까지 지시선을 긋는다.
// 지시선도 글자·컨트롤·다른 상자·다른 말풍선을 지나지 않는 자리 중 가장 짧은 것을 고른다. 가까운 빈 자리가 없으면 그림 오른쪽 말풍선 열에 둔다.
// Boxes: target grown 5 px, clipped to the enclosing table cell and to the frame (inset 1). A td/th target = cell inset 1. An empty .shotwrap .mk = box exactly at the mark.
// Bubbles never lie inside a frame; if they do not fit, the .fig bottom padding grows by 80 px steps. A .fig without .calls (the clean copy) is only zoomed.
(() => {
  const GUT = 290; // 말풍선 열(px). 화면 그림은 이 열을 남기고 줄어든다.
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
  // 말풍선은 화면 틀 밖에만 둔다. 자리가 모자라면 그림 아래 여백을 80px씩 늘려 다시 놓는다.
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
    const gutterX = Math.max(...frames.map(e => rel(e.getBoundingClientRect()).r)) + 16;
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); svg.setAttribute('class', 'lead-svg');
    svg.setAttribute('width', F.width); svg.setAttribute('height', F.height); fig.appendChild(svg);
    // 한 번 놓아 보고 막힌 말풍선이 있으면 순서를 바꿔 다시 놓는다. 가장 덜 막힌 순서를 쓴다.
    const attempt = order => {
    fig.querySelectorAll(':scope > .co').forEach(e => e.remove()); svg.replaceChildren();
    const placed = [], segs = [], seen = new Set(), pad = 4; let fails = 0;
    order.forEach(i => { const m = marks[i];
      const n = +m.dataset.n; if (seen.has(n) || !texts[n - 1]) return; seen.add(n);
      const co = document.createElement('div'); co.className = 'co'; co.dataset.n = n; co.innerHTML = `<b>${n}</b><span>${texts[n - 1]}</span>`;
      co.style.left = '0px'; co.style.top = '0px'; fig.appendChild(co);
      const own = m.closest('.page, .xlwin, .shotwrap');
      const other = frames.filter(e => e !== own).map(e => rel(e.getBoundingClientRect()));
      const k = outl[i];
      // 지시선이 피할 것: 이 상자 안의 것과 이 상자를 감싼 큰 틀(캡처 그림, 엑셀 칸 전체)은 뺀다. 그 안의 글자는 따로 들어 있어 피한다.
      const cx = (k.l + k.r) / 2, cy = (k.t + k.b) / 2, holds = o => o.l <= cx && cx <= o.r && o.t <= cy && cy <= o.b;
      const lineObs = ui.filter(o => !inside(o, k) && !holds(o)).concat(outl.filter((o, j) => j !== i && !holds(o)), badges.filter(b => !hit(b, grow(k, 1))), other);
      const bubObs = ui.concat(boxes, badges, frames.map(e => rel(e.getBoundingClientRect()))); // 말풍선은 화면 틀(.page·.xlwin·.shotwrap) 밖에만 둔다.
      const segFree = (x1, y1, x2, y2) => { const d = Math.hypot(x2 - x1, y2 - y1); if (d < 3) return true;
        const ux = (x2 - x1) / d, uy = (y2 - y1) / d; return ![...lineObs, ...placed].some(o => cross(x1 + ux * 2, y1 + uy * 2, x2 - ux * 2, y2 - uy * 2, o)); };
      const plen = p => p.reduce((s, q, z) => z ? s + Math.hypot(q[0] - p[z - 1][0], q[1] - p[z - 1][1]) : 0, 0);
      // 상자 테두리에서 말풍선까지의 길: 곧은 선, 막히면 상자 위·아래로 나가 가로로 꺾는 선.
      const route = c => {
        const x1 = Math.min(Math.max((c.l + c.r) / 2, k.l), k.r), y1 = Math.min(Math.max((c.t + c.b) / 2, k.t), k.b);
        const st = [[x1, y1], [Math.min(Math.max(x1, c.l), c.r), Math.min(Math.max(y1, c.t), c.b)]];
        if (segFree(...st[0], ...st[1])) return st;
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
      let w = co.offsetWidth, h = co.offsetHeight; co.style.width = w + 'px';
      const cands = [];
      for (let d = 6; d <= 120; d += 6)
        for (const [x, y] of [[k.r + d, k.t], [k.l, k.b + d], [k.l - w - d, k.t], [k.l, k.t - h - d], [k.r + d, k.b - h], [k.r - w, k.b + d], [k.r - w, k.t - h - d], [k.l - w - d, k.b - h]])
          cands.push({ l: x, t: y, r: x + w, b: y + h });
      // 가까운 자리가 막히면 그림 안의 다른 빈 자리(16px 격자)도 본다.
      for (let y = 40; y + h < F.height; y += 16) for (let x = 0; x + w < F.width; x += 16) cands.push({ l: x, t: y, r: x + w, b: y + h });
      const pick = list => { let bp = null, bc = null; for (const c of list) { if (!free(c)) continue; const p = route(c); if (p && (!bp || plen(p) < plen(bp))) { bp = p; bc = c; } } return bc && { c: bc, p: bp }; };
      let best = pick(cands);
      if (!best) {
        // 말풍선 열: 이 틀 오른쪽. 선이 막히지 않는 높이 중 가장 짧은 것.
        const gx = Math.min(rel(own.getBoundingClientRect()).r + 16, gutterX);
        co.style.width = ''; co.style.maxWidth = Math.min(250, F.width - gx) + 'px'; w = co.offsetWidth; h = co.offsetHeight; co.style.width = w + 'px';
        const g = []; for (let y = 40; y + h < F.height; y += 4) g.push({ l: gx, t: y, r: gx + w, b: y + h });
        best = pick(g);
        // 두 꼴의 선이 모두 막히면 6px 격자에서 꺾이는 수가 적은 빈 길을 찾아 말풍선 열까지 간다.
        if (!best) {
          const s = 6, W = Math.ceil(F.width / s), H = Math.ceil(F.height / s), bl = new Uint8Array(W * H);
          const put = (o, v) => { for (let y = Math.max(0, Math.floor(o.t / s)); y <= Math.min(H - 1, Math.floor(o.b / s)); y++) for (let x = Math.max(0, Math.floor(o.l / s)); x <= Math.min(W - 1, Math.floor(o.r / s)); x++) bl[y * W + x] |= v; };
          [...lineObs, ...placed].forEach(o => put(grow(o, 1), 1)); put(k, 2);
          const D = [[1, 0], [-1, 0], [0, 1], [0, -1]], cost = new Float64Array(W * H * 4).fill(Infinity), back = new Int32Array(W * H * 4).fill(-1), q = [];
          const push = (st, cv) => { if (cv >= cost[st]) return; cost[st] = cv; (q[cv] = q[cv] || []).push(st); };
          for (let y = 0; y < H; y++) for (let x = 0; x < W; x++) { if (bl[y * W + x]) continue;
            D.forEach(([dx, dy], d) => { const px = x - dx, py = y - dy; const cx0 = x * s + s / 2, cy0 = y * s + s / 2; if (px >= 0 && py >= 0 && px < W && py < H && bl[py * W + px] & 2 && (dx ? k.t <= cy0 && cy0 <= k.b : k.l <= cx0 && cx0 <= k.r)) push((y * W + x) * 4 + d, 0); }); }
          const cg = Math.floor((gx - 1) / s), okRow = {};
          const goal = y => okRow[y] ?? (okRow[y] = free({ l: gx, t: y * s + 3 - 12, r: gx + w, b: y * s + 3 - 12 + h }));
          let end = -1;
          for (let cv = 0; cv < q.length && end < 0; cv++) for (const st of q[cv] || []) {
            if (cost[st] !== cv) continue; const cell = st >> 2, d = st & 3, x = cell % W, y = (cell - x) / W;
            if (x === cg && goal(y)) { end = st; break; }
            D.forEach(([dx, dy], nd) => { const nx = x + dx, ny = y + dy; if (nx < 0 || ny < 0 || nx >= W || ny >= H || bl[ny * W + nx]) return;
              const ns = (ny * W + nx) * 4 + nd; if (cv + 1 + (nd !== d ? 4 : 0) < cost[ns]) { back[ns] = st; push(ns, cv + 1 + (nd !== d ? 4 : 0)); } });
          }
          if (end >= 0) {
            const cells = []; for (let st = end; st >= 0; st = back[st]) cells.unshift(st);
            const ctr = st => { const cell = st >> 2, x = cell % W; return [x * s + s / 2, (cell - x) / W * s + s / 2]; };
            const [sx, sy] = ctr(cells[0]), d0 = cells[0] & 3;
            const p = [d0 === 0 ? [k.r, sy] : d0 === 1 ? [k.l, sy] : d0 === 2 ? [sx, k.b] : [sx, k.t], [sx, sy]];
            for (let z = 1; z < cells.length; z++) { if ((cells[z] & 3) !== (cells[z - 1] & 3)) p.push(ctr(cells[z - 1])); }
            const [ex, ey] = ctr(cells[cells.length - 1]); p.push([ex, ey], [gx, ey]);
            best = { c: { l: gx, t: ey - 12, r: gx + w, b: ey - 12 + h }, p: p.filter((v, z) => !z || v[0] !== p[z - 1][0] || v[1] !== p[z - 1][1]) };
          }
        }
        // 막히지 않은 길이 없으면 가장 짧은 곧은 선을 쓴다(점검이 이 경우를 잡는다).
        // 말풍선 열에 자리가 하나도 없으면 말풍선을 만들지 않는다. 점검이 빠진 번호로 실패시킨다.
        if (!best && !g.length) { fails++; co.remove(); console.error(`callouts: no place for bubble ${n}`); return; }
        if (!best) { fails++; const c = g.find(free) || g[0]; const x1 = Math.min(Math.max(c.l, k.l), k.r), y1 = Math.min(Math.max(c.t + 12, k.t), k.b); best = { c, p: [[x1, y1], [c.l, c.t + 12]] }; }
      }
      const { c: bc, p: path } = best;
      co.style.left = bc.l + 'px'; co.style.top = bc.t + 'px'; placed.push(bc);
      const ln = document.createElementNS('http://www.w3.org/2000/svg', 'polyline');
      ln.setAttribute('points', path.map(q => q.join(',')).join(' ')); ln.dataset.n = n; svg.appendChild(ln);
      const dot = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
      dot.setAttribute('cx', path[0][0]); dot.setAttribute('cy', path[0][1]); dot.setAttribute('r', 2.5); svg.appendChild(dot);
      for (let z = 1; z < path.length; z++) segs.push([...path[z - 1], ...path[z]]);
    });
    return fails; };
    const idx = marks.map((_, i) => i);
    const orders = [idx, [...idx].sort((a, b) => outl[b].r - outl[a].r), [...idx].sort((a, b) => outl[a].t - outl[b].t), [...idx].reverse()];
    let bestO = null, bestF = Infinity;
    for (const o of orders) { const f = attempt(o); if (f < bestF) { bestF = f; bestO = o; } if (!f) break; }
    if (bestF) attempt(bestO);
    return bestF;
  };
  // 그림이 다 읽힌 뒤에 놓고, 창 폭이 바뀌면 다시 놓는다.
  let tm; addEventListener('resize', () => { clearTimeout(tm); tm = setTimeout(layout, 120); });
  if (document.readyState === 'complete') layout(); else addEventListener('load', layout);
})();
