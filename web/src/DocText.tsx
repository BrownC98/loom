import { ExternalLink } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { isTauri } from '@tauri-apps/api/core';
import { docPageUrl, docPass, openDoc } from './api';
import type { NodeId } from './gen/NodeId';
import type { TextPart } from './gen/TextPart';
import type { WaitDoc } from './gen/WaitDoc';
import { openLink, openPass } from './openLink';

export function DocText(props: { parts: readonly TextPart[]; doc: WaitDoc | null; node: NodeId | null; onRefusal: (message: string) => void }) {
  const { doc, parts } = props;
  const inline = parts.some((part) => part.part === 'doc');
  return (
    <>
      {parts.map((part, index) => {
        switch (part.part) {
          case 'text':
            return part.text;
          case 'doc':
            return doc ? <DocLink key={index} doc={doc} label={part.label} onRefusal={props.onRefusal} /> : part.label;
          case 'link':
            return <TextLink key={index} label={part.label} href={part.href} node={props.node} />;
        }
      })}
      {doc && !inline && (
        <>
          {parts.length > 0 && ' '}
          <DocLink doc={doc} label={`${doc.id} ${doc.title}`} onRefusal={props.onRefusal} />
        </>
      )}
    </>
  );
}

function TextLink(props: { label: string; href: string; node: NodeId | null }) {
  return (
    <button type="button" className="doc-link" title={props.href} onClick={() => openLink(props.href, props.node)}>
      {props.label}
    </button>
  );
}

export function DocLink(props: { doc: WaitDoc; label: string; onRefusal: (message: string) => void }) {
  const { t } = useTranslation();
  const { doc } = props;
  return isTauri() || window.loomBrowser ? (
    <button
      type="button"
      className="doc-link"
      title={t('inbox.openDocLabel', { id: doc.id, title: doc.title })}
      onClick={() => {
        const browser = window.loomBrowser;
        (browser ? openPass(docPass(doc.id), browser) : openDoc(doc.id)).catch((err: Error) => props.onRefusal(err.message));
      }}
    >
      {props.label}
      <ExternalLink aria-hidden="true" />
    </button>
  ) : (
    <a className="doc-link" href={docPageUrl(doc.id)} target="_blank" rel="noopener" title={t('inbox.openDocTabLabel', { id: doc.id, title: doc.title })}>
      {props.label}
      <ExternalLink aria-hidden="true" />
    </a>
  );
}
