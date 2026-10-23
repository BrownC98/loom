// 목업 말풍선 점검(SKILL.md 「목업 말풍선」): 설명마다 말풍선이 있는지, 말풍선이 화면 글자·컨트롤·번호 상자·다른 말풍선과 겹치는지,
// 번호 배지(.mk::after)가 잘리거나 가려지거나 글자를 덮는지 센다. 배치 스크립트(callouts.js)와 따로 센다.
// 사용: node check-callouts.cjs <문서.html>   출력 JSON {"bubbles":N,"badges":N,"overlaps":[...]}, 끝 코드 0 = 통과, 1 = 문제 있음
const path = require('path'), fs = require('fs'), os = require('os'), { pathToFileURL } = require('url');
let pw; try { pw = require('playwright'); } catch { const c = path.join(os.homedir(), '.npm/_npx'); for (const n of fs.readdirSync(c)) { const m = path.join(c, n, 'node_modules/playwright'); if (fs.existsSync(m)) { pw = require(m); break; } } }
(async () => {
  const b = await pw.chromium.launch(); const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  await p.goto(pathToFileURL(path.resolve(process.argv[2])).href, { waitUntil: 'networkidle' });
  const res = await p.evaluate(() => {
    const R = r => ({ l: r.left, t: r.top, r: r.right, b: r.bottom });
    const hit = (a, c) => a.l < c.r && c.l < a.r && a.t < c.b && c.t < a.b;
    // 요소 자신의 overflow도 자기 글자와 ::after 배지를 자른다.
    const clips = e => { const out = []; for (let a = e; a; a = a.parentElement) { const cs = getComputedStyle(a); if (cs.overflowX !== 'visible' || cs.overflowY !== 'visible') out.push(a); } return out; };
    // 넘침을 자르는 조상 밖의 글자는 보이지 않는다. 보이는 부분만 남긴다.
    const visible = (e, r) => { for (const a of clips(e)) { const c = R(a.getBoundingClientRect()); r = { l: Math.max(r.l, c.l), t: Math.max(r.t, c.t), r: Math.min(r.r, c.r), b: Math.min(r.b, c.b) }; } return r.l < r.r && r.t < r.b ? r : null; };
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
    // 배지가 실제로 칠해졌는지 본다. pointer-events:none인 덮개도 맞히게 모두 켠다(지시선 svg는 뺀다).
    const probe = document.createElement('style'); probe.textContent = '*:not(.lead-svg):not(.lead-svg *) { pointer-events: auto !important; }'; document.head.appendChild(probe);
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
        for (const a of clips(m)) { const c = R(a.getBoundingClientRect()); if (g.l < c.l || g.t < c.t || g.r > c.r || g.b > c.b) bad.push(id + ' clipped by ' + (a.className || a.tagName)); }
        const sx = g.l + scrollX, sy = g.t + scrollY, D = document.documentElement;
        if (sx < 0 || sy < 0 || g.r + scrollX > D.scrollWidth || g.b + scrollY > D.scrollHeight) bad.push(id + ' outside the page');
        scrollBy((g.l + g.r) / 2 - innerWidth / 2, (g.t + g.b) / 2 - innerHeight / 2);
        let z; ({ g, z } = box());
        const cx = (g.l + g.r) / 2, cy = (g.t + g.b) / 2, d = 7 * z;
        for (const [x, y] of [[cx, cy], [cx - d, cy], [cx + d, cy], [cx, cy - d], [cx, cy + d]]) {
          const top = document.elementFromPoint(x, y); if (top !== m) bad.push(id + ' covered by ' + (top ? top.className || top.tagName : 'nothing (off screen)'));
        }
        for (const t of textRuns(fig)) if (!m.contains(t.e) && hit(g, t.r)) bad.push(id + ' over text "' + t.what + '"');
      }
    }
    probe.remove();
    return { bubbles: n, badges, overlaps: [...new Set(bad)] };
  });
  console.log(JSON.stringify(res));
  await b.close();
  process.exitCode = res.overlaps.length ? 1 : 0;
})().catch(e => { console.error(e); process.exitCode = 2; });
