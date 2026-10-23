import { useEffect, useId, useRef, useState, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { renameNode } from './api';
import type { NodeId } from './gen/NodeId';

export type SaveName = (name: string) => Promise<unknown>;

export type Rename = { editing: boolean; open: () => void; close: () => void };

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

export function NameEdit(props: { id: NodeId; name: string; rename: Rename }) {
  const { t } = useTranslation();
  return <NameField name={props.name} label={t('rename.label')} rename={props.rename} save={(name) => renameNode(props.id, name)} />;
}
