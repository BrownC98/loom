// 목업 말풍선 점검(SKILL.md 「목업 말풍선」): 말풍선이 화면 글자·컨트롤·번호 상자·다른 말풍선과 겹치는지,
// 번호 배지(.mk::after)가 잘리거나 가려지거나 글자를 덮는지 센다. 배치 스크립트(callouts.js)와 따로 센다.
// 사용: node check-callouts.cjs <문서.html 절대 경로>   출력 JSON {"bubbles":N,"badges":N,"overlaps":[...]}
const path = require('path'), fs = require('fs'), os = require('os');
let pw; try { pw = require('playwright'); } catch { const c = path.join(os.homedir(), '.npm/_npx'); for (const n of fs.readdirSync(c)) { const m = path.join(c, n, 'node_modules/playwright'); if (fs.existsSync(m)) { pw = require(m); break; } } }
(async () => {
  const b = await pw.chromium.launch(); const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  await p.goto('file://' + process.argv[2], { waitUntil: 'networkidle' });
  const res = await p.evaluate(() => {
    const R = r => ({ l: r.left, t: r.top, r: r.right, b: r.bottom });
    const hit = (a, c) => a.l < c.r && c.l < a.r && a.t < c.b && c.t < a.b;
    const UI = ['img, svg:not(.lead-svg), button, input, select, textarea, .mk', document.body.dataset.ui].filter(Boolean).join(', ');
    const textRuns = root => {
      const out = []; const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
      for (let t; (t = w.nextNode());) {
        const e = t.parentElement; if (!t.textContent.trim() || e.closest('.co, .calls, script, style') || getComputedStyle(e).visibility === 'hidden') continue;
        const g = document.createRange(); g.selectNodeContents(t);
        for (const q of g.getClientRects()) if (q.width && q.height) out.push({ r: R(q), what: t.textContent.trim().slice(0, 20), e });
      }
      return out;
    };
    const bad = []; let n = 0, badges = 0;
    for (const fig of document.querySelectorAll('.fig')) {
      const title = fig.querySelector('h2')?.textContent.slice(0, 12) ?? '';
      const cos = [...fig.querySelectorAll(':scope > .co')]; n += cos.length;
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
        m.scrollIntoView({ block: 'center' });
        const k = m.getBoundingClientRect(), z = m.offsetWidth ? k.width / m.offsetWidth : 1;
        const g = { l: k.left - 14 * z, t: k.top - 14 * z, r: k.left + 8 * z, b: k.top + 8 * z }, id = title + ' badge ' + m.dataset.n;
        for (let a = m.parentElement; a && a !== document.body; a = a.parentElement) {
          const cs = getComputedStyle(a); if (cs.overflowX === 'visible' && cs.overflowY === 'visible') continue;
          const c = R(a.getBoundingClientRect()); if (g.l < c.l || g.t < c.t || g.r > c.r || g.b > c.b) bad.push(id + ' clipped by ' + (a.className || a.tagName));
        }
        const cx = (g.l + g.r) / 2, cy = (g.t + g.b) / 2, d = 7 * z;
        for (const [x, y] of [[cx, cy], [cx - d, cy], [cx + d, cy], [cx, cy - d], [cx, cy + d]]) {
          const top = document.elementFromPoint(x, y); if (top !== m) bad.push(id + ' covered by ' + (top?.className || top?.tagName));
        }
        for (const t of textRuns(fig)) if (!m.contains(t.e) && hit(g, t.r)) bad.push(id + ' over text "' + t.what + '"');
      }
    }
    return { bubbles: n, badges, overlaps: [...new Set(bad)] };
  });
  console.log(JSON.stringify(res));
  await b.close();
})();
