import { useCallback, useEffect, useRef, useState } from 'react'
import { Check } from 'lucide-react'
import { archiveErrorMessage } from '../archive'
import { getUserProfile, setUserProfile } from '../profile'
import { W } from '../words'

type Draft = { name: string; about: string; style: string }
const EMPTY: Draft = { name: '', about: '', style: '' }

/**
 * 설정의 "나에 대해": 세 AI 가 모두 보고 답하는 사용자 정보. 칸 셋(불릴 이름, 나에 대해, 답하는 방식)과 저장 단추.
 * 백엔드 명령이 없으면 칸과 단추가 눌리지 않게만 둔다(글 없음).
 */
export function UserProfileSettings() {
  const [ready, setReady] = useState<boolean | null>(null)
  const [saved, setSaved] = useState<Draft>(EMPTY)
  const [draft, setDraft] = useState<Draft>(EMPTY)
  const [saving, setSaving] = useState(false)
  const [notice, setNotice] = useState<{ kind: 'done' | 'error'; text: string } | null>(null)
  const noticeTimer = useRef<number | null>(null)

  const load = useCallback(() => {
    setReady(null)
    getUserProfile()
      .then((profile) => {
        const value = { name: profile.name ?? '', about: profile.about ?? '', style: profile.style ?? '' }
        setSaved(value)
        setDraft(value)
        setReady(true)
      })
      .catch(() => setReady(false))
  }, [])
  useEffect(() => {
    load()
    return () => {
      if (noticeTimer.current) window.clearTimeout(noticeTimer.current)
    }
  }, [load])

  const dirty = draft.name !== saved.name || draft.about !== saved.about || draft.style !== saved.style
  const disabled = ready !== true

  async function save() {
    if (!dirty || saving) return
    setSaving(true)
    setNotice(null)
    const value = { name: draft.name.trim(), about: draft.about.trim(), style: draft.style.trim() }
    try {
      await setUserProfile(value)
      setSaved(value)
      setDraft(value)
      setNotice({ kind: 'done', text: W.userProfileSettings.text1 })
      if (noticeTimer.current) window.clearTimeout(noticeTimer.current)
      noticeTimer.current = window.setTimeout(() => setNotice(null), 2400)
    } catch (error) {
      setNotice({ kind: 'error', text: archiveErrorMessage(error, W.userProfileSettings.text2) })
    } finally {
      setSaving(false)
    }
  }

  const set = (key: keyof Draft) => (value: string) => {
    setDraft((current) => ({ ...current, [key]: value }))
    if (notice?.kind === 'done') setNotice(null)
  }

  return (
    <section className="settings-group user-profile" aria-label={W.userProfileSettings.aria1} aria-disabled={disabled || undefined}>
      <p className="user-profile-lead">{W.userProfileSettings.text3(W.provider.codex, W.provider.claude, W.provider.gemini)}</p>
      {/* 읽는 동안·읽지 못했을 때 칸만 잠겨 있지 않게 한 줄(2026-11-17). 읽지 못하면 저장은 계속 막는다(덮어쓰지 않게). */}
      {ready === null && <p className="settings-checking" role="status">{W.userProfileSettings.loading}</p>}
      {ready === false && (
        <div className="settings-problem" role="alert">
          <p>{W.userProfileSettings.failed}</p>
          <button className="secondary-button" type="button" onClick={load}>{W.firstRun.retry}</button>
        </div>
      )}
      <label className="setting-field">
        <span>{W.userProfileSettings.text4}</span>
        <input
          value={draft.name}
          maxLength={40}
          disabled={disabled}
          placeholder={W.userProfileSettings.hint1}
          onChange={(event) => set('name')(event.target.value)}
        />
      </label>
      <label className="setting-field">
        <span>{W.userProfileSettings.text5}</span>
        <textarea
          value={draft.about}
          rows={4}
          maxLength={2000}
          disabled={disabled}
          placeholder={W.userProfileSettings.hint2}
          onChange={(event) => set('about')(event.target.value)}
        />
      </label>
      <label className="setting-field">
        <span>{W.userProfileSettings.text6}</span>
        <textarea
          value={draft.style}
          rows={4}
          maxLength={2000}
          disabled={disabled}
          placeholder={W.userProfileSettings.hint3}
          onChange={(event) => set('style')(event.target.value)}
        />
      </label>
      <div className="user-profile-actions">
        {notice && (
          <span className={`user-profile-notice is-${notice.kind}`} role="status">
            {notice.kind === 'done' && <Check size={14} aria-hidden />}
            {notice.text}
          </span>
        )}
        <button className="confirm-button" type="button" disabled={disabled || !dirty || saving} onClick={() => void save()}>{W.userProfileSettings.text7}</button>
      </div>
    </section>
  )
}
