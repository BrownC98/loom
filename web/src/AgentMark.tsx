import type { CSSProperties } from 'react';
import { Terminal } from 'lucide-react';
import amp from '@lobehub/icons-static-svg/icons/amp-color.svg?url';
import antigravity from '@lobehub/icons-static-svg/icons/antigravity-color.svg?url';
import claude from '@lobehub/icons-static-svg/icons/claude-color.svg?url';
import cline from '@lobehub/icons-static-svg/icons/cline.svg?url';
import codebuddy from '@lobehub/icons-static-svg/icons/codebuddy-color.svg?url';
import commandcode from '@lobehub/icons-static-svg/icons/commandcode.svg?url';
import cursor from '@lobehub/icons-static-svg/icons/cursor.svg?url';
import devin from '@lobehub/icons-static-svg/icons/devin-color.svg?url';
import gemini from '@lobehub/icons-static-svg/icons/gemini-color.svg?url';
import githubcopilot from '@lobehub/icons-static-svg/icons/githubcopilot.svg?url';
import goose from '@lobehub/icons-static-svg/icons/goose.svg?url';
import grok from '@lobehub/icons-static-svg/icons/grok.svg?url';
import hermesagent from '@lobehub/icons-static-svg/icons/hermesagent.svg?url';
import kilocode from '@lobehub/icons-static-svg/icons/kilocode.svg?url';
import kimi from '@lobehub/icons-static-svg/icons/kimi.svg?url';
import kiro from '@lobehub/icons-static-svg/icons/kiro-color.svg?url';
import meta from '@lobehub/icons-static-svg/icons/meta-color.svg?url';
import mistral from '@lobehub/icons-static-svg/icons/mistral-color.svg?url';
import openai from '@lobehub/icons-static-svg/icons/openai.svg?url';
import openclaw from '@lobehub/icons-static-svg/icons/openclaw-color.svg?url';
import opencode from '@lobehub/icons-static-svg/icons/opencode.svg?url';
import pi from '@lobehub/icons-static-svg/icons/pi.svg?url';
import qoder from '@lobehub/icons-static-svg/icons/qoder-color.svg?url';
import qwen from '@lobehub/icons-static-svg/icons/qwen-color.svg?url';
import trae from '@lobehub/icons-static-svg/icons/trae-color.svg?url';
import xiaomimimo from '@lobehub/icons-static-svg/icons/xiaomimimo.svg?url';
import zai from '@lobehub/icons-static-svg/icons/zai.svg?url';
import aider from './agent-logos/aider.png?url';
import ante from './agent-logos/ante.png?url';
import autohand from './agent-logos/autohand.svg?url';
import continueLogo from './agent-logos/continue.svg?url';
import crush from './agent-logos/crush.png?url';
import dsh from './agent-logos/dsh.svg?url';
import jcode from './agent-logos/jcode.svg?url';
import omp from './agent-logos/omp.svg?url';
import openclaude from './agent-logos/openclaude.svg?url';
import type { AgentId } from './gen/AgentId';

const LOGOS: Partial<Record<string, { color: string } | { mono: string }>> = {
  claude: { color: claude },
  openclaude: { color: openclaude },
  codex: { mono: openai },
  grok: { mono: grok },
  copilot: { mono: githubcopilot },
  opencode: { mono: opencode },
  opencode2: { mono: opencode },
  'mimo-code': { mono: xiaomimimo },
  ante: { color: ante },
  trae: { color: trae },
  muse: { color: meta },
  dsh: { mono: dsh },
  qoder: { color: qoder },
  'qoder-cn': { color: qoder },
  zcode: { mono: zai },
  pi: { mono: pi },
  omp: { color: omp },
  gemini: { color: gemini },
  antigravity: { color: antigravity },
  aider: { color: aider },
  goose: { mono: goose },
  amp: { color: amp },
  kilo: { mono: kilocode },
  kiro: { color: kiro },
  crush: { color: crush },
  autohand: { mono: autohand },
  cline: { mono: cline },
  'command-code': { mono: commandcode },
  continue: { mono: continueLogo },
  cursor: { mono: cursor },
  kimi: { mono: kimi },
  'mistral-vibe': { color: mistral },
  'qwen-code': { color: qwen },
  hermes: { mono: hermesagent },
  devin: { color: devin },
  openclaw: { color: openclaw },
  codebuddy: { color: codebuddy },
  jcode: { mono: jcode },
};

export function AgentMark(props: { agent: AgentId | null; label: string }) {
  const logo = props.agent === null ? undefined : LOGOS[props.agent];
  if (logo && 'color' in logo) return <img className="agent-logo" src={logo.color} alt={props.label} title={props.label} />;
  if (props.agent !== null && !logo) {
    return <span className="agent-logo letter" role="img" aria-label={props.label} title={props.label} data-letter={props.label.charAt(0).toUpperCase()} />;
  }
  return (
    <span className="agent-logo" role="img" aria-label={props.label} title={props.label} style={logo ? ({ '--logo': `url("${logo.mono}")` } as CSSProperties) : undefined}>
      {!logo && <Terminal />}
    </span>
  );
}
