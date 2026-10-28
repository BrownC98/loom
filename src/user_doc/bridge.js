(() => {
  if (window.parent === window) return;
  const boxes = [...document.querySelectorAll('.decision[id]')];
  const keyOf = (opt) => opt.querySelector('.key')?.textContent.trim() ?? '';
  const list = boxes.map((box) => ({
    id: box.id,
    keys: [...box.querySelectorAll('.opt')].map(keyOf).filter(Boolean),
    recommended: keyOf(box.querySelector('.opt[data-recommended]') ?? document.createElement('i')) || null,
  }));
  const post = (message) => window.parent.postMessage({ loom: 'doc', ...message }, '*');
  const style = document.createElement('style');
  style.textContent = `
.decision .opt[data-loom-open] { cursor: pointer; }
.decision .opt[data-loom-open]:hover { filter: brightness(0.96); }
.decision .opt[data-loom-picked] { border: 2px solid var(--warn, #9a7b16); padding: 4px 9px; }
.decision .opt[data-loom-picked]::after { content: attr(data-loom-picked); margin-left: auto; padding-left: 8px; color: var(--warn, #9a7b16); font-size: 12px; font-weight: 700; white-space: nowrap; }
.decision[data-loom-locked] .opt:not([data-loom-picked]) { opacity: 0.55; }
.decision .loom-write { width: 100%; height: 30px; padding: 0 8px; border: 1px solid var(--line-strong, #ccc); border-radius: 6px; background: var(--surface, #fff); color: inherit; font: inherit; font-size: 13px; }
.decision .loom-write:focus-visible { outline: none; border-color: var(--warn, #9a7b16); }
.decision .loom-write:disabled { opacity: 0.7; }`;
  document.head.append(style);
  const show = ({ mode, picks, labels }) => {
    for (const box of boxes) {
      const pick = picks[box.id] ?? {};
      for (const opt of box.querySelectorAll('.opt')) {
        opt.toggleAttribute('data-loom-open', mode === 'open');
        if (mode !== 'off' && keyOf(opt) === pick.key) opt.setAttribute('data-loom-picked', labels.picked);
        else opt.removeAttribute('data-loom-picked');
      }
      box.toggleAttribute('data-loom-locked', mode === 'locked');
      let field = box.querySelector(':scope > .loom-write');
      if (mode === 'off') {
        field?.remove();
        continue;
      }
      if (!field) {
        field = document.createElement('input');
        field.className = 'loom-write';
        field.addEventListener('input', () => post({ kind: 'write', id: box.id, text: field.value }));
        (box.querySelector(':scope > .opts') ?? box.lastElementChild).after(field);
      }
      field.placeholder = labels.write;
      field.disabled = mode === 'locked';
      if (document.activeElement !== field) field.value = pick.text ?? '';
    }
  };
  document.addEventListener('click', (e) => {
    const opt = e.target.closest('.decision[id] .opt[data-loom-open]');
    if (opt) post({ kind: 'pick', id: opt.closest('.decision[id]').id, key: keyOf(opt) });
  });
  window.addEventListener('message', (e) => {
    if (e.source !== window.parent || e.data?.loom !== 'page') return;
    if (e.data.kind === 'show') show(e.data);
    if (e.data.kind === 'hello') post({ kind: 'decisions', list });
    if (e.data.kind === 'focus') document.getElementById(e.data.id)?.scrollIntoView({ block: 'center' });
  });
  post({ kind: 'decisions', list });
})();
