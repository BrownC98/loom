import { ExternalLink } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { isTauri } from '@tauri-apps/api/core';
import { docPass, docUrl, openDoc } from './api';
import type { NodeId } from './gen/NodeId';
import type { WaitDoc } from './gen/WaitDoc';
import { DOC_LINK, linkParts } from './links';
import { openLink, openPass, phoneBrowser } from './openLink';

/**
 * 질문·완료 글과 붙은 문서 링크. 받은편지함과 이벤트 콘솔이 같은 모양으로 그린다(목업 docs/mockups/doc-reopen.html M13 ①②).
 * 글이 문서에 링크를 걸었으면 그 자리에 둔다(2026-10-23 사람 지시 "질문 목업 링크도 아래 완료의 링크처럼 본문에 하이퍼 링크 달리는 방식으로 만들어줘").
 * 없으면 글 끝에 붙인다. 따로 단추가 아니다(2026-10-21 사람 지시 "문서 링크는 별도 버튼이 아니라 질문텍스트의 하이퍼링크표시+아이콘 으로 표시한다").
 */
export function DocText(props: { text: string; doc: WaitDoc | null; node: NodeId | null; onRefusal: (message: string) => void }) {
  const { doc } = props;
  const parts = linkParts(props.text);
  const inline = parts.some((part) => 'href' in part && part.href === DOC_LINK);
  return (
    <>
      {parts.map((part, index) =>
        !('href' in part) ? (
          part.text
        ) : part.href === DOC_LINK && doc ? (
          <DocLink key={index} doc={doc} label={part.label} onRefusal={props.onRefusal} />
        ) : (
          <button key={index} type="button" className="doc-link" title={part.href} onClick={() => openLink(part.href, props.node)}>
            {part.label}
          </button>
        ),
      )}
      {doc && !inline && (
        <>
          {props.text && ' '}
          <DocLink doc={doc} label={`${doc.id} ${doc.title}`} onRefusal={props.onRefusal} />
        </>
      )}
    </>
  );
}

/** 문서 ID의 링크. 대기가 닫혀도 열린다(2026-10-23 사람 "클로드 아티팩트처럼 상시 볼 수 있게 하면 안됨/"). */
export function DocLink(props: { doc: WaitDoc; label: string; onRefusal: (message: string) => void }) {
  const { t } = useTranslation();
  const { doc } = props;
  // 앱 창은 새 탭을 열지 못해 데몬이 기본 브라우저를 연다. 폰 앱은 통행증 주소를 폰 브라우저로 넘긴다. 브라우저와 폰 브라우저는 데몬이 내는 문서를 새 탭에서 연다(목업 docs/mockups/mobile.html ②).
  return isTauri() || phoneBrowser() ? (
    <button
      type="button"
      className="doc-link"
      title={t('inbox.openDocLabel', { id: doc.id, title: doc.title })}
      onClick={() => {
        const browser = phoneBrowser();
        (browser ? openPass(docPass(doc.id), browser) : openDoc(doc.id)).catch((err: Error) => props.onRefusal(err.message));
      }}
    >
      {props.label}
      <ExternalLink aria-hidden="true" />
    </button>
  ) : (
    <a className="doc-link" href={docUrl(doc.id)} target="_blank" rel="noopener" title={t('inbox.openDocTabLabel', { id: doc.id, title: doc.title })}>
      {props.label}
      <ExternalLink aria-hidden="true" />
    </a>
  );
}
