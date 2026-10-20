// 이름 바꾸기(docs/mockups/node-rename.html): 이름 글자를 두 번 누르거나 F2를 누르면 그 자리가 입력창이 된다.
// 사이드바 트리 줄(노드, 워크트리 브랜치), 표준 카드 머리, 노드 정보(심플 카드·오른쪽 칸 머리)가 함께 쓴다.
import { useEffect, useId, useRef, useState, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { renameNode } from './api';
import type { NodeId } from './gen/NodeId';

/** 새 이름을 데몬에 보낸다. 거절하면 데몬의 문장을 담은 Error로 끝난다. */
export type SaveName = (name: string) => Promise<unknown>;

/** 편집을 여닫는 쪽. 트리는 줄마다, 카드와 오른쪽 칸은 하나씩 가진다. F2를 받는 쪽이 `open`을 부른다. */
export type Rename = { editing: boolean; open: () => void; close: () => void };

/** 닫은 뒤 포커스가 갈 곳이 없으면(입력창이 사라짐) `home`에 포커스를 돌린다. 입력창 밖을 눌러 닫았으면 누른 곳에 둔다. */
export function returnFocus(home: () => HTMLElement | null | undefined) {
  requestAnimationFrame(() => {
    if (document.activeElement === document.body) home()?.focus();
  });
}

/** 카드와 오른쪽 칸의 편집 상태. `home`이 없으면 포커스를 돌리지 않는다. */
export function useRename(home?: RefObject<HTMLElement | null>): Rename {
  const [editing, setEditing] = useState(false);
  return {
    editing,
    open: () => setEditing(true),
    close: () => {
      setEditing(false);
      if (home) returnFocus(() => home.current);
    },
  };
}

function Editor(props: { name: string; label: string; save: SaveName; onClose: () => void }) {
  const { t } = useTranslation();
  const tipId = useId();
  const input = useRef<HTMLInputElement>(null);
  const [value, setValue] = useState(props.name);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);
  const save = () => {
    if (value.trim() === props.name) return props.onClose();
    setSaving(true);
    props.save(value).then(props.onClose, (e: Error) => {
      setSaving(false);
      setError(e.message);
    });
  };
  return (
    // 두 번 누르기는 카드 머리의 확대로, 누르기는 카드 끌기로 가지 않게 한다.
    <span className="rename nodrag" onDoubleClick={(e) => e.stopPropagation()}>
      <input
        ref={input}
        name="name"
        aria-label={props.label}
        autoComplete="off"
        spellCheck={false}
        value={value}
        readOnly={saving}
        aria-invalid={error !== null || undefined}
        aria-describedby={tipId}
        onChange={(e) => {
          setValue(e.target.value);
          setError(null);
        }}
        onBlur={props.onClose}
        // 화살표·Enter가 트리와 카드의 키보드 이동으로 가지 않게 한다.
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === 'Escape') props.onClose();
          else if (e.key === 'Enter' && !saving) save();
        }}
      />
      {error === null ? (
        <span className="tip" id={tipId}>
          {t('rename.hint')}
        </span>
      ) : (
        <span className="tip err" id={tipId} role="alert">
          {error}
        </span>
      )}
    </span>
  );
}

/** 이름 글자. 편집 중이면 입력창이다. 거절되면 입력창에 남아 사유를 보인다. `label`은 입력창의 이름, `detail`은 툴팁 앞줄이다. */
export function NameField(props: { name: string; label: string; detail?: string; rename: Rename; save: SaveName }) {
  const { t } = useTranslation();
  if (props.rename.editing) return <Editor name={props.name} label={props.label} save={props.save} onClose={props.rename.close} />;
  return (
    <span
      className="name renamable"
      translate="no"
      title={props.detail === undefined ? t('rename.title') : `${props.detail}\n${t('rename.title')}`}
      onDoubleClick={(e) => {
        e.stopPropagation();
        props.rename.open();
      }}
    >
      {props.name}
    </span>
  );
}

/** 노드 이름. */
export function NameEdit(props: { id: NodeId; name: string; rename: Rename }) {
  const { t } = useTranslation();
  return <NameField name={props.name} label={t('rename.label')} rename={props.rename} save={(name) => renameNode(props.id, name)} />;
}
