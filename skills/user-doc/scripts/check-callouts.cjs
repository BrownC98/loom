const path = require('path'), fs = require('fs'), os = require('os'), { pathToFileURL } = require('url');
let pw; try { pw = require('playwright'); } catch { const c = path.join(os.homedir(), '.npm/_npx'); for (const n of fs.readdirSync(c)) { const m = path.join(c, n, 'node_modules/playwright'); if (fs.existsSync(m)) { pw = require(m); break; } } }

const RUNS = [{ w: 1920 }, { w: 1440 }, { w: 1280 }, { w: 1100 }, { w: 960 }, { w: 1440, slow: 1500 }, { w: 1440, resizeTo: 1000 }];
const HEIGHT = 900, SETTLE_MS = 600;
const LEADER_MAX = 120;

const doc = path.resolve(process.argv[2]);
const html = fs.readFileSync(doc, 'utf8');

async function openSlow(page, delay) {
  const imgs = [];
  const body = html.replace(/(<img\b[^>]*?\bsrc\s*=\s*)(?:"(data:[^"]*)"|'(data:[^']*)'|(data:[^\s>]*))/gi, (_, a, d1, d2, d3) => `${a}"https://mockup.invalid/img/${imgs.push(d1 ?? d2 ?? d3) - 1}"`);
  await page.route('https://mockup.invalid/**', async r => {
    const u = r.request().url();
    if (u.endsWith('/doc.html')) return r.fulfill({ contentType: 'text/html; charset=utf-8', body });
    const m = /^data:([^;,]*)(;base64)?,(.*)$/s.exec(imgs[+u.split('/').pop()] ?? '');
    if (!m) return r.abort();
    await new Promise(z => setTimeout(z, delay));
    r.fulfill({ contentType: m[1], body: m[2] ? Buffer.from(m[3], 'base64') : decodeURIComponent(m[3]) });
  });
  await page.goto('https://mockup.invalid/doc.html', { waitUntil: 'load' });
}

