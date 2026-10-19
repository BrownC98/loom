import { useState, type FormEvent } from 'react';
import { createNode } from './api';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import type { NodeId } from './gen/NodeId';
import type { SettingsView } from './gen/SettingsView';
import type { Start } from './gen/Start';
import { accountName, activeAccount, windowsText } from './usage';

/** 에이전트 선택 칸에서 「직접 입력」의 값. 카탈로그 id와 겹치지 않는다. */
const COMMAND = '';

/** `agents`는 켠 에이전트만, `initial`은 처음에 고를 기본 에이전트다(목업 5번). */
export function NewNodePopover(props: {
  branch: string;
  agents: AgentView[];
  initial: AgentView | undefined;
  settings: SettingsView;
  accounts: AccountView[] | null;
  onCreated: (id: NodeId) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState('');
  const [choice, setChoice] = useState(props.initial?.id ?? COMMAND);
  const [text, setText] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const agent = props.agents.find((a) => a.id === choice);
  const account = agent?.profile && props.accounts ? activeAccount(props.accounts, props.settings, agent.profile) : undefined;

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    const start: Start = agent ? { kind: 'agent', agent: agent.id, brief: agent.takesBrief ? text : '' } : { kind: 'command', line: text };
    try {
      props.onCreated(await createNode({ name, start }));
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  }

  return (
    <form
      className="pop nodrag nowheel"
      aria-label="새 노드"
      onSubmit={submit}
      onKeyDown={(e) => e.key === 'Escape' && props.onCancel()}
    >
      <h2>
        새 노드<span translate="no">{props.branch}</span>
      </h2>
      <div className="field">
        <label htmlFor="n-name">이름</label>
        <input id="n-name" value={name} onChange={(e) => setName(e.target.value)} autoComplete="off" spellCheck={false} required autoFocus />
      </div>
      <div className="field">
        <label htmlFor="n-agent">에이전트</label>
        <select id="n-agent" value={choice} onChange={(e) => setChoice(e.target.value)}>
          {props.agents.map((a) => (
            <option key={a.id} value={a.id}>
              {a.label}
            </option>
          ))}
          <option value={COMMAND}>직접 입력</option>
        </select>
        {account && (
          <span className="hint">
            계정: {accountName(account)} · {windowsText(account.usage.limits)}
          </span>
        )}
      </div>
      {agent === undefined ? (
        <div className="field">
          <label htmlFor="n-text">명령</label>
          <input id="n-text" value={text} onChange={(e) => setText(e.target.value)} autoComplete="off" spellCheck={false} placeholder="비워 두면 기본 셸을 띄운다" translate="no" />
        </div>
      ) : (
        <div className="field">
          <label htmlFor="n-text">첫 지시문</label>
          <textarea
            id="n-text"
            value={agent.takesBrief ? text : ''}
            onChange={(e) => setText(e.target.value)}
            disabled={!agent.takesBrief}
            placeholder={agent.takesBrief ? '비워 두면 빈 세션으로 시작한다' : `${agent.label}: 첫 지시문을 받지 않는다. 노드를 띄운 뒤 카드에 입력한다.`}
          />
          {agent.takesBrief && <span className="hint">4KB까지. 더 긴 자료는 파일 경로를 적는다.</span>}
        </div>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="pop-actions">
        <button type="button" className="btn ghost" onClick={props.onCancel}>
          취소
        </button>
        <button type="submit" className="btn primary" disabled={busy}>
          만들기
        </button>
      </div>
    </form>
  );
}
