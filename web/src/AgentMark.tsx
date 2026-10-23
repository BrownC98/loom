import type { CSSProperties } from 'react';
import { Terminal } from 'lucide-react';
import claude from '@lobehub/icons-static-svg/icons/claude-color.svg?url';
import cursor from '@lobehub/icons-static-svg/icons/cursor.svg?url';
import gemini from '@lobehub/icons-static-svg/icons/gemini-color.svg?url';
import grok from '@lobehub/icons-static-svg/icons/grok.svg?url';
import openai from '@lobehub/icons-static-svg/icons/openai.svg?url';
import opencode from '@lobehub/icons-static-svg/icons/opencode.svg?url';
import qwen from '@lobehub/icons-static-svg/icons/qwen-color.svg?url';
import type { AgentId } from './gen/AgentId';

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

export function AgentMark(props: { agent: AgentId | null; label: string }) {
  const logo = props.agent === null ? undefined : LOGOS[props.agent];
  if (props.agent !== null && !logo) return <span className="agent">{props.label}</span>;
  if (logo && 'color' in logo) return <img className="agent-logo" src={logo.color} alt={props.label} title={props.label} />;
  return (
    <span className="agent-logo" role="img" aria-label={props.label} title={props.label} style={logo ? ({ '--logo': `url("${logo.mono}")` } as CSSProperties) : undefined}>
      {!logo && <Terminal />}
    </span>
  );
}
