/**
 * 말줄임으로 잘린 글자에 마우스를 올리면 전문을 브라우저 툴팁으로 보인다(목업 docs/mockups/sidebar-resize.html).
 * 화면 전체에 리스너 하나를 둔다. 그래서 새 화면의 말줄임 글자도 따로 손대지 않아도 된다.
 * 사이드바 너비가 바뀌면 잘림도 바뀐다. 그래서 올릴 때마다 다시 잰다. 코드가 직접 단 `title`은 건드리지 않는다.
 */
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
