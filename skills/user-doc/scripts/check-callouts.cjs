// 목업 말풍선 점검(SKILL.md 「목업 말풍선」): 설명마다 말풍선이 있는지, 말풍선이 화면 글자·컨트롤·번호 상자·다른 말풍선과 겹치는지,
// 번호 배지(.mk::after)가 잘리거나 가려지거나 글자를 덮는지 센다. 배치 스크립트(callouts.js)와 따로 센다.
// 사용: node check-callouts.cjs <문서.html>   출력 JSON {"bubbles":N,"badges":N,"overlaps":[...]}, 끝 코드 0 = 통과, 1 = 문제 있음
const path = require('path'), fs = require('fs'), os = require('os'), { pathToFileURL } = require('url');
let pw; try { pw = require('playwright'); } catch { const c = path.join(os.homedir(), '.npm/_npx'); for (const n of fs.readdirSync(c)) { const m = path.join(c, n, 'node_modules/playwright'); if (fs.existsSync(m)) { pw = require(m); break; } } }
(async () => {
  const b = await pw.chromium.launch(); const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  await p.goto(pathToFileURL(path.resolve(process.argv[2])).href, { waitUntil: 'networkidle' });
  // 움직임을 멈춘다. 전환·애니메이션 중에는 위치와 색이 측정마다 달라진다(검토 BLOCK).
  await p.addStyleTag({ content: '*, *::before, *::after { transition: none !important; animation: none !important; }' });
  const res = await p.evaluate(() => {
    const R = r => ({ l: r.left, t: r.top, r: r.right, b: r.bottom });
    const hit = (a, c) => a.l < c.r && c.l < a.r && a.t < c.b && c.t < a.b;
    // 요소 자신의 overflow도 자기 글자와 ::after 배지를 자른다.
    // 화면 끝(html)은 문서 끝 비교가 맡는다.
    const clips = e => { const out = []; for (let a = e; a && a !== document.documentElement; a = a.parentElement) { const cs = getComputedStyle(a); if (cs.overflowX !== 'visible' || cs.overflowY !== 'visible') out.push(a); } return out; };
    // overflow는 테두리 안쪽(padding box)에서 자른다. zoom을 곱해 화면 크기로 바꾼다.
    const clipBox = a => { const k = a.getBoundingClientRect(), s = a.offsetWidth ? k.width / a.offsetWidth : 1, l = k.left + a.clientLeft * s, t = k.top + a.clientTop * s; return { l, t, r: l + a.clientWidth * s, b: t + a.clientHeight * s }; };
    // 넘침을 자르는 조상 밖의 글자는 보이지 않는다. 보이는 부분만 남긴다.
    const visible = (e, r) => { for (const a of clips(e)) { const c = clipBox(a); r = { l: Math.max(r.l, c.l), t: Math.max(r.t, c.t), r: Math.min(r.r, c.r), b: Math.min(r.b, c.b) }; } return r.l < r.r && r.t < r.b ? r : null; };
    const UI = ['img, svg:not(.lead-svg), button, input, select, textarea, .mk', document.body.dataset.ui].filter(Boolean).join(', ');
    const textRuns = root => {
      const out = []; const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
      for (let t; (t = w.nextNode());) {
        const e = t.parentElement; if (!t.textContent.trim() || e.closest('.co, .calls, script, style') || getComputedStyle(e).visibility === 'hidden') continue;
        const g = document.createRange(); g.selectNodeContents(t);
        for (const q of g.getClientRects()) { const r = q.width && q.height && visible(e, R(q)); if (r) out.push({ r, what: t.textContent.trim().slice(0, 20), e }); }
      }
      return out;
    };
    const bad = []; let n = 0, badges = 0;
    const marks = [];
    for (const fig of document.querySelectorAll('.fig')) {
      const title = fig.querySelector('h2')?.textContent.slice(0, 12) ?? '';
      const calls = fig.querySelector(':scope > .calls');
      const cos = [...fig.querySelectorAll(':scope > .co')]; n += cos.length;
      if (calls) {
        if (getComputedStyle(calls).display !== 'none') bad.push(title + ' placement script did not run (.calls still shown)');
        const want = new Set([...calls.querySelectorAll(':scope > div')].map((_, i) => String(i + 1)).concat([...fig.querySelectorAll('.mk[data-n]')].map(m => m.dataset.n)));
        const got = new Set(cos.map(c => c.querySelector('b').textContent));
        for (const k of want) if (!got.has(k)) bad.push(title + ' #' + k + ' has no bubble (box without text, text without box, or no free place)');
      }
      const texts = textRuns(fig);
      const ui = texts.concat([...fig.querySelectorAll(UI)].filter(e => !e.closest('.co, .lead-svg')).map(e => ({ r: R(e.getBoundingClientRect()), what: e.className.baseVal ?? e.className })));
      cos.forEach((c, i) => {
        const cr = R(c.getBoundingClientRect());
        for (const u of ui) if (hit(cr, u.r)) bad.push(title + ' #' + c.querySelector('b').textContent + ' x ' + u.what);
        cos.forEach((d, j) => { if (j > i && hit(cr, R(d.getBoundingClientRect()))) bad.push(title + ' bubble x bubble ' + i + ' ' + j); });
      });
      // 배지는 .mk::after라 상자 왼쪽 위 밖으로 -14px, 22px 원이다(callouts.css). zoom을 곱해 화면 크기로 바꾼다.
      for (const m of fig.querySelectorAll('.mk[data-n]')) {
        badges++;
        const id = title + ' badge ' + m.dataset.n;
        const box = () => { const k = m.getBoundingClientRect(), z = m.offsetWidth ? k.width / m.offsetWidth : 1; return { g: { l: k.left - 14 * z, t: k.top - 14 * z, r: k.left + 8 * z, b: k.top + 8 * z }, z }; };
        // 스크롤하기 전에 자르는 조상(body·html 포함)과 문서 끝을 네 변 모두 비교한다.
        let { g } = box();
        for (const a of clips(m)) { const c = clipBox(a); if (g.l < c.l || g.t < c.t || g.r > c.r || g.b > c.b) bad.push(id + ' clipped by ' + (a.className || a.tagName)); }
        const sx = g.l + scrollX, sy = g.t + scrollY, D = document.documentElement;
        if (sx < 0 || sy < 0 || g.r + scrollX > D.scrollWidth || g.b + scrollY > D.scrollHeight) bad.push(id + ' outside the page');
        m.dataset.chk = marks.length; marks.push(id);
        for (const t of textRuns(fig)) if (!m.contains(t.e) && hit(g, t.r)) bad.push(id + ' over text "' + t.what + '"');
      }
    }
    return { bubbles: n, badges, overlaps: bad, marks };
  });
  // 덮개 점검은 그린 픽셀로 한다. 배지 색만 바꿔 두 번 찍고, 표본 점이 바뀌지 않으면 무언가가 배지를 덮었다.
  // 요소 단위 판정(hit test, 배경·테두리 규칙)은 ::before 덮개와 투명 테두리 상자를 잘못 판정했다(검토 BLOCK).
  await p.addStyleTag({ content: '.mk[data-recolor]::after { background: #00FF00 !important; color: #0000FF !important; }' });
  const vp = p.viewportSize();
  for (const [k, id] of res.marks.entries()) {
    const { g, z } = await p.evaluate(k => {
      const m = document.querySelector(`[data-chk="${k}"]`);
      const box = () => { const r = m.getBoundingClientRect(), z = m.offsetWidth ? r.width / m.offsetWidth : 1; return { g: { l: r.left - 14 * z, t: r.top - 14 * z, r: r.left + 8 * z, b: r.top + 8 * z }, z }; };
      const { g } = box(); scrollBy((g.l + g.r) / 2 - innerWidth / 2, (g.t + g.b) / 2 - innerHeight / 2);
      return box();
    }, k);
    const cx = (g.l + g.r) / 2, cy = (g.t + g.b) / 2, d = 7 * z;
    const pts = [[cx, cy], [cx - d, cy], [cx + d, cy], [cx, cy - d], [cx, cy + d]].map(([x, y]) => [Math.round(x), Math.round(y)]);
    if (pts.some(([x, y]) => x < 0 || y < 0 || x >= vp.width || y >= vp.height)) { res.overlaps.push(id + ' off screen'); continue; }
    const clip = { x: Math.max(0, Math.floor(g.l) - 2), y: Math.max(0, Math.floor(g.t) - 2) };
    clip.width = Math.min(vp.width, Math.ceil(g.r) + 2) - clip.x; clip.height = Math.min(vp.height, Math.ceil(g.b) + 2) - clip.y;
    const shot = async () => (await p.screenshot({ clip, animations: 'disabled' })).toString('base64');
    const before = await shot();
    await p.evaluate(k => { document.querySelector(`[data-chk="${k}"]`).dataset.recolor = ''; }, k);
    const after = await shot();
    await p.evaluate(k => { delete document.querySelector(`[data-chk="${k}"]`).dataset.recolor; }, k);
    const same = await p.evaluate(async ({ before, after, pts }) => {
      const px = async b64 => { const i = new Image(); i.src = 'data:image/png;base64,' + b64; await i.decode(); const c = document.createElement('canvas'); c.width = i.width; c.height = i.height; const x = c.getContext('2d'); x.drawImage(i, 0, 0); return x.getImageData(0, 0, i.width, i.height); };
      const [a, b] = [await px(before), await px(after)];
      return pts.filter(([x, y]) => { const o = (y * a.width + x) * 4; return Math.abs(a.data[o] - b.data[o]) + Math.abs(a.data[o + 1] - b.data[o + 1]) + Math.abs(a.data[o + 2] - b.data[o + 2]) < 60; }).length;
    }, { before, after, pts: pts.map(([x, y]) => [x - clip.x, y - clip.y]) });
    if (same) res.overlaps.push(id + ' covered at ' + same + ' of 5 points');
  }
  res.overlaps = [...new Set(res.overlaps)]; delete res.marks;
  console.log(JSON.stringify(res));
  await b.close();
  process.exitCode = res.overlaps.length ? 1 : 0;
})().catch(e => { console.error(e); process.exitCode = 2; });
