import { useEffect, useState } from 'react'
import { ArrowLeft, Plus, Wand2 } from 'lucide-react'
import {
  SKILL_NAME_PATTERN,
  createSkill,
  readSkill,
  removeSkill,
  setSkillEnabled,
  skillErrorCode,
  skillUnavailableText,
  updateSkill,
  type SkillDocument,
  type SkillEntry,
} from '../skills'
import { W } from '../words'
import { QuestionDialog } from './ZetPanel'

type Draft = { name: string; title: string; description: string; body: string }
const EMPTY: Draft = { name: '', title: '', description: '', body: '' }

/**
 * 설정의 "스킬" 쪽. 목록(이름, 설명, 처음부터 있던 것 / 내가 만든 것, 켜고 끄기) → 누르면 고치기, "새 스킬", 없애기.
 * 처음부터 있던 스킬은 없앨 수 없고 끌 수만 있다. 저장은 문서처럼 그사이 바뀌었으면 묻는다.
 */
export function SkillsSettings({ skills, onChanged }: { skills: SkillEntry[]; onChanged: () => void }) {
  const [editing, setEditing] = useState<{ name: string | null } | null>(null)
  const [message, setMessage] = useState<string | null>(null)

  async function toggle(item: SkillEntry, enabled: boolean) {
    setMessage(null)
    try { await setSkillEnabled(item.name, enabled); onChanged() } catch { setMessage(W.skills.toggleFailed) }
  }

  if (editing) {
    return <SkillEditor name={editing.name} onBack={() => { setEditing(null); onChanged() }} />
  }

  return (
    <div className="settings-page skills-page">
      <header className="settings-page-heading skills-heading">
        <div>
          <h2>{W.skills.word}</h2>
          <p>{W.skills.settingsIntro}</p>
        </div>
        <button className="secondary-button" type="button" onClick={() => setEditing({ name: null })}>
          <Plus size={14} aria-hidden />{W.skills.add}
        </button>
      </header>
      {message && <p className="skills-error" role="alert">{message}</p>}
      {skills.length === 0 ? (
        <div className="skills-empty">
          <Wand2 size={20} aria-hidden />
          <p>{W.skills.empty}</p>
        </div>
      ) : (
        <ul className="skills-list">
          {skills.map((item) => {
            const unavailable = skillUnavailableText(item)
            return (
            <li key={item.name} className="skills-row">
              <button className="skills-row-main" type="button" onClick={() => setEditing({ name: item.name })}>
                <span className="skills-row-title">
                  {item.title}
                  <span className="skills-tag">{item.builtin ? W.skills.builtin : W.skills.mine}</span>
                  {item.syncState === 'conflict' && <span className="skills-tag is-warn">{W.skills.conflict}</span>}
                  {unavailable && <span className="skills-tag is-warn">{W.skills.unavailableTag}</span>}
                </span>
                {item.description && <span className="skills-row-description">{item.description}</span>}
                {/* 까닭 줄: 다른 곳에서 수정한 것은 위 표시가 이미 까닭을 말한다. */}
                {unavailable && item.unavailableReason !== 'skill_conflict' && <span className="skills-row-unavailable">{unavailable}</span>}
                <span className="skills-row-name">/{item.name}</span>
              </button>
              <button
                className={item.enabled ? 'skills-switch is-on' : 'skills-switch'}
                type="button"
                role="switch"
                aria-checked={item.enabled}
                aria-label={W.skills.enable(item.title)}
                onClick={() => void toggle(item, !item.enabled)}
              />
            </li>
            )
          })}
        </ul>
      )}
    </div>
  )
}

