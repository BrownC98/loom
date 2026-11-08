import { ExternalLink } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { isTauri } from '@tauri-apps/api/core';
import { docPageUrl, docPass, openDoc } from './api';
import type { Block } from './gen/Block';
import type { NodeId } from './gen/NodeId';
import type { TextPart } from './gen/TextPart';
import type { WaitDoc } from './gen/WaitDoc';
import { appOpen, openLink, openPass } from './openLink';

type Shown = { doc: WaitDoc | null; node: NodeId | null; onRefusal: (message: string) => void };

export function DocText(props: Shown & { parts: readonly TextPart[] }) {
  const { doc, parts } = props;
  const inline = parts.some((part) => part.part === 'doc');
  return (
    <>
      <Inline {...props} />
      {doc && !inline && (
        <>
          {parts.length > 0 && ' '}
          <DocLink doc={doc} label={`${doc.id} ${doc.title}`} onRefusal={props.onRefusal} />
        </>
      )}
    </>
  );
}

export function DocBlocks(props: Shown & { blocks: readonly Block[] }) {
  const { doc, blocks } = props;
  return (
    <>
      {blocks.map((block, index) => (
        <BlockView key={index} {...props} block={block} />
      ))}
      {doc && !blocks.some(holdsDoc) && (
        <p>
          <DocLink doc={doc} label={`${doc.id} ${doc.title}`} onRefusal={props.onRefusal} />
        </p>
      )}
    </>
  );
}

function holdsDoc(block: Block): boolean {
  const inParts = (parts: readonly TextPart[]) => parts.some((part) => part.part === 'doc');
  switch (block.block) {
    case 'paragraph':
    case 'heading':
      return inParts(block.parts);
    case 'list':
      return block.items.some((item) => item.some(holdsDoc));
    case 'quote':
      return block.blocks.some(holdsDoc);
    case 'table':
      return [block.head, ...block.rows].some((row) => row.some(inParts));
    case 'code':
    case 'rule':
      return false;
  }
}

function BlockView(props: Shown & { block: Block }) {
  const { block } = props;
  switch (block.block) {
    case 'paragraph':
      return (
        <p>
          <Inline {...props} parts={block.parts} />
        </p>
      );
    case 'heading': {
      const Heading = `h${Math.min(Math.max(block.level, 1), 6)}` as 'h1';
      return (
        <Heading>
          <Inline {...props} parts={block.parts} />
        </Heading>
      );
    }
    case 'code':
      return (
        <pre>
          <code>{block.text}</code>
        </pre>
      );
    case 'list': {
      const items = block.items.map((item, index) => (
        <li key={index}>
          {item.map((inner, at) => (
            <BlockView key={at} {...props} block={inner} />
          ))}
        </li>
      ));
      return block.start === null ? <ul>{items}</ul> : <ol start={block.start}>{items}</ol>;
    }
    case 'quote':
      return (
        <blockquote>
          {block.blocks.map((inner, index) => (
            <BlockView key={index} {...props} block={inner} />
          ))}
        </blockquote>
      );
    case 'table': {
      const align = (at: number) => (block.align[at] === 'none' ? undefined : block.align[at]);
      return (
        <div className="table">
          <table>
            <thead>
              <tr>
                {block.head.map((cell, at) => (
                  <th key={at} style={{ textAlign: align(at) }}>
                    <Inline {...props} parts={cell} />
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {block.rows.map((row, index) => (
                <tr key={index}>
                  {row.map((cell, at) => (
                    <td key={at} style={{ textAlign: align(at) }}>
                      <Inline {...props} parts={cell} />
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    }
    case 'rule':
      return <hr />;
  }
}

function Inline(props: Shown & { parts: readonly TextPart[] }) {
  const { doc } = props;
  return (
    <>
      {props.parts.map((part, index) => {
        switch (part.part) {
          case 'text':
            return part.text;
          case 'doc':
            return doc ? <DocLink key={index} doc={doc} label={part.label} onRefusal={props.onRefusal} /> : part.label;
          case 'link':
            return <TextLink key={index} label={part.label} href={part.href} node={props.node} />;
          case 'strong':
            return <strong key={index}>{part.text}</strong>;
          case 'emphasis':
            return <em key={index}>{part.text}</em>;
          case 'code':
            return <code key={index}>{part.text}</code>;
        }
      })}
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
        (browser ? openPass(docPass(doc.id), browser) : appOpen(new URL(docPageUrl(doc.id), location.href).href, () => openDoc(doc.id))).catch((err: Error) => props.onRefusal(err.message));
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
