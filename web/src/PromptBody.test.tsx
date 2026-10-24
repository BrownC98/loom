import i18next from 'i18next';
import { renderToStaticMarkup } from 'react-dom/server';
import { I18nextProvider, initReactI18next } from 'react-i18next';
import { beforeAll, describe, expect, it } from 'vitest';
import type { Prompt } from './gen/Prompt';
import en from './locales/en';
import { keyLabel, PromptBody } from './PromptBody';

const i18n = i18next.createInstance();
beforeAll(() => i18n.use(initReactI18next).init({ lng: 'en', resources: { en: { translation: en } } }));

const render = (prompt: Prompt) =>
  renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <PromptBody prompt={prompt} busy={false} onPick={() => {}} onType={() => {}} />
    </I18nextProvider>,
  );

const trust: Prompt = {
  rows: [],
  body: ['Accessing workspace:', '/tmp/repo'],
  summary: 'Accessing workspace:',
  options: [
    { label: 'No, exit', key: { kind: 'move', value: 0 } },
    { label: 'Yes, I trust this folder', key: { kind: 'move', value: 1 } },
  ],
};

describe('PromptBody', () => {
  it('draws one button per terminal option with the key it types', () => {
    const html = render(trust);
    expect(html.match(/<button/g)).toHaveLength(2);
    expect(html).toContain('<span class="key">2</span><span class="label">Yes, I trust this folder</span>');
    expect(html).not.toContain('<input');
  });

  it('draws a hidden field for a password and a plain one for a name', () => {
    const asked = (field: 'secret' | 'text'): Prompt => ({ rows: [], body: ['[sudo] password for me:'], summary: '', options: [], field });
    const secret = render(asked('secret'));
    expect(secret).toContain('type="password"');
    expect(secret).toContain('aria-label="Password"');
    expect(secret).toContain('autoComplete="off"');
    expect(secret).not.toContain('class="choices');
    expect(render(asked('text'))).toContain('type="text"');
  });

  it('labels each key kind', () => {
    expect(keyLabel({ kind: 'char', value: 'a' }, 0)).toBe('a');
    expect(keyLabel({ kind: 'line', value: 'y' }, 0)).toBe('y');
    expect(keyLabel({ kind: 'move', value: -1 }, 0)).toBe('1');
    expect(keyLabel({ kind: 'permission', value: 'once' }, 2)).toBe('3');
    expect(keyLabel({ kind: 'tab' }, 0)).toBe('tab');
    expect(keyLabel({ kind: 'backTab' }, 0)).toBe('⇧tab');
  });
});
