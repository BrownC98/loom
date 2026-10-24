import { useEffect, useId, useRef, useState, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { renameNode } from './api';
import type { NodeId } from './gen/NodeId';
import { editEnd, type SaveName } from './editEnd';

export type Rename = { editing: boolean; open: () => void; close: () => void; refused?: (message: string) => void };

export function returnFocus(home: () => HTMLElement | null | undefined) {
  requestAnimationFrame(() => {
    if (document.activeElement === document.body) home()?.focus();
  });
}

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

function Editor(props: { name: string; label: string; save: SaveName; onClose: () => void; onRefusedAfterClose?: (message: string) => void }) {
  const { t } = useTranslation();
  const tipId = useId();
  const input = useRef<HTMLInputElement>(null);
  const [value, setValue] = useState(props.name);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const shown = useRef(true);
  useEffect(() => {
    shown.current = true;
    input.current?.focus();
    input.current?.select();
    return () => {
      shown.current = false;
    };
  }, []);
  const ending = useRef(editEnd()).current;
  const save = () => {
    const fail = (message: string) => {
      if (!shown.current) return props.onRefusedAfterClose?.(message);
      setSaving(false);
      setError(message);
    };
    if (ending.save(value, props.name, props.save, props.onClose, fail)) setSaving(true);
  };
  return (
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
        onBlur={save}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === 'Escape') ending.cancel(props.onClose);
          else if (e.key === 'Enter') save();
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

export function NameField(props: { name: string; label: string; detail?: string; rename: Rename; save: SaveName }) {
  const { t } = useTranslation();
  if (props.rename.editing) return <Editor name={props.name} label={props.label} save={props.save} onClose={props.rename.close} onRefusedAfterClose={props.rename.refused} />;
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

export function NameEdit(props: { id: NodeId; name: string; rename: Rename }) {
  const { t } = useTranslation();
  return <NameField name={props.name} label={t('rename.label')} rename={props.rename} save={(name) => renameNode(props.id, name)} />;
}
