import { ArrowUp } from 'lucide-react';
import { useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { Prompt } from './gen/Prompt';
import type { PromptKey } from './gen/PromptKey';

export function PromptBody(props: { prompt: Prompt; busy: boolean; onPick: (option: number) => void; onType: (text: string, option?: number) => void }) {
  const { prompt } = props;
  const id = useId();
  return (
    <>
      <pre className="prompt">{prompt.body.join('\n')}</pre>
      {prompt.options.length > 0 && (
        <div className="choices prompt-options">
          {prompt.options.map((o, index) =>
            o.writes ? (
              <label key={index} className="choice" htmlFor={`${id}-${index}`}>
                <span className="key">{keyLabel(o.key, index)}</span>
                <span className="label">{o.label}</span>
              </label>
            ) : (
              <button key={index} type="button" className="choice" data-recommended={index === 0 || undefined} disabled={props.busy} onClick={() => props.onPick(index)}>
                <span className="key">{keyLabel(o.key, index)}</span>
                <span className="label">{o.label}</span>
              </button>
            ),
          )}
        </div>
      )}
      {prompt.options.map((o, index) => o.writes && <FieldForm key={index} id={`${id}-${index}`} secret={false} label={o.label} busy={props.busy} onType={(text) => props.onType(text, index)} />)}
      {prompt.field && <FieldForm secret={prompt.field === 'secret'} busy={props.busy} onType={(text) => props.onType(text)} />}
    </>
  );
}

export function keyLabel(key: PromptKey, index: number): string {
  switch (key.kind) {
    case 'char':
      return key.value;
    case 'line':
      return key.value.charAt(0);
    case 'tab':
      return 'tab';
    case 'backTab':
      return '⇧tab';
    case 'move':
    case 'tick':
    case 'permission':
      return String(index + 1);
    case 'enter':
      return '⏎';
  }
}

function FieldForm(props: { id?: string; secret: boolean; label?: string; busy: boolean; onType: (text: string) => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState('');
  const label = props.label ?? t(props.secret ? 'inbox.secretLabel' : 'inbox.textLabel');
  return (
    <form
      className="answer"
      onSubmit={(e) => {
        e.preventDefault();
        props.onType(text);
        setText('');
      }}
    >
      <input id={props.id} className="field" type={props.secret ? 'password' : 'text'} autoComplete="off" spellCheck={false} value={text} onChange={(e) => setText(e.target.value)} aria-label={label} />
      <button type="submit" className="send" aria-label={t('inbox.typeSend')} title={t('inbox.typeSend')} disabled={props.busy}>
        <ArrowUp aria-hidden="true" />
      </button>
    </form>
  );
}
