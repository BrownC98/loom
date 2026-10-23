// 목업 말풍선 배치. 목업 <body> 끝 <script>에 복사한다. 마크업은 user-doc SKILL.md 「목업 말풍선」에 있다.
// 말풍선은 화면 글자·컨트롤과 겹치지 않는 빈 자리에만 둔다. 상자 가까이에 빈 자리가 없으면 그림 오른쪽 여백에 두고 지시선을 긋는다.
(() => {
  const hit = (a, b) => a.l < b.r && b.l < a.r && a.t < b.b && b.t < a.b;
  // 제품마다 컨트롤 클래스가 다르다. 목업은 <body data-ui=".abtn, .seg">로 더한다. scripts/check-callouts.cjs도 같은 값을 읽는다.
  const UI = ['img, svg, button, input, select, textarea', document.body.dataset.ui].filter(Boolean).join(', ');
  const uiRects = (fig, rel) => {
    const out = [];
    const root = [...fig.children].filter(e => !e.matches('.co, .lead-svg, .calls'));
    for (const r of root) {
      const w = document.createTreeWalker(r, NodeFilter.SHOW_TEXT);
      for (let n; (n = w.nextNode());) {
        if (!n.textContent.trim() || n.parentElement.closest('.co, .calls, .lead-svg')) continue;
        const g = document.createRange(); g.selectNodeContents(n);
        for (const q of g.getClientRects()) if (q.width && q.height) out.push(rel(q));
      }
      for (const e of r.querySelectorAll(UI)) if (!e.closest('.co, .lead-svg, .calls')) out.push(rel(e.getBoundingClientRect()));
    }
    return out;
  };
  for (const fig of document.querySelectorAll('.fig')) {
    const src = fig.querySelector(':scope > .calls'); if (!src) continue;
    const texts = [...src.querySelectorAll(':scope > div > span')].map(s => s.innerHTML);
    src.style.display = 'none';
    const F = fig.getBoundingClientRect();
    const rel = r => ({ l: r.left - F.left, t: r.top - F.top, r: r.right - F.left, b: r.bottom - F.top });
    const marks = [...fig.querySelectorAll('.mk[data-n]')];
    const boxes = marks.map(m => { const r = rel(m.getBoundingClientRect()); return { l: r.l - 16, t: r.t - 16, r: r.r + 5, b: r.b + 5 }; });
    const ui = uiRects(fig, rel).concat(boxes);
    const frames = [...fig.querySelectorAll('.page, .xlwin, .shotwrap')].map(e => rel(e.getBoundingClientRect()));
    const gutterX = Math.max(...frames.map(f => f.r)) + 16;
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); svg.setAttribute('class', 'lead-svg');
    svg.setAttribute('width', F.width); svg.setAttribute('height', F.height); fig.appendChild(svg);
    const placed = []; const seen = new Set(); const pad = 4;
    marks.forEach((m, i) => {
      const n = +m.dataset.n; if (seen.has(n) || !texts[n - 1]) return; seen.add(n);
      const co = document.createElement('div'); co.className = 'co'; co.innerHTML = `<b>${n}</b><span>${texts[n - 1]}</span>`;
      co.style.left = '0px'; co.style.top = '0px'; fig.appendChild(co);
      const own = m.closest('.page, .xlwin, .shotwrap'), other = [...fig.querySelectorAll('.page, .xlwin, .shotwrap')].filter(e => e !== own).map(e => rel(e.getBoundingClientRect()));
      const k = boxes[i], free = c => c.l >= 0 && c.r <= F.width && c.t >= 40 && ![...ui, ...placed, ...other].some(o => hit({ l: c.l - pad, t: c.t - pad, r: c.r + pad, b: c.b + pad }, o));
      let w = co.offsetWidth, h = co.offsetHeight, best = null;
      // 상자에서 가까운 자리부터 찾는다(오른쪽, 아래, 왼쪽, 위, 120px 안).
      outer: for (let d = 6; d <= 120; d += 6) {
        for (const [x, y] of [[k.r + d, k.t], [k.l, k.b + d], [k.l - w - d, k.t], [k.l, k.t - h - d], [k.r + d, k.b - h], [k.r - w, k.b + d], [k.r - w, k.t - h - d], [k.l - w - d, k.b - h]]) {
          const c = { l: x, t: y, r: x + w, b: y + h };
          if (free(c)) { best = c; break outer; }
        }
      }
      let lead = false;
      if (!best) {
        const gx = own ? Math.min(rel(own.getBoundingClientRect()).r + 16, gutterX) : gutterX;
        co.style.maxWidth = Math.min(250, F.width - gx) + 'px'; w = co.offsetWidth; h = co.offsetHeight;
        for (let y = Math.max(40, (k.t + k.b) / 2 - h / 2); y + h <= F.height; y += 4) { const c = { l: gx, t: y, r: gx + w, b: y + h }; if (free(c)) { best = c; break; } }
        // 자리가 없으면 말풍선을 만들지 않는다. check-callouts.cjs가 빠진 번호로 실패시킨다.
        if (!best) { co.remove(); console.error(`callouts: no free place for bubble ${n}`); return; }
        lead = true;
      }
      co.style.left = best.l + 'px'; co.style.top = best.t + 'px'; placed.push(best);
      if (lead) {
        const p = document.createElementNS('http://www.w3.org/2000/svg', 'polyline');
        // 지시선은 그림 틀 오른쪽 끝에서 시작해 여백 안에서만 긋는다. 화면 글자를 지나지 않는다.
        const fr = rel(m.closest('.page, .xlwin, .shotwrap').getBoundingClientRect());
        const y1 = Math.min(Math.max((k.t + 16 + k.b - 5) / 2, fr.t + 6), fr.b - 6), y2 = best.t + 12;
        p.setAttribute('points', `${fr.r - 6},${y1} ${fr.r + 4},${y1} ${best.l - 4},${y2} ${best.l},${y2}`);
        const dot = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
        dot.setAttribute('cx', fr.r - 6); dot.setAttribute('cy', y1); dot.setAttribute('r', 3); svg.appendChild(dot);
        svg.appendChild(p);
      }
    });
  }
})();
