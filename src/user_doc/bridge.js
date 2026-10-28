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
.decision .opt[data-loom-open]:focus-visible { outline: 2px solid var(--warn, #9a7b16); outline-offset: 2px; }
.decision .opt[data-loom-picked] { border: 2px solid var(--warn, #9a7b16); padding: 4px 9px; }
.decision .opt[data-loom-picked]::after { content: attr(data-loom-picked); margin-left: auto; padding-left: 8px; color: var(--warn, #9a7b16); font-size: 12px; font-weight: 700; white-space: nowrap; }
.decision[data-loom-locked] .opt:not([data-loom-picked]) { opacity: 0.55; }
.decision .loom-write { display: block; width: 100%; min-height: 30px; padding: 5px 8px; line-height: 18px; resize: vertical; border: 1px solid var(--line-strong, #ccc); border-radius: 6px; background: var(--surface, #fff); color: inherit; font: inherit; font-size: 13px; }
.decision .loom-write:focus-visible { outline: none; border-color: var(--warn, #9a7b16); }
.decision .loom-write:disabled { opacity: 0.7; }`;
  document.head.append(style);
  const fit = (field) => {
    field.style.height = 'auto';
    field.style.height = `${field.scrollHeight + field.offsetHeight - field.clientHeight}px`;
  };
  const show = ({ mode, picks, labels }) => {
    for (const box of boxes) {
      const pick = picks[box.id] ?? {};
      for (const opt of box.querySelectorAll('.opt')) {
        const open = mode === 'open';
        const picked = mode !== 'off' && keyOf(opt) === pick.key;
        opt.toggleAttribute('data-loom-open', open);
        if (picked) opt.setAttribute('data-loom-picked', labels.picked);
        else opt.removeAttribute('data-loom-picked');
        if (open) {
          opt.setAttribute('role', 'button');
          opt.setAttribute('tabindex', '0');
          opt.setAttribute('aria-pressed', String(picked));
        } else for (const name of ['role', 'tabindex', 'aria-pressed']) opt.removeAttribute(name);
      }
      box.toggleAttribute('data-loom-locked', mode === 'locked');
      let field = box.querySelector(':scope > .loom-write');
      if (mode === 'off') {
        field?.remove();
        continue;
      }
      if (!field) {
        field = document.createElement('textarea');
        field.className = 'loom-write';
        field.rows = 1;
        field.addEventListener('input', () => {
          fit(field);
          post({ kind: 'write', id: box.id, text: field.value });
        });
        field.addEventListener('keydown', (e) => {
          if (e.key !== 'Enter' || e.shiftKey || e.isComposing) return;
          e.preventDefault();
          post({ kind: 'send' });
        });
        (box.querySelector(':scope > .opts') ?? box.lastElementChild).after(field);
      }
      field.placeholder = labels.write;
      field.setAttribute('aria-label', `${box.id}: ${labels.write}`);
      field.disabled = mode === 'locked';
      if (document.activeElement !== field) field.value = pick.text ?? '';
      fit(field);
    }
  };
  const pickAt = (target) => {
    const opt = target.closest?.('.decision[id] .opt[data-loom-open]');
    if (opt) post({ kind: 'pick', id: opt.closest('.decision[id]').id, key: keyOf(opt) });
    return opt;
  };
  document.addEventListener('click', (e) => pickAt(e.target));
  document.addEventListener('keydown', (e) => {
    if ((e.key === 'Enter' || e.key === ' ') && e.target.matches?.('.opt[data-loom-open]') && pickAt(e.target)) e.preventDefault();
  });
  window.addEventListener('message', (e) => {
    if (e.source !== window.parent || e.data?.loom !== 'page') return;
    if (e.data.kind === 'show') show(e.data);
    if (e.data.kind === 'hello') post({ kind: 'decisions', list });
    if (e.data.kind === 'focus') document.getElementById(e.data.id)?.scrollIntoView({ block: 'center' });
  });
  post({ kind: 'decisions', list });
})();