function SkillEditor({ name, onBack }: { name: string | null; onBack: () => void }) {
  const isNew = name === null
  const [loaded, setLoaded] = useState<SkillDocument | null>(null)
  const [draft, setDraft] = useState<Draft>(EMPTY)
  const [error, setError] = useState<string | null>(null)
  const [status, setStatus] = useState<string | null>(null)
  const [working, setWorking] = useState(false)
  const [asking, setAsking] = useState<'remove' | 'conflict' | null>(null)

  useEffect(() => {
    if (isNew) return
    readSkill(name).then((doc) => {
      setLoaded(doc)
      setDraft({ name: doc.name, title: doc.title, description: doc.description, body: doc.body })
    }).catch(() => setError(W.skills.loadFailed))
  }, [isNew, name])

  const set = (key: keyof Draft) => (value: string) => { setDraft((current) => ({ ...current, [key]: value })); setStatus(null) }
  const dirty = isNew ? Boolean(draft.name || draft.title || draft.description || draft.body)
    : Boolean(loaded && (draft.title !== loaded.title || draft.description !== loaded.description || draft.body !== loaded.body))

  async function save(force = false) {
    setError(null)
    if (!draft.title.trim()) { setError(W.skills.needTitle); return }
    if (isNew && !SKILL_NAME_PATTERN.test(draft.name)) { setError(W.skills.invalidName); return }
    setWorking(true)
    try {
      if (isNew) {
        await createSkill({ name: draft.name, title: draft.title.trim(), description: draft.description.trim(), body: draft.body })
        setWorking(false)
        onBack()
        return
      }
      if (!loaded) return
      const base = force ? await readSkill(loaded.name) : loaded
      const changes = { title: draft.title.trim(), description: draft.description.trim(), body: draft.body }
      // 뒤쪽은 { status: 'ok' } 만 돌려준다. 저장된 모양(새 contentHash 포함)은 다시 읽는다.
      await updateSkill(loaded.name, changes, base.contentHash)
      const next = await readSkill(loaded.name)
      setLoaded(next)
      setDraft({ name: next.name, title: next.title, description: next.description, body: next.body })
      setStatus(W.skills.saved)
    } catch (reason) {
      const code = skillErrorCode(reason)
      if (code === 'conflict' && !isNew) setAsking('conflict')
      else if (code === 'invalid_name') setError(W.skills.invalidName)
      else if (code === 'already_exists' || code === 'conflict') setError(W.skills.nameTaken)
      else setError(W.skills.saveFailed)
    } finally {
      setWorking(false)
    }
  }

  async function reloadFromDisk() {
    if (!loaded) return
    setAsking(null)
    const doc = await readSkill(loaded.name).catch(() => null)
    if (!doc) { setError(W.skills.loadFailed); return }
    setLoaded(doc)
    setDraft({ name: doc.name, title: doc.title, description: doc.description, body: doc.body })
  }

  async function remove() {
    if (!loaded) return
    setAsking(null)
    try { await removeSkill(loaded.name); onBack() } catch { setError(W.skills.removeFailed) }
  }

  return (
    <div className="settings-page skills-page">
      <button className="skills-back" type="button" onClick={onBack}>
        <ArrowLeft size={14} aria-hidden />{W.skills.back}
      </button>
      <form className="skills-form" onSubmit={(event) => { event.preventDefault(); void save() }}>
        <label className="setting-field">
          <span>{W.skills.fieldTitle}</span>
          <input value={draft.title} onChange={(event) => set('title')(event.target.value)} autoFocus={isNew} />
        </label>
        <label className="setting-field">
          <span>{W.skills.fieldName}</span>
          <input
            value={draft.name}
            onChange={(event) => set('name')(event.target.value.toLowerCase())}
            readOnly={!isNew}
            aria-readonly={!isNew}
            spellCheck={false}
            className="skills-name-input"
          />
          <small>{W.skills.fieldNameHelp}</small>
        </label>
        <label className="setting-field">
          <span>{W.skills.fieldDescription}</span>
          <input value={draft.description} onChange={(event) => set('description')(event.target.value)} />
        </label>
        <label className="setting-field skills-body-field">
          <span>{W.skills.fieldBody}</span>
          <textarea value={draft.body} placeholder={W.skills.bodyHint} onChange={(event) => set('body')(event.target.value)} rows={12} />
        </label>
        {error && <p className="skills-error" role="alert">{error}</p>}
        <div className="skills-actions">
          {!isNew && loaded && !loaded.builtin && (
            <button className="secondary-button skills-remove" type="button" onClick={() => setAsking('remove')}>{W.skills.remove}</button>
          )}
          <span className="skills-status" role="status">{status}</span>
          <button className="secondary-button" type="button" onClick={onBack}>{W.skills.cancel}</button>
          <button className="confirm-button" type="submit" disabled={working || !dirty}>{W.skills.save}</button>
        </div>
      </form>
      {asking === 'remove' && loaded && (
        <QuestionDialog
          title={W.skills.removeTitle(loaded.title)}
          text={W.skills.removeText}
          actions={[
            { label: W.skills.cancel, onSelect: () => setAsking(null), autoFocus: true },
            { label: W.skills.remove, primary: true, onSelect: () => void remove() },
          ]}
          onDismiss={() => setAsking(null)}
        />
      )}
      {asking === 'conflict' && (
        <QuestionDialog
          title={W.skills.conflictTitle}
          text={W.skills.conflictText}
          actions={[
            { label: W.skills.reload, onSelect: () => void reloadFromDisk() },
            { label: W.skills.overwrite, primary: true, onSelect: () => { setAsking(null); void save(true) } },
          ]}
          onDismiss={() => setAsking(null)}
        />
      )}
    </div>
  )
}
