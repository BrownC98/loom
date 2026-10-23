// 오른쪽 사이드바 [문서] 탭(목업 docs/mockups/doc-reopen.html M13 ③). 닫힌 질문·완료의 문서도 여기서 다시 연다(2026-10-23 사람 "클로드 아티팩트처럼 상시 볼 수 있게 하면 안됨/").
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { listDocs } from './api';
import { DocLink } from './DocText';
import type { DocGroup } from './gen/DocGroup';
import type { DocRow } from './gen/DocRow';
import { dayAndTime } from './i18n';
import { shownDocs } from './shownDocs';

const GROUPS: DocGroup[] = ['mockup', 'report'];

/** 탭을 열 때마다 목록을 새로 읽는다. */
export function DocList() {
  const { t } = useTranslation();
  const [docs, setDocs] = useState<DocRow[] | null>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [hidden, setHidden] = useState<ReadonlySet<DocGroup>>(new Set());
  const [query, setQuery] = useState('');
  useEffect(() => {
    listDocs()
      .then(setDocs)
      .catch((err: Error) => setRefusal(err.message));
  }, []);
  const toggle = (g: DocGroup) =>
    setHidden((prev) => {
      const next = new Set(prev);
      if (!next.delete(g)) next.add(g);
      return next;
    });
  const shown = docs && shownDocs(docs, hidden, query);
  return (
    <>
      <p className="dock-head hint">{t('docs.order')}</p>
      <div className="filters" role="group" aria-label={t('docs.filterKinds')}>
        {GROUPS.map((g) => (
          <button key={g} className="chip" aria-pressed={!hidden.has(g)} onClick={() => toggle(g)}>
            {t(`docs.kind.${g}`)}
          </button>
        ))}
        <input type="search" name="doc" aria-label={t('docs.filter')} placeholder={t('docs.filterPlaceholder')} autoComplete="off" spellCheck={false} value={query} onChange={(e) => setQuery(e.target.value)} />
      </div>
      {refusal && <p className="dock-note error">{refusal}</p>}
      {docs === null || shown === null ? (
        !refusal && <p className="dock-note">{t('docs.reading')}</p>
      ) : docs.length === 0 ? (
        <p className="dock-note">{t('docs.empty')}</p>
      ) : shown.length === 0 ? (
        <p className="dock-note">{t('docs.noMatch')}</p>
      ) : (
        <ul className="docs" aria-label={t('docs.list')}>
          {shown.map((d) => (
            <DocItem key={d.id} doc={d} onRefusal={setRefusal} />
          ))}
        </ul>
      )}
    </>
  );
}

function DocItem({ doc, onRefusal }: { doc: DocRow; onRefusal: (message: string) => void }) {
  const { t } = useTranslation();
  const openable = doc.source || doc.snapshot;
  const time = doc.attachedAt === null ? [] : [dayAndTime(doc.attachedAt * 1000)];
  const meta = [t(`docs.kind.${doc.kind}`), ...(openable ? [doc.folder, ...(doc.source ? [] : [t('docs.lastCopy')]), ...time] : [t('docs.gone')])];
  return (
    <li className={openable ? 'doc-row' : 'doc-row gone'}>
      <span className="id">{doc.id}</span>
      <span className="title">{openable ? <DocLink doc={{ id: doc.id, title: doc.title }} label={doc.title} onRefusal={onRefusal} /> : doc.title}</span>
      <span className="meta" title={meta.join(" · ")}>{meta.join(" · ")}</span>
    </li>
  );
}
