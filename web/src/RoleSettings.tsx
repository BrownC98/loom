import { Plus } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { FieldReset } from './FieldReset';
import type { AgentView } from './gen/AgentView';
import type { Role } from './gen/Role';
import type { SettingsView } from './gen/SettingsView';
import { blankToNull, canChooseModel, effortWithoutModel, nameTaken, rolesChanged, runnerText } from './roles';

type Editing = number | 'new' | null;

const EMPTY: Role = { name: '', duty: '', agent: null, model: null, effort: null };

export function RoleSettings({ agents, settings, onSettings }: { agents: AgentView[]; settings: SettingsView; onSettings: (next: SettingsView) => void }) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState<Editing>(null);
  const roles = settings.roles;
  const seen = useRef(roles);
  useEffect(() => {
    if (!rolesChanged(seen.current, roles)) return;
    seen.current = roles;
    setEditing(null);
  }, [roles]);
  const save = (next: Role[]) => {
    onSettings({ ...settings, roles: next });
    setEditing(null);
  };
  const text = { none: t('settings.roles.none'), cliDefault: t('settings.roles.cliDefault') };
  const installed = agents.filter((a) => a.places.some((p) => p.path !== null));
  const form = (index: Editing) => (
    <RoleForm
      key="form"
      initial={index === 'new' || index === null ? EMPTY : roles[index]}
      adding={index === 'new'}
      others={roles.filter((_, i) => i !== index)}
      agents={installed}
      onCancel={() => setEditing(null)}
      onSave={(role) => save(index === 'new' ? [...roles, role] : roles.map((r, i) => (i === index ? role : r)))}
    />
  );
  return (
    <section className={settings.alwaysOn.oMode ? undefined : 'faded'}>
      <h4>
        {t('settings.roles.title')}
        <FieldReset path={['roles']} />
      </h4>
      <div className="rows">
        {roles.map((role, i) =>
          editing === i ? (
            form(i)
          ) : (
            <div className={`role${editing === null ? '' : ' faded'}`} key={role.name}>
              <span className="rname">{role.name}</span>
              <span className="duty">{role.duty}</span>
              <span className={`runner${role.agent === null ? ' none' : ''}`} translate="no">
                {runnerText(role, agents, text)}
              </span>
              <span className="row-actions">
                <button type="button" className="btn ghost sm" disabled={editing !== null} onClick={() => setEditing(i)}>
                  {t('settings.roles.edit')}
                </button>
                <button type="button" className="btn ghost sm" disabled={editing !== null} onClick={() => confirm(t('settings.roles.confirmDelete', { name: role.name })) && save(roles.filter((_, j) => j !== i))}>
                  {t('settings.roles.delete')}
                </button>
              </span>
            </div>
          ),
        )}
        {editing === 'new' && form('new')}
      </div>
      <div>
        <button type="button" className="btn sm" disabled={editing !== null} onClick={() => setEditing('new')}>
          <Plus />
          {t('settings.roles.add')}
        </button>
      </div>
      <span className="hint">{t('settings.roles.hint')}</span>
    </section>
  );
}

function RoleForm(props: { initial: Role; adding: boolean; others: Role[]; agents: AgentView[]; onCancel: () => void; onSave: (role: Role) => void }) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState({ ...props.initial, model: props.initial.model ?? '', effort: props.initial.effort ?? '' });
  const { model, effort } = draft;
  const picks = canChooseModel(props.agents.find((a) => a.id === draft.agent));
  const taken = nameTaken(draft.name, props.others);
  const duplicate = draft.name.trim() !== '' && taken;
  const needsModel = effortWithoutModel(props.agents.find((a) => a.id === draft.agent), model, effort);
  return (
    <div className="edit">
      <div className="line">
        <div className="field">
          <label htmlFor="role-name">{t('settings.roles.name')}</label>
          <input id="role-name" value={draft.name} style={{ width: 140 }} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
        </div>
        <div className="field">
          <label htmlFor="role-agent">{t('settings.roles.agent')}</label>
          <select id="role-agent" value={draft.agent ?? ''} onChange={(e) => setDraft({ ...draft, agent: e.target.value || null })}>
            {props.agents.map((a) => (
              <option key={a.id} value={a.id}>
                {a.label}
              </option>
            ))}
            {draft.agent !== null && !props.agents.some((a) => a.id === draft.agent) && <option value={draft.agent}>{draft.agent}</option>}
            <option value="">{t('settings.roles.none')}</option>
          </select>
        </div>
        <div className="field">
          <label htmlFor="role-model">{t('settings.roles.model')}</label>
          <input id="role-model" className="mono" value={model} disabled={!picks} placeholder={t('settings.roles.cliDefault')} style={{ width: 170 }} onChange={(e) => setDraft({ ...draft, model: e.target.value })} />
        </div>
        <div className="field">
          <label htmlFor="role-effort">{t('settings.roles.effort')}</label>
          <input id="role-effort" className="mono" value={effort} disabled={!picks} placeholder={t('settings.roles.cliDefault')} style={{ width: 90 }} onChange={(e) => setDraft({ ...draft, effort: e.target.value })} />
        </div>
      </div>
      {duplicate && <span className="err-text">{t('settings.roles.duplicate')}</span>}
      {needsModel && <span className="err-text">{t('settings.roles.cursorNeedsModel')}</span>}
      <div className="field grow">
        <label htmlFor="role-duty">{t('settings.roles.duty')}</label>
        <textarea id="role-duty" value={draft.duty} placeholder={t('settings.roles.dutyPlaceholder')} onChange={(e) => setDraft({ ...draft, duty: e.target.value })} />
      </div>
      <span className="hint">{t('settings.roles.formHint')}</span>
      <div className="acts">
        <button type="button" className="btn ghost sm" onClick={props.onCancel}>
          {t('settings.roles.cancel')}
        </button>
        <button type="button" className="btn primary sm" disabled={taken || needsModel} onClick={() => props.onSave({ ...draft, name: draft.name.trim(), model: picks ? blankToNull(model) : null, effort: picks ? blankToNull(effort) : null })}>
          {t(props.adding ? 'settings.roles.addSave' : 'settings.roles.save')}
        </button>
      </div>
    </div>
  );
}
