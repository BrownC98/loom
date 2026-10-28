(() => {
  const GUT = 290;
  const fit = fig => {
    const room = fig.clientWidth - GUT;
    for (const e of fig.querySelectorAll('.page, .shotwrap')) e.style.zoom = Math.min(0.8, room / (e.matches('.shotwrap') ? 1394 : 1360));
    for (const e of fig.querySelectorAll('.xlwin')) { e.style.zoom = 1; e.style.zoom = Math.min(1, room / e.offsetWidth); }
  };
  const hit = (a, b) => a.l < b.r && b.l < a.r && a.t < b.b && b.t < a.b;
  const inside = (a, b) => a.l >= b.l && a.r <= b.r && a.t >= b.t && a.b <= b.b;
  const grow = (r, d) => ({ l: r.l - d, t: r.t - d, r: r.r + d, b: r.b + d });
  const cross = (x1, y1, x2, y2, r) => {
    let t0 = 0, t1 = 1; const dx = x2 - x1, dy = y2 - y1;
    for (const [p, q] of [[-dx, x1 - r.l], [dx, r.r - x1], [-dy, y1 - r.t], [dy, r.b - y1]]) {
      if (p === 0) { if (q < 0) return false; continue; }
      const t = q / p; if (p < 0) { if (t > t1) return false; if (t > t0) t0 = t; } else { if (t < t0) return false; if (t < t1) t1 = t; }
    }
    return true;
  };
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
  const refs = new Map();
  const name = fig => [...fig.querySelector('h2')?.childNodes ?? []].filter(c => !c.classList?.contains('tag')).map(c => c.textContent).join('').trim();
  for (const d of document.querySelectorAll('.decision[data-for]')) {
    const groups = new Map();
    for (const n of d.dataset.for.match(/\d+/g) ?? []) {
      refs.set(n, [...(refs.get(n) ?? []), d.id]);
      const fig = document.querySelector(`.mk[data-n="${n}"]`)?.closest('.fig'); if (!fig) continue;
      groups.set(fig, [...(groups.get(fig) ?? []), n]);
    }
    const line = document.createElement('p'); line.className = 'for';
    line.textContent = '대상: ' + [...groups].map(([fig, ns]) => `${name(fig)} · ${ns.join(', ')}번`).join(' / ');
    d.querySelector(':scope > .for')?.remove(); d.querySelector(':scope > h3')?.after(line);
  }
  const layout = () => { for (const fig of document.querySelectorAll('.fig')) {
    const src = fig.querySelector(':scope > .calls'); if (!src) { fit(fig); continue; }
    fig.style.paddingBottom = ''; const base = parseFloat(getComputedStyle(fig).paddingBottom);
    for (let x = 20; place(fig, src) && x <= 1200; x += 20) fig.style.paddingBottom = base + x + 'px';
  } };
  const place = (fig, src) => {
    fig.querySelectorAll(':scope > .co, :scope > .lead-svg, :scope > .ov-box, :scope > .ov-badge').forEach(e => e.remove());
    fit(fig);
    const texts = new Map([...src.querySelectorAll(':scope > div')].map(d => [d.querySelector(':scope > b')?.textContent.trim(), d.querySelector(':scope > span')?.innerHTML]));
    src.style.display = 'none';
    const F = fig.getBoundingClientRect();
    const rel = r => ({ l: r.left - F.left, t: r.top - F.top, r: r.right - F.left, b: r.bottom - F.top });
    const marks = [...fig.querySelectorAll('.mk[data-n]')];
    const clip = (g, e) => { if (!e || !fig.contains(e)) return g; const z = grow(rel(e.getBoundingClientRect()), -1); return { l: Math.max(g.l, z.l), t: Math.max(g.t, z.t), r: Math.min(g.r, z.r), b: Math.min(g.b, z.b) }; };
    const outl = marks.map(m => { const r = rel(m.getBoundingClientRect()), frame = m.closest('.page, .xlwin, .shotwrap');
      if (m.matches('td, th')) return clip(grow(r, -1), frame);
      if (m.matches('.shotwrap .mk')) return r;
      return clip(clip(grow(r, 5), m.parentElement.closest('td, th')), frame); });
    const ui = uiRects(fig, rel);
    const badges = [];
    marks.forEach((m, i) => {
      const o = outl[i], bx = document.createElement('div'); bx.className = 'ov-box' + (m.classList.contains('new') ? ' new' : ''); bx.dataset.n = m.dataset.n;
      Object.assign(bx.style, { left: o.l + 'px', top: o.t + 'px', width: o.r - o.l + 'px', height: o.b - o.t + 'px' }); fig.appendChild(bx);
      const spots = [[o.l, o.t], [o.r, o.t], [o.l, o.b], [o.r, o.b], [(o.l + o.r) / 2, o.t - 14], [(o.l + o.r) / 2, o.b + 14], [o.l - 14, o.t + 14], [o.r + 14, o.t + 14]].map(([x, y]) => ({ l: x - 14, t: y - 14, r: x + 14, b: y + 14 }));
      const ocx = (o.l + o.r) / 2, ocy = (o.t + o.b) / 2, cont = u => !u.txt && u.l <= ocx && ocx <= u.r && u.t <= ocy && ocy <= u.b;
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
    const attempt = order => {
    fig.querySelectorAll(':scope > .co').forEach(e => e.remove()); svg.replaceChildren();
    const placed = [], segs = [], seen = new Set(), pad = 4; let fails = 0, longest = 0;
    order.forEach(i => { const m = marks[i];
      const n = +m.dataset.n; if (seen.has(n) || !texts.get(String(n))) return; seen.add(n);
      const co = document.createElement('div'); co.className = 'co'; co.dataset.n = n; co.innerHTML = `<b>${n}</b>${(refs.get(String(n)) ?? []).map(k => `<i class="dref">${k}</i>`).join('')}<span>${texts.get(String(n))}</span>`;
      Object.assign(co.style, { left: '0px', top: '0px', boxSizing: 'border-box' }); fig.appendChild(co);
      const own = m.closest('.page, .xlwin, .shotwrap');
      const other = frames.filter(e => e !== own).map(e => rel(e.getBoundingClientRect()));
      const k = outl[i];
      const cx = (k.l + k.r) / 2, cy = (k.t + k.b) / 2, holds = o => o.l <= cx && cx <= o.r && o.t <= cy && cy <= o.b;
      const lineObs = ui.filter(o => !inside(o, k) && !holds(o)).concat(outl.filter((o, j) => j !== i && !holds(o)), badges.filter(b => !hit(b, grow(k, 1))), other);
      const bubObs = ui.concat(boxes, badges);
      const segFree = (x1, y1, x2, y2) => { const d = Math.hypot(x2 - x1, y2 - y1); if (d < 3) return true;
        const ux = (x2 - x1) / d, uy = (y2 - y1) / d; return ![...lineObs, ...placed].some(o => cross(x1 + ux * 2, y1 + uy * 2, x2 - ux * 2, y2 - uy * 2, o)); };
      const plen = p => p.reduce((s, q, z) => z ? s + Math.hypot(q[0] - p[z - 1][0], q[1] - p[z - 1][1]) : 0, 0);
      const route = c => {
        const x1 = Math.min(Math.max((c.l + c.r) / 2, k.l), k.r), y1 = Math.min(Math.max((c.t + c.b) / 2, k.t), k.b);
        const st = [[x1, y1], [Math.min(Math.max(x1, c.l), c.r), Math.min(Math.max(y1, c.t), c.b)]];
        if (segFree(...st[0], ...st[1])) return st;
        let line = null;
        for (const [a, b] of [[k.l, k.t, k.r, k.t], [k.l, k.b, k.r, k.b], [k.l, k.t, k.l, k.b], [k.r, k.t, k.r, k.b]].flatMap(([xa, ya, xb, yb]) => { const L = Math.hypot(xb - xa, yb - ya), m = Math.max(1, Math.round(L / 8)); return Array.from({ length: m + 1 }, (_, z) => [xa + (xb - xa) * z / m, ya + (yb - ya) * z / m]); })) {
          const p = [[a, b], [Math.min(Math.max(a, c.l), c.r), Math.min(Math.max(b, c.t), c.b)]];
          if ((!line || plen(p) < plen(line)) && segFree(...p[0], ...p[1])) line = p;
        }
        if (line) return line;
        const yc = c.t + 16, side = c.l >= k.r ? c.l : c.r <= k.l ? c.r : null; if (side === null) return null;
        if (yc >= k.t && yc <= k.b) return null;
        const ye = yc < k.t ? k.t : k.b; let best = null;
        for (let xv = k.l + 6; xv <= k.r - 6; xv += 10) {
          const p = [[xv, ye], [xv, yc], [side, yc]];
          if (segFree(...p[0], ...p[1]) && segFree(...p[1], ...p[2]) && (!best || plen(p) < plen(best))) best = p;
        }
        return best;
      };
      const free = c => c.l >= 0 && c.r <= F.width && c.t >= 40 && c.b <= F.height - 4 && ![...bubObs, ...placed].some(o => hit(grow(c, pad), o)) && !segs.some(q => cross(...q, grow(c, 2)));
      const w0 = co.offsetWidth, sizes = [...new Set([w0, Math.round(w0 * 0.75), Math.round(w0 * 0.55)])].map(w => { co.style.width = w + 'px'; return [w, co.offsetHeight]; });
      const cands = [];
      for (const [w, h] of sizes) {
        for (let d = 6; d <= 120; d += 6)
          for (const [x, y] of [[k.r + d, k.t], [k.l, k.b + d], [k.l - w - d, k.t], [k.l, k.t - h - d], [k.r + d, k.b - h], [k.r - w, k.b + d], [k.r - w, k.t - h - d], [k.l - w - d, k.b - h]])
            cands.push({ l: x, t: y, r: x + w, b: y + h });
        for (let y = 40; y + h < F.height; y += 16) for (let x = 0; x + w < F.width; x += 16) cands.push({ l: x, t: y, r: x + w, b: y + h });
      }
      const gap = c => Math.hypot(Math.max(0, c.l - k.r, k.l - c.r), Math.max(0, c.t - k.b, k.t - c.b));
      const pick = list => { let bp = null, bc = null; for (const c of list.map(c => [gap(c), c]).sort((a, b) => a[0] - b[0]).map(([, c]) => c)) { if (bp && gap(c) >= plen(bp)) break; if (!free(c)) continue; const p = route(c); if (p && (!bp || plen(p) < plen(bp))) { bp = p; bc = c; } } return bc && { c: bc, p: bp }; };
      const best = pick(cands);
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
  document.fonts?.addEventListener('loadingdone', () => dispatchEvent(new Event('resize')));
  let tm; addEventListener('resize', () => { clearTimeout(tm); tm = setTimeout(layout, 120); });
  if (document.readyState === 'complete') layout(); else addEventListener('load', layout);
})();
