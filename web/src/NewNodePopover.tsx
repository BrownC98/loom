import { useState, type FormEvent } from 'react';
import { createNode } from './api';
import type { NodeId } from './gen/NodeId';
import type { Profile } from './gen/Profile';
import type { Start } from './gen/Start';
import { PROFILE_LABEL } from './NodeCard';

const PROFILES: Profile[] = ['claude', 'codex', 'command'];

function startOf(profile: Profile, text: string): Start {
  switch (profile) {
    case 'claude':
    case 'codex':
      return { profile, brief: text };
    case 'command':
      return { profile, line: text };
  }
}

export function NewNodePopover(props: { branch: string; onCreated: (id: NodeId) => void; onCancel: () => void }) {
  const [name, setName] = useState('');
  const [profile, setProfile] = useState<Profile>('claude');
  const [text, setText] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      props.onCreated(await createNode({ name, start: startOf(profile, text) }));
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
      <fieldset className="field">
        <legend>프로필</legend>
        <div className="seg">
          {PROFILES.map((p) => (
            <label key={p}>
              <input type="radio" name="profile" checked={profile === p} onChange={() => setProfile(p)} />
              <span>{PROFILE_LABEL[p]}</span>
            </label>
          ))}
        </div>
      </fieldset>
      {profile === 'command' ? (
        <div className="field">
          <label htmlFor="n-text">명령</label>
          <input id="n-text" value={text} onChange={(e) => setText(e.target.value)} autoComplete="off" spellCheck={false} placeholder="비워 두면 기본 셸을 띄운다" translate="no" />
        </div>
      ) : (
        <div className="field">
          <label htmlFor="n-text">첫 지시문</label>
          <textarea id="n-text" value={text} onChange={(e) => setText(e.target.value)} placeholder="비워 두면 빈 세션으로 시작한다" />
          <span className="hint">4KB까지. 더 긴 자료는 파일 경로를 적는다.</span>
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