const LOCAL_REFS = () => {
  const ok = v => /^(data:|https:|#|$)/i.test(v.trim());
  const bad = [];
  for (const e of document.querySelectorAll('[src], [href], [srcset], [poster]'))
    for (const a of ['src', 'href', 'srcset', 'poster']) {
      const v = e.getAttribute(a); if (v === null) continue;
      const refs = a === 'srcset' ? v.split(/(?<=\s\d+(?:\.\d+)?[wx])\s*,|,\s+/).map(c => c.trim().split(/\s+/)[0]) : [v];
      for (const r of refs) if (!ok(r)) bad.push(`not one file: <${e.tagName.toLowerCase()} ${a}="${r.slice(0, 60)}">`);
    }
  const css = [...document.querySelectorAll('style')].map(s => s.textContent).concat([...document.querySelectorAll('[style]')].map(e => e.getAttribute('style')));
  for (const t of css) {
    for (const m of t.matchAll(/url\(\s*['"]?([^'")]*)/gi)) if (!ok(m[1])) bad.push(`not one file: url(${m[1].slice(0, 60)})`);
    for (const m of t.matchAll(/@import\s+(?:url\(\s*)?['"]?([^'")\s;]+)/gi)) if (!ok(m[1])) bad.push(`not one file: @import ${m[1].slice(0, 60)}`);
  }
  return bad;
};

const IDS = mockup => {
  const bad = [], ids = new Map(), cut = e => e.textContent.trim().replace(/\s+/g, ' ').slice(0, 30);
  const use = (k, what) => { if (ids.has(k)) bad.push(`ID ${k} is used twice (${ids.get(k)} and ${what})`); else ids.set(k, what); };
  const SCREEN = '.page, .xlwin, .shotwrap';
  for (const d of document.querySelectorAll('.decision')) {
    const k = d.id, chip = d.querySelector('.did')?.textContent.trim();
    if (!/^D\d+$/.test(k) || chip !== k) bad.push(`decision "${cut(d)}" has no ID: give it id="D<n>" and the same text in .did`);
    else use(k, 'decision');
    const opts = d.querySelectorAll('.opt').length, rec = d.querySelectorAll('.opt[data-recommended]');
    if (opts < 2 || opts > 4) bad.push(`decision ${k} has ${opts} options (needs 2 to 4 .opt)`);
    if (rec.length !== 1 || !rec[0].querySelector('.mark')?.textContent.trim()) bad.push(`decision ${k} needs one recommended option (.opt[data-recommended] with text in .mark)`);
    if (document.querySelector('.calls')) {
      const ns = d.dataset.for?.match(/\d+/g) ?? [];
      if (!ns.length) bad.push(`decision ${k} has no target: give it data-for="<box numbers>"`);
      for (const n of ns) if (![...document.querySelectorAll(`.fig .mk[data-n="${n}"]`)].some(m => [...m.closest('.fig').querySelectorAll(':scope > .calls > div > b')].some(b => b.textContent.trim() === n))) bad.push(`decision ${k} target ${n} is not a box with a callout in a screen section`);
    }
  }
  for (const e of document.querySelectorAll('.opt, .did, [data-recommended]')) if (!e.closest('.decision, ' + SCREEN)) bad.push(`option outside a decision box: "${cut(e)}"`);
  for (const c of document.querySelectorAll('.calls > div')) {
    const k = c.querySelector(':scope > b')?.textContent.trim() ?? '';
    if (!/^\d+$/.test(k)) bad.push(`callout "${cut(c)}" has no number (<b>n</b>)`); else use(k, 'callout "' + cut(c.querySelector(':scope > span') ?? c) + '"');
  }
  for (const m of document.querySelectorAll('.mk:not([data-n])')) bad.push(`box "${cut(m)}" has no number (data-n)`);
  if (mockup) {
    const w = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let t; (t = w.nextNode());) if (/권장|정할 것|Recommended/i.test(t.textContent) && !t.parentElement.closest('.decision, .lead, .who, script, style, ' + SCREEN)) bad.push(`decision outside a decision box: "${cut(t)}"`);
  }
  return [...new Set(bad)];
};

const DOM_PASS = LEADER_MAX => {
  const R = r => ({ l: r.left, t: r.top, r: r.right, b: r.bottom });
  const hit = (a, c) => a.l < c.r && c.l < a.r && a.t < c.b && c.t < a.b;
  const inside = (a, c) => a.l >= c.l - 0.5 && a.t >= c.t - 0.5 && a.r <= c.r + 0.5 && a.b <= c.b + 0.5;
  const clips = e => { const out = []; for (let a = e; a && a !== document.documentElement; a = a.parentElement) { const cs = getComputedStyle(a); if (cs.overflowX !== 'visible' || cs.overflowY !== 'visible') out.push(a); } return out; };
  const clipBox = a => { const k = a.getBoundingClientRect(), s = a.offsetWidth ? k.width / a.offsetWidth : 1, l = k.left + a.clientLeft * s, t = k.top + a.clientTop * s; return { l, t, r: l + a.clientWidth * s, b: t + a.clientHeight * s }; };
  const visible = (e, r) => { for (const a of clips(e)) { const c = clipBox(a); r = { l: Math.max(r.l, c.l), t: Math.max(r.t, c.t), r: Math.min(r.r, c.r), b: Math.min(r.b, c.b) }; } return r.l < r.r && r.t < r.b ? r : null; };
  const OVER = '.co, .calls, .lead-svg, .ov-box, .ov-badge, script, style';
  const UI = ['img, svg:not(.lead-svg), button, input, select, textarea', document.body.dataset.ui].filter(Boolean).join(', ');
  const textRuns = root => {
    const out = []; const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let t; (t = w.nextNode());) {
      const e = t.parentElement; if (!t.textContent.trim() || e.closest(OVER) || getComputedStyle(e).visibility === 'hidden') continue;
      const g = document.createRange(); g.selectNodeContents(t);
      for (const q of g.getClientRects()) { const r = q.width && q.height && visible(e, R(q)); if (r) out.push({ r, what: t.textContent.trim().slice(0, 20), e }); }
    }
    return out;
  };
  const cross = (x1, y1, x2, y2, r) => {
    const d = Math.hypot(x2 - x1, y2 - y1); if (d < 5) return false;
    const ux = (x2 - x1) / d * 2, uy = (y2 - y1) / d * 2; x1 += ux; y1 += uy; x2 -= ux; y2 -= uy;
    let t0 = 0, t1 = 1; const dx = x2 - x1, dy = y2 - y1;
    for (const [p, q] of [[-dx, x1 - r.l], [dx, r.r - x1], [-dy, y1 - r.t], [dy, r.b - y1]]) {
      if (p === 0) { if (q < 0) return false; continue; }
      const t = q / p; if (p < 0) { if (t > t1) return false; if (t > t0) t0 = t; } else { if (t < t0) return false; if (t < t1) t1 = t; }
    }
    return true;
  };
  const allPins = [...document.querySelectorAll('.fig > .ov-badge')], allBoxes = [...document.querySelectorAll('.fig > .ov-box')];
  const bad = [], badges = [], bands = []; let n = 0;
  for (const fig of document.querySelectorAll('.fig')) {
    const title = fig.querySelector('h2')?.textContent.slice(0, 12) ?? '';
    const calls = fig.querySelector(':scope > .calls');
    const marks = [...fig.querySelectorAll('.mk[data-n]')];
    if (!calls && !marks.length) continue;
    const by = sel => { const g = new Map(); for (const e of fig.querySelectorAll(sel)) g.set(e.dataset.n, [...(g.get(e.dataset.n) || []), e]); return g; };
    const flat = g => [...g].flatMap(([k, l]) => l.map(e => [k, e]));
    const cos = by(':scope > .co'), boxes = by(':scope > .ov-box'), pins = by(':scope > .ov-badge'), lines = by(':scope > .lead-svg polyline');
    n += cos.size;
    if (calls && getComputedStyle(calls).display !== 'none') bad.push(title + ' placement script did not run (.calls still shown)');
    const want = new Set((calls ? [...calls.querySelectorAll(':scope > div > b')].map(b => b.textContent.trim()) : []).concat(marks.map(m => m.dataset.n)));
    for (const k of want) {
      if (!cos.has(k)) bad.push(title + ' #' + k + ' has no bubble (box without text, text without box, or no free place)');
      if (!boxes.has(k)) bad.push(title + ' #' + k + ' has no box overlay (.ov-box)');
      if (!pins.has(k)) bad.push(title + ' #' + k + ' has no badge (.ov-badge)');
    }
    const texts = textRuns(fig);
    const ui = texts.concat([...fig.querySelectorAll(UI)].filter(e => !e.closest(OVER)).map(e => ({ r: R(e.getBoundingClientRect()), what: e.tagName.toLowerCase() + (e.className.baseVal ?? e.className ? '.' + (e.className.baseVal ?? e.className) : '') })));
    const rect = e => R(e.getBoundingClientRect());
    const fr = rect(fig);
    for (const f of fig.querySelectorAll('.page, .xlwin, .shotwrap')) if (rect(f).r > fr.r + 0.5) bad.push(title + ' screen frame is wider than its section');
    for (const [k, e] of [...flat(cos), ...flat(boxes), ...flat(pins)]) if (!inside(rect(e), fr)) bad.push(title + ' #' + k + ' ' + e.className.split(' ')[0] + ' outside its section');
    const cl = flat(cos);
    cl.forEach(([k, c], i) => {
      const cr = rect(c);
      for (const u of ui) if (hit(cr, u.r)) bad.push(title + ' bubble ' + k + ' x ' + u.what);
      for (const [j, e] of [...flat(boxes), ...flat(pins)]) if (hit(cr, rect(e))) bad.push(title + ' bubble ' + k + ' x box/badge ' + j);
      cl.forEach(([j, d], z) => { if (z > i && hit(cr, rect(d))) bad.push(title + ' bubble ' + k + ' x bubble ' + j); });
    });
    const edge = ([x, y], r) => { const dx = Math.max(r.l - x, 0, x - r.r), dy = Math.max(r.t - y, 0, y - r.b); return dx || dy ? Math.hypot(dx, dy) : Math.min(x - r.l, r.r - x, y - r.t, r.b - y); };
    const ends = l => { const s = l.getScreenCTM(); return [...l.points].map(p => { const q = p.matrixTransform(s); return [q.x, q.y]; }); };
    for (const [k, c] of cl) {
      const ls = lines.get(k) || [];
      if (ls.length !== 1) { bad.push(title + ' bubble ' + k + ' has ' + ls.length + ' leaders (needs 1)'); continue; }
      const pts = ends(ls[0]);
      if (pts.length < 2) { bad.push(title + ' leader ' + k + ' has fewer than 2 points'); continue; }
      if (!(boxes.get(k) || []).some(b => edge(pts[0], rect(b)) <= 3)) bad.push(title + ' leader ' + k + ' does not start on its box');
      if (edge(pts[pts.length - 1], rect(c)) > 3) bad.push(title + ' leader ' + k + ' does not end on its bubble');
      const len = pts.reduce((s, q, i) => (i ? s + Math.hypot(q[0] - pts[i - 1][0], q[1] - pts[i - 1][1]) : 0), 0);
      if (len > LEADER_MAX) bad.push(title + ' leader ' + k + ' is ' + Math.round(len) + ' px long (max ' + LEADER_MAX + ')');
    }
    for (const [k, l] of flat(lines)) {
      const s = l.getScreenCTM(), pts = [...l.points].map(p => { const q = p.matrixTransform(s); return [q.x, q.y]; });
      const own = boxes.get(k)?.[0], c = own && rect(own), mid = c && [(c.l + c.r) / 2, (c.t + c.b) / 2];
      const holds = e => { const r = rect(e); return mid && r.l <= mid[0] && mid[0] <= r.r && r.t <= mid[1] && mid[1] <= r.b; };
      const others = [...flat(boxes).filter(([j, e]) => !holds(e)), ...cl].filter(([j]) => j !== k);
      for (let i = 1; i < pts.length; i++) for (const [j, e] of others) if (cross(...pts[i - 1], ...pts[i], rect(e))) bad.push(title + ' leader ' + k + ' crosses ' + (e.classList.contains('co') ? 'bubble ' : 'box ') + j);
    }
    const ori = (a, b, c) => Math.sign((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]));
    const segs = flat(lines).map(([k, l]) => { const p = ends(l); return [k, p.slice(1).map((q, i) => [p[i], q])]; });
    segs.forEach(([k, a], i) => segs.slice(i + 1).forEach(([j, b]) => {
      if (a.some(([p, q]) => b.some(([r, s]) => ori(p, q, r) * ori(p, q, s) < 0 && ori(r, s, p) * ori(r, s, q) < 0))) bad.push(title + ' leader ' + k + ' crosses leader ' + j);
    }));
    const seen = new Map();
    for (const m of marks) {
      const k = m.dataset.n, id = title + ' #' + k, z = seen.get(k) || 0; seen.set(k, z + 1);
      const box = boxes.get(k)?.[z], pin = pins.get(k)?.[z];
      if (box) {
        const o = rect(box);
        for (const a of clips(m)) if (!inside(o, clipBox(a))) bad.push(id + ' box clipped by ' + (a.className || a.tagName));
        const fr = m.closest('.page, .xlwin, .shotwrap'); if (fr && !inside(o, rect(fr))) bad.push(id + ' box outside its frame');
        const cell = m.parentElement?.closest('td, th'); if (cell && fig.contains(cell) && !inside(o, rect(cell))) bad.push(id + ' box crosses its table cell');
        const rg = document.createRange(); rg.selectNodeContents(m); const u = R(rg.getBoundingClientRect());
        if (m.children.length) {
          const g = [u.l - o.l, o.r - u.r, u.t - o.t, o.b - u.b].map(Math.round);
          if (!inside(u, o) || Math.abs(g[0] - g[1]) > 4 || Math.abs(g[2] - g[3]) > 4) bad.push(id + ' box misfit (gaps l/r/t/b ' + g.join('/') + ')');
        } else if (!m.textContent.trim() && getComputedStyle(m).position === 'absolute') bands.push({ id, i: allBoxes.indexOf(box) });
      }
      if (box) {
        const t = rect(m), o = rect(box), at = (r, c, d) => Math.max(Math.abs(r.l - c.l), Math.abs(r.t - c.t), Math.abs(r.r - c.r), Math.abs(r.b - c.b)) <= d;
        const frm = m.closest('.page, .xlwin, .shotwrap'), cut = (g, e) => { if (!e || !fig.contains(e)) return g; const z = rect(e); return { l: Math.max(g.l, z.l + 1), t: Math.max(g.t, z.t + 1), r: Math.min(g.r, z.r - 1), b: Math.min(g.b, z.b - 1) }; };
        const grow = d => ({ l: t.l - d, t: t.t - d, r: t.r + d, b: t.b + d });
        const want = m.matches('td, th') ? cut(grow(-1), frm) : m.matches('.shotwrap .mk') ? t : cut(cut(grow(5), m.parentElement.closest('td, th')), frm);
        if (!at(o, want, 1.5)) bad.push(id + ' box is not at its target (expected ' + [want.l - t.l, want.t - t.t, t.r - want.r, t.b - want.b].map(v => Math.round(-v)).join('/') + ' px around it)');
        const ups = []; for (let a = m.parentElement; a && a !== fig; a = a.parentElement) ups.push(a);
        const line = cs => ['Top', 'Right', 'Bottom', 'Left'].every(z => cs['border' + z + 'Style'] !== 'none' && parseFloat(cs['border' + z + 'Width']) > 0 && !/rgba\(.*,\s*0\)$|transparent/.test(cs['border' + z + 'Color']));
        const outl = cs => cs.outlineStyle !== 'none' && parseFloat(cs.outlineWidth) > 0;
        for (const e of [m, ...m.querySelectorAll('*'), ...ups].filter(e => at(rect(e), t, 9))) {
          const ps = ['::before', '::after'].map(z => getComputedStyle(e, z)).filter(cs => cs.content !== 'none' && cs.content !== 'normal');
          const big = cs => { const sz = k => parseFloat(cs[k]) + parseFloat(cs['border' + (k === 'width' ? 'Left' : 'Top') + 'Width']) + parseFloat(cs['border' + (k === 'width' ? 'Right' : 'Bottom') + 'Width']); return sz('width') >= e.offsetWidth / 2 && sz('height') >= e.offsetHeight / 2; };
          if (ps.some(cs => (line(cs) || outl(cs)) && big(cs))) { bad.push(id + ' box has a second border line (::before/::after on <' + e.tagName.toLowerCase() + '>)'); break; }
          const cs = getComputedStyle(e); if (outl(cs)) { bad.push(id + ' box has a second border line (outline on <' + e.tagName.toLowerCase() + (e.className.baseVal ?? e.className ? '.' + (e.className.baseVal ?? e.className).trim().replace(/\s+/g, '.') : '') + '>)'); break; } }
      }
      if (pin) {
        const g = rect(pin);
        for (const a of clips(pin)) if (!inside(g, clipBox(a))) bad.push(id + ' badge clipped by ' + (a.className || a.tagName));
        const D = document.documentElement;
        if (g.l + scrollX < 0 || g.t + scrollY < 0 || g.r + scrollX > D.scrollWidth || g.b + scrollY > D.scrollHeight) bad.push(id + ' badge outside the page');
        for (const t of texts) if (hit(g, t.r)) bad.push(id + ' badge over text "' + t.what + '"');
        const bc = box && rect(box), mid = bc && [(bc.l + bc.r) / 2, (bc.t + bc.b) / 2];
        for (const u of ui.slice(texts.length)) if (hit(g, u.r) && !(mid && u.r.l <= mid[0] && mid[0] <= u.r.r && u.r.t <= mid[1] && mid[1] <= u.r.b)) bad.push(id + ' badge over control ' + u.what);
        badges.push({ id, i: allPins.indexOf(pin) });
      }
    }
  }
  return { bad, badges, bands, bubbles: n };
};

const LAYOUT_PASS = () => {
  const els = [...document.querySelectorAll('.fig .page *, .fig .xlwin *, .fig .shotwrap *')].filter(e => !e.closest('.co, .lead-svg, .ov-box, .ov-badge'));
  const before = els.map(e => e.getBoundingClientRect());
  document.querySelectorAll('.co, .lead-svg, .ov-box, .ov-badge').forEach(e => e.remove());
  for (const m of document.querySelectorAll('.mk')) { if (m.matches('.shotwrap .mk') && !m.children.length && !m.textContent.trim()) m.style.position = 'absolute'; m.classList.remove('mk', 'new'); }
  const bad = [];
  els.forEach((e, i) => {
    const a = before[i], b = e.getBoundingClientRect();
    if (Math.abs(a.left - b.left) > 0.5 || Math.abs(a.top - b.top) > 0.5 || Math.abs(a.width - b.width) > 0.5 || Math.abs(a.height - b.height) > 0.5) {
      const fig = e.closest('.fig'), m = e.closest('[data-n]');
      bad.push((fig?.querySelector('h2')?.textContent.slice(0, 12) ?? '') + (m ? ' #' + m.dataset.n : '') + ' callouts shift layout of <' + e.tagName.toLowerCase() + '>');
    }
  });
  return [...new Set(bad)].slice(0, 20);
};

const PIXELS = async ({ shots, pins, bands }) => {
  const px = async b64 => { if (!b64) return null; const i = new Image(); i.src = 'data:image/png;base64,' + b64; await i.decode(); const c = document.createElement('canvas'); c.width = i.width; c.height = i.height; const x = c.getContext('2d'); x.drawImage(i, 0, 0); return x.getImageData(0, 0, i.width, i.height); };
  const [a, b, h] = [await px(shots[0]), await px(shots[1]), await px(shots[2])];
  const at = (im, x, y) => { const o = (y * im.width + x) * 4; return [im.data[o], im.data[o + 1], im.data[o + 2]]; };
  const diff = (p, q) => Math.abs(p[0] - q[0]) + Math.abs(p[1] - q[1]) + Math.abs(p[2] - q[2]);
  const covered = pins.map(({ pts }) => pts.filter(([x, y]) => diff(at(a, x, y), at(b, x, y)) < 60).length);
  const cuts = bands.map(({ strips }) => strips.map(st => { const h2 = {}; for (const [x, y] of st) { const k = at(h, x, y).join(); h2[k] = (h2[k] || 0) + 1; } return h2; }));
  return { covered, cuts };
};

async function checkRun(page) {
  await page.addStyleTag({ content: '*, *::before, *::after { transition: none !important; animation: none !important; } .chk-recolor .fig > .ov-badge { background: #00FF00 !important; color: #0000FF !important; } .chk-hide .ov-box, .chk-hide .ov-badge, .chk-hide .co, .chk-hide .lead-svg { visibility: hidden !important; }' });
  const res = await page.evaluate(DOM_PASS, LEADER_MAX);
  const vp = page.viewportSize(), OVERLAP = 40;
  const shot = async () => (await page.screenshot({ animations: 'disabled' })).toString('base64');
  const todo = { pins: [...res.badges], bands: res.bands.map(b => ({ ...b })) }, all = [];
  const docH = await page.evaluate(() => document.documentElement.scrollHeight);
  for (let y0 = 0; todo.pins.length + todo.bands.length && y0 < docH; y0 += vp.height - OVERLAP) {
    const geo = await page.evaluate(({ y0, pins, bands }) => {
      scrollTo(0, y0);
      const r = e => { const q = e.getBoundingClientRect(); return { l: q.left, t: q.top, r: q.right, b: q.bottom }; };
      const P = [...document.querySelectorAll('.fig > .ov-badge')], B = [...document.querySelectorAll('.fig > .ov-box')];
      return { sy: scrollY, pins: pins.map(p => r(P[p.i])), bands: bands.map(p => r(B[p.i])) };
    }, { y0, pins: todo.pins, bands: todo.bands.map(b => ({ i: b.i })) });
    const inView = g => g.t >= 0 && g.b <= vp.height;
    const pins = [], bands = [];
    todo.pins = todo.pins.filter((p, k) => {
      const g = geo.pins[k]; if (!inView(g)) return true;
      const cx = (g.l + g.r) / 2, cy = (g.t + g.b) / 2, d = (g.r - g.l) / 3;
      const pts = [[cx, cy], [cx - d, cy], [cx + d, cy], [cx, cy - d], [cx, cy + d]].map(([x, y]) => [Math.round(x), Math.round(y)]);
      if (pts.some(([x]) => x < 0 || x >= vp.width)) res.bad.push(p.id + ' badge off screen'); else pins.push({ id: p.id, pts });
      return false;
    });
    todo.bands = todo.bands.filter((p, k) => {
      const g = geo.bands[k], sy = geo.sy, L = Math.round(g.l), T = Math.round(g.t), Rr = Math.round(g.r) - 1, Bt = Math.round(g.b) - 1;
      const y0 = Math.max(T, (p.done ?? -1e9) - sy, 0), y1 = Math.min(Bt, vp.height - 1);
      if (y0 > y1) return Bt >= vp.height;
      const strip = (x0, ya, x1, yb) => { const s = []; for (let y = Math.max(y0, ya); y <= Math.min(y1, yb); y++) for (let x = Math.max(0, x0); x <= Math.min(vp.width - 1, x1); x++) s.push([x, y]); return s; };
      bands.push({ p, strips: [strip(L, T, Rr, T + 3), strip(L, Bt - 3, Rr, Bt), strip(L, T, L + 3, Bt), strip(Rr - 3, T, Rr, Bt)] });
      p.done = sy + y1 + 1;
      return Bt >= vp.height;
    });
    if (!pins.length && !bands.length) continue;
    const shots = [];
    if (pins.length) {
      shots[0] = await shot();
      await page.evaluate(() => document.body.classList.add('chk-recolor')); shots[1] = await shot(); await page.evaluate(() => document.body.classList.remove('chk-recolor'));
    }
    if (bands.length) { await page.evaluate(() => document.body.classList.add('chk-hide')); shots[2] = await shot(); await page.evaluate(() => document.body.classList.remove('chk-hide')); }
    const { covered, cuts } = await page.evaluate(PIXELS, { shots, pins, bands: bands.map(b => ({ strips: b.strips })) });
    covered.forEach((c, k) => { if (c) res.bad.push(pins[k].id + ' badge covered at ' + c + ' of 5 points'); });
    cuts.forEach((hs, k) => { const p = bands[k].p; p.hist ??= hs.map(() => ({})); hs.forEach((h2, z) => { for (const c in h2) p.hist[z][c] = (p.hist[z][c] || 0) + h2[c]; }); if (!all.includes(p)) all.push(p); });
  }
  for (const p of todo.pins) res.bad.push(p.id + ' badge off screen');
  for (const p of todo.bands) res.bad.push(p.id + ' box border not fully checked');
  const diff = (a, b) => Math.abs(a[0] - b[0]) + Math.abs(a[1] - b[1]) + Math.abs(a[2] - b[2]);
  for (const p of all) {
    let cut = 0;
    for (const h2 of p.hist) { const ks = Object.entries(h2); if (!ks.length) continue; const dom = ks.sort((a, b) => b[1] - a[1])[0][0].split(',').map(Number); for (const [c, v] of ks) if (diff(c.split(',').map(Number), dom) > 60) cut += v; }
    if (cut) res.bad.push(p.id + ' box border cuts content (' + cut + ' px)');
  }
  res.bad.push(...await page.evaluate(LAYOUT_PASS));
  return res;
}

(async () => {
  const b = await pw.chromium.launch();
  const out = { bubbles: 0, badges: 0, overlaps: [] };
  for (const run of RUNS) {
    const label = `[${run.w}${run.slow ? ' slow images' : ''}${run.resizeTo ? '->' + run.resizeTo : ''}] `;
    const page = await b.newPage({ viewport: { width: run.w, height: HEIGHT } });
    if (run.slow) await openSlow(page, run.slow);
    else await page.goto(pathToFileURL(doc).href, { waitUntil: 'load' });
    if (run === RUNS[0]) out.overlaps.push(...(await page.evaluate(LOCAL_REFS)));
    if (run === RUNS[0]) out.overlaps.push(...(await page.evaluate(IDS, /[\\/]docs[\\/]mockups[\\/]/.test(doc))));
    await page.evaluate(() => document.fonts.ready);
    await page.waitForTimeout(SETTLE_MS);
    if (run.resizeTo) { await page.setViewportSize({ width: run.resizeTo, height: HEIGHT }); await page.waitForTimeout(SETTLE_MS); }
    const res = await checkRun(page);
    if (run === RUNS[1]) { out.bubbles = res.bubbles; out.badges = res.badges.length; }
    out.overlaps.push(...res.bad.map(x => label + x));
    await page.close();
  }
  await b.close();
  out.overlaps = [...new Set(out.overlaps)];
  console.log(JSON.stringify(out));
  process.exitCode = out.overlaps.length ? 1 : 0;
})().catch(e => { console.error(e); process.exitCode = 2; });
