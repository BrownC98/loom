export function showClippedText(doc: Document) {
  doc.addEventListener('mouseover', (e) => {
    let el = e.target instanceof HTMLElement ? e.target : null;
    while (el && getComputedStyle(el).textOverflow !== 'ellipsis') el = el.parentElement;
    if (!el || (el.hasAttribute('title') && !('clipped' in el.dataset))) return;
    if (el.scrollWidth > el.clientWidth) {
      el.title = el.textContent ?? '';
      el.dataset.clipped = '';
    } else if ('clipped' in el.dataset) {
      el.removeAttribute('title');
      delete el.dataset.clipped;
    }
  });
}
