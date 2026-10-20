import { Fragment, useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { CardView } from './gen/CardView';
import type { Language } from './gen/Language';
import type { SettingsView } from './gen/SettingsView';
import type { TerminalFont } from './gen/TerminalFont';
import type { TerminalTheme } from './gen/TerminalTheme';
import type { Theme } from './gen/Theme';
import { activeFont, DEFAULT_TERMINAL_FONT, FONT_SIZE_RANGE } from './Terminal';
import { chosenTheme, DEFAULT_TERMINAL_THEME, RESET_TERMINAL_THEME, TERMINAL_THEMES, type TerminalThemeName } from './terminalThemes';

type TabProps = { settings: SettingsView; onSettings: (next: SettingsView) => void };

const THEME_CHOICES = [
  {
    value: 'system',
    label: 'common.system',
    icon: (
      <>
        <rect x="3" y="4" width="18" height="12" rx="2" />
        <path d="M8 20h8M12 16v4" />
      </>
    ),
  },
  {
    value: 'light',
    label: 'theme.light',
    icon: (
      <>
        <circle cx="12" cy="12" r="4" />
        <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
      </>
    ),
  },
  { value: 'dark', label: 'theme.dark', icon: <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" /> },
] as const satisfies readonly { value: Theme; label: string; icon: ReactNode }[];

const LANGUAGES = ['system', 'en', 'ko'] as const satisfies readonly Language[];
const CARD_VIEWS = ['terminal', 'header'] as const satisfies readonly CardView[];

/** 「UI」 탭(목업 settings.html 6번, language.html, terminal-font.html): 화면 테마, 카드 보기, 언어. 언어 이름은 번역하지 않는다. */
export function UiTab({ settings, onSettings }: TabProps) {
  const { t } = useTranslation();
  return (
    <>
      <section>
        <h4 id="t-theme">{t('theme.heading')}</h4>
        <div className="choice" role="radiogroup" aria-labelledby="t-theme">
          {THEME_CHOICES.map((c) => (
            <label key={c.value}>
              <input type="radio" name="theme" checked={settings.theme === c.value} onChange={() => onSettings({ ...settings, theme: c.value })} />
              <svg viewBox="0 0 24 24" aria-hidden="true">
                {c.icon}
              </svg>
              {t(c.label)}
            </label>
          ))}
        </div>
        <span className="hint">{t('theme.systemHint')}</span>
      </section>
      <section>
        <h4 id="t-card-view">{t('cardView.heading')}</h4>
        <div className="choice" role="radiogroup" aria-labelledby="t-card-view">
          {CARD_VIEWS.map((v) => (
            <label key={v}>
              <input type="radio" name="card-view" checked={settings.cardView === v} onChange={() => onSettings({ ...settings, cardView: v })} />
              {t(`cardView.${v}`)}
            </label>
          ))}
        </div>
        <span className="hint">{t('cardView.hint')}</span>
      </section>
      <section>
        <h4>
          <label htmlFor="t-lang">{t('settings.language')}</label>
        </h4>
        <div className="field narrow">
          <select id="t-lang" value={settings.language} onChange={(e) => onSettings({ ...settings, language: e.target.value as Language })}>
            {LANGUAGES.map((l) => (
              <option key={l} value={l} lang={l === 'system' ? undefined : l}>
                {l === 'system' ? t('common.system') : t(`languageName.${l}`)}
              </option>
            ))}
          </select>
        </div>
      </section>
    </>
  );
}

/** 「터미널」 탭(목업 6번, terminal-font.html): 글꼴과 모드마다 터미널 테마. */
export function TerminalTab({ settings, onSettings }: TabProps) {
  const { t } = useTranslation();
  const saved = settings.terminalTheme;
  const save = (next: Partial<TerminalTheme>) => onSettings({ ...settings, terminalTheme: { ...saved, ...next } });
  return (
    <>
      <FontFields settings={settings} onSettings={onSettings} />
      <section>
        <h4>{t('theme.darkTheme')}</h4>
        <ThemePicker label={t('theme.darkPicker')} value={chosenTheme(saved, 'dark')} onChoose={(dark) => save({ dark })} />
        <span className="hint">{t('theme.previewHint')}</span>
      </section>
      <section>
        <h4>{t('theme.lightTheme')}</h4>
        <label className="switch">
          <input type="checkbox" checked={saved.matchDark} onChange={(e) => save({ matchDark: e.target.checked })} />
          {t('theme.matchDark')}
        </label>
        <ThemePicker label={t('theme.lightPicker')} value={chosenTheme(saved, 'light')} onChoose={(light) => save({ light })} disabled={saved.matchDark} />
        <span className="hint">{t('theme.matchHint')}</span>
      </section>
      <section>
        <div>
          <button className="btn" onClick={() => onSettings({ ...settings, terminalTheme: RESET_TERMINAL_THEME })}>
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M3 12a9 9 0 1 0 3-6.7L3 8M3 3v5h5" />
            </svg>
            {t('theme.reset')}
          </button>
        </div>
        <span className="hint">{t('theme.resetHint', DEFAULT_TERMINAL_THEME)}</span>
      </section>
    </>
  );
}

/** Orca의 「Font Size」·「Font Family」와 같은 두 칸. 칸을 떠나거나 Enter를 누르면 저장한다. 범위 밖 크기는 버린다. 글꼴 칸을 비우면 기본값이다. */
function FontFields({ settings, onSettings }: TabProps) {
  const { t } = useTranslation();
  const saved = settings.terminalFont;
  const size = activeFont(saved).size;
  const save = (next: Partial<TerminalFont>) => onSettings({ ...settings, terminalFont: { ...saved, ...next } });
  const saveSize = (n: number) => {
    if (Number.isInteger(n) && n >= FONT_SIZE_RANGE.min && n <= FONT_SIZE_RANGE.max) save({ size: n });
  };
  const saveFamily = (text: string) => {
    const family = text.trim() || null;
    if (family !== saved.family) save({ family });
  };
  return (
    <>
      <section>
        <h4>
          <label htmlFor="t-font-size">{t('terminalFont.size')}</label>
        </h4>
        <div className="stepper">
          <button className="btn icon" aria-label={t('terminalFont.smaller')} disabled={size <= FONT_SIZE_RANGE.min} onClick={() => saveSize(size - 1)}>
            −
          </button>
          <input
            id="t-font-size"
            key={size}
            type="number"
            min={FONT_SIZE_RANGE.min}
            max={FONT_SIZE_RANGE.max}
            defaultValue={size}
            onBlur={(e) => saveSize(e.target.valueAsNumber)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') saveSize(e.currentTarget.valueAsNumber);
            }}
          />
          <button className="btn icon" aria-label={t('terminalFont.larger')} disabled={size >= FONT_SIZE_RANGE.max} onClick={() => saveSize(size + 1)}>
            +
          </button>
        </div>
      </section>
      <section>
        <h4>
          <label htmlFor="t-font-family">{t('terminalFont.family')}</label>
        </h4>
        <div className="field">
          <input
            id="t-font-family"
            key={saved.family}
            defaultValue={saved.family ?? ''}
            placeholder={DEFAULT_TERMINAL_FONT.family}
            spellCheck={false}
            autoComplete="off"
            translate="no"
            onBlur={(e) => saveFamily(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') saveFamily(e.currentTarget.value);
            }}
          />
        </div>
        <span className="hint">{t('terminalFont.familyHint')}</span>
      </section>
    </>
  );
}

const THEME_NAMES = Object.keys(TERMINAL_THEMES) as TerminalThemeName[];

/** 테마 목록과 미리보기. 마우스를 올린 테마를 미리 보이고, 고르면(클릭·방향키) 바로 저장한다. */
function ThemePicker(props: { label: string; value: TerminalThemeName; onChoose: (name: TerminalThemeName) => void; disabled?: boolean }) {
  const group = useId();
  const list = useRef<HTMLDivElement>(null);
  const [hovered, setHovered] = useState<TerminalThemeName | null>(null);
  useEffect(() => {
    // 고른 테마가 목록 아래쪽에 있어도 창을 열면 보이게 한다.
    list.current?.querySelector(':checked')?.scrollIntoView({ block: 'nearest' });
  }, []);
  return (
    <fieldset className="tpick" disabled={props.disabled}>
      <div ref={list} className="tlist" role="radiogroup" aria-label={props.label} onMouseLeave={() => setHovered(null)}>
        {THEME_NAMES.map((name) => {
          const t = TERMINAL_THEMES[name];
          return (
            <label key={name} onMouseEnter={() => setHovered(name)}>
              <input
                type="radio"
                name={group}
                checked={name === props.value}
                onChange={() => {
                  setHovered(null);
                  props.onChoose(name);
                }}
              />
              <i style={{ background: t.background, color: t.foreground }}>Aa</i>
              {name}
            </label>
          );
        })}
      </div>
      <Preview name={hovered ?? props.value} />
    </fieldset>
  );
}

const NORMAL = ['black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white'] as const;
const BRIGHT = ['brightBlack', 'brightRed', 'brightGreen', 'brightYellow', 'brightBlue', 'brightMagenta', 'brightCyan', 'brightWhite'] as const;

/** 짧은 셸 출력으로 바탕·글자·16색·커서·선택 영역을 모두 보인다. 정적 HTML이라 터미널을 띄우지 않는다. */
function Preview({ name }: { name: TerminalThemeName }) {
  const { t: tr } = useTranslation();
  const t = TERMINAL_THEMES[name];
  const c = (color: string, text: string) => <span style={{ color }}>{text}</span>;
  const prompt = (
    <>
      {c(t.green, '~/loom')} {c(t.brightBlack, '$')}{' '}
    </>
  );
  const chips = (keys: readonly (keyof typeof t)[]) => (
    <span className="chips">
      {keys.map((k) => (
        <b key={k} style={{ background: t[k] }} />
      ))}
    </span>
  );
  const lines: ReactNode[] = [
    <>{prompt}git status -sb</>,
    <>
      {c(t.cyan, '## main')}...{c(t.red, 'origin/main')}
    </>,
    <>
      {' '}
      {c(t.red, 'M')} src/settings.rs
    </>,
    <>{c(t.red, '??')} web/src/theme.ts</>,
    <>{prompt}cargo test</>,
    <>test settings::put_then_get ... {c(t.green, 'ok')}</>,
    <>
      test daemon::<span style={{ background: t.selectionBackground, color: t.selectionForeground }}>with_theme</span> ... {c(t.red, 'FAILED')}
    </>,
    <>
      {c(t.yellow, 'warning')}: {c(t.blue, '1 test')} {c(t.magenta, 'skipped')}
    </>,
    chips(NORMAL),
    chips(BRIGHT),
    <>
      {prompt}
      <span style={{ background: t.cursor, color: t.cursorAccent }}>{' '}</span>
    </>,
  ];
  return (
    <figure className="tprev">
      <figcaption>{tr('theme.preview', { name })}</figcaption>
      <pre translate="no" style={{ background: t.background, color: t.foreground }}>
        {lines.map((line, i) => (
          <Fragment key={i}>
            {i > 0 && '\n'}
            {line}
          </Fragment>
        ))}
      </pre>
    </figure>
  );
}
