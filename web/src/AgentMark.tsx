import type { CSSProperties } from 'react';
import claude from '@lobehub/icons-static-svg/icons/claude-color.svg?url';
import cursor from '@lobehub/icons-static-svg/icons/cursor.svg?url';
import gemini from '@lobehub/icons-static-svg/icons/gemini-color.svg?url';
import grok from '@lobehub/icons-static-svg/icons/grok.svg?url';
import openai from '@lobehub/icons-static-svg/icons/openai.svg?url';
import opencode from '@lobehub/icons-static-svg/icons/opencode.svg?url';
import qwen from '@lobehub/icons-static-svg/icons/qwen-color.svg?url';
import type { AgentId } from './gen/AgentId';

/**
 * 하네스 로고. Orca와 같은 LobeHub 아이콘이다(Codex는 OpenAI 로고). `color`는 색 그림, `mono`는 글자색으로 칠하는 모양이다.
 * 목록에 없는 에이전트는 이름 글자를 보인다.
 */
const LOGOS: Partial<Record<string, { color: string } | { mono: string }>> = {
  claude: { color: claude },
  codex: { mono: openai },
  opencode: { mono: opencode },
  opencode2: { mono: opencode },
  gemini: { color: gemini },
  cursor: { mono: cursor },
  grok: { mono: grok },
  'qwen-code': { color: qwen },
};

/** 카드 머리 등에서 이름 대신 보이는 로고. 이름은 접근 이름과 툴팁으로 남긴다. 셸(`agent`가 없음)은 터미널 모양이다. */
export function AgentMark(props: { agent: AgentId | null; label: string }) {
  const logo = props.agent === null ? undefined : LOGOS[props.agent];
  if (props.agent !== null && !logo) return <span className="agent">{props.label}</span>;
  if (logo && 'color' in logo) return <img className="agent-logo" src={logo.color} alt={props.label} title={props.label} />;
  return (
    <span className="agent-logo" role="img" aria-label={props.label} title={props.label} style={logo ? ({ '--logo': `url("${logo.mono}")` } as CSSProperties) : undefined}>
      {!logo && (
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="M4 17l6-5-6-5M12 19h8" />
        </svg>
      )}
    </span>
  );
}
