import { useCallback, useEffect, useRef, useState } from 'react'
import { LoaderCircle, Plus } from 'lucide-react'
import {
  deleteSavedLogin,
  listSavedLogins,
  saveSavedLogin,
  savedLoginErrorCode,
  siteHost,
  siteOrigin,
  type SavedLogin,
  type SavedLoginInput,
} from '../saved-logins'
import { W } from '../words'
import { QuestionDialog } from './ZetPanel'

/** 누른 단추가 사라지거나 잠겨 포커스가 화면 밖(body)으로 빠졌을 때만 다음 자리로 옮긴다(용어 사전과 같음). */
function focusIfLost(target: () => HTMLElement | null | undefined) {
  requestAnimationFrame(() => {
    const active = document.activeElement
    if (!active || active === document.body) target()?.focus()
  })
}

function errorText(error: unknown, plain: string) {
  const code = savedLoginErrorCode(error)
  return (code && W.savedLogins.errors[code]) || plain
}

/**
 * 설정 › 저장된 로그인(2026-11-21 본부 지시, 뒤쪽 275e017 saved-logins.ts).
 * 용어 사전의 "사람 이름" 목록과 같은 부품: 위에 제목·한 줄·"새 로그인", 줄마다 이름과 "사이트 · 아이디", 오른쪽에 수정·삭제.
 * 넣기·고치기 칸은 사이트 주소, 이름, 아이디, 비밀번호, 쓸 때마다 묻기. 로그인 화면 경로와 기본 계정은 같은 사이트(경로)에 로그인이 둘 이상일 때만 보인다
 * (이미 정해 둔 값이 있으면 늘 보인다). 비밀번호는 목록에 없고(뒤쪽도 주지 않는다) React 상태에도 담지 않는다: 칸에만 있다가 보낼 때 읽고 바로 비운다.
 */
export function SavedLoginsSettings() {
  const words = W.savedLogins
  const [list, setList] = useState<SavedLogin[] | null>(null)
  const [problem, setProblem] = useState<string | null>(null)
  const [adding, setAdding] = useState(false)
  const [editing, setEditing] = useState<string | null>(null)
  const [asking, setAsking] = useState<SavedLogin | null>(null)
  const [working, setWorking] = useState(false)
  const [message, setMessage] = useState<{ text: string; alert: boolean } | null>(null)
  const addButton = useRef<HTMLButtonElement>(null)
  const rows = useRef<HTMLUListElement>(null)
  const focusAdd = () => focusIfLost(() => addButton.current)
  const focusRow = (id: string, which: string) => focusIfLost(() => rows.current?.querySelector<HTMLElement>(`[data-login-id="${CSS.escape(id)}"] .${which}`))

  const reload = useCallback(async () => {
    try {
      setList(await listSavedLogins())
      setProblem(null)
    } catch (error) {
      setProblem(errorText(error, W.savedLogins.loadFailed))
    }
  }, [])

  useEffect(() => { void reload() }, [reload])

  /** 넣기·고치기 한 번. input 의 비밀번호는 넘기기만 한다(칸은 부르는 쪽이 이미 비웠다). */
  async function save(input: SavedLoginInput) {
    if (working) return false
    setWorking(true)
    setMessage(null)
    try {
      await saveSavedLogin(input)
      await reload()
      setMessage({ text: words.saved, alert: false })
      return true
    } catch (error) {
      if (savedLoginErrorCode(error) === 'not_found') await reload()
      setMessage({ text: errorText(error, words.saveFailed), alert: true })
      return false
    } finally {
      setWorking(false)
    }
  }

  async function remove(login: SavedLogin) {
    if (working) return
    setWorking(true)
    setMessage(null)
    try {
      await deleteSavedLogin(login.id)
      await reload()
      setMessage({ text: words.removed, alert: false })
    } catch (error) {
      const code = savedLoginErrorCode(error)
      await reload()
      setMessage(code === 'not_found' ? { text: words.removed, alert: false } : { text: errorText(error, words.removeFailed), alert: true })
    } finally {
      setWorking(false)
      focusAdd()
    }
  }

  const stopAsking = () => { if (asking) focusRow(asking.id, 'term-person-delete'); setAsking(null) }

  return (
    <div className="settings-page saved-logins">
      <header className="settings-page-heading skills-heading">
        <div>
          <h2>{words.title}</h2>
          <p>{words.intro}</p>
        </div>
        {list && (
          <button ref={addButton} className="secondary-button" type="button" onClick={() => { setAdding(true); setEditing(null); setMessage(null) }}>
            <Plus size={14} aria-hidden />{words.add}
          </button>
        )}
      </header>
      {message && <p className={message.alert ? 'term-note is-alert' : 'term-note'} role={message.alert ? 'alert' : 'status'}>{message.text}</p>}
      {problem ? (
        <div className="settings-problem" role="alert">
          <p>{problem}</p>
          <button className="secondary-button" type="button" onClick={() => void reload()}>{W.firstRun.retry}</button>
        </div>
      ) : list === null ? (
        <p className="settings-checking" role="status"><LoaderCircle size={14} className="is-spinning" aria-hidden />{W.firstRun.checking}</p>
      ) : (
        <>
          {adding && (
            <LoginForm
              all={list}
              working={working}
              onCancel={() => { setAdding(false); focusAdd() }}
              onSave={async (input) => { if (await save(input)) { setAdding(false); focusAdd() } }}
            />
          )}
          <p className="term-count" role="status">{words.total(list.length)}</p>
          {list.length === 0 && <p className="term-note">{words.empty}</p>}
          {list.length > 0 && (
            <ul ref={rows} className="term-rows">
              {list.map((login) => {
                if (editing === login.id) {
                  return (
                    <li key={login.id} className="term-row" data-login-id={login.id}>
                      <LoginForm
                        login={login}
                        all={list}
                        working={working}
                        onCancel={() => { setEditing(null); focusRow(login.id, 'term-person-edit') }}
                        onSave={async (input) => { if (await save(input)) { setEditing(null); focusRow(login.id, 'term-person-edit') } }}
                      />
                    </li>
                  )
                }
                return (
                  <li key={login.id} className="term-row term-person" data-login-id={login.id}>
                    <div className="term-person-main">
                      <span className="term-head"><strong>{login.label}</strong></span>
                      <span className="term-meaning">{siteHost(login.origin)}{login.loginPath ?? ''} · {login.username}</span>
                    </div>
                    <div className="term-person-actions">
                      <button className="secondary-button term-person-edit" type="button" disabled={working} onClick={() => { setEditing(login.id); setAdding(false); setMessage(null) }}>{words.edit}</button>
                      <button className="secondary-button term-person-delete" type="button" disabled={working} onClick={() => setAsking(login)}>{words.remove}</button>
                    </div>
                  </li>
                )
              })}
            </ul>
          )}
        </>
      )}
      {asking && (
        <QuestionDialog
          title={words.removeTitle(asking.label)}
          text={words.removeText}
          working={working}
          actions={[
            { label: words.cancel, onSelect: stopAsking, autoFocus: true },
            { label: words.remove, primary: true, onSelect: () => { const target = asking; setAsking(null); void remove(target) } },
          ]}
          onDismiss={stopAsking}
        />
      )}
    </div>
  )
}

/**
 * 넣기(login 없음)와 고치기. 고칠 때 사이트 주소는 바꿀 수 없어 잠그고, 비밀번호 칸을 비워 두면 기존 것을 그대로 쓴다.
 * 로그인 화면 경로: 같은 사이트에 다른 로그인이 있거나 이미 경로를 정해 둔 로그인일 때만. 기본 계정: 그 경로에 다른 로그인이 있거나 이미 기본일 때만.
 */
function LoginForm({ login, all, working, onCancel, onSave }: {
  login?: SavedLogin
  all: SavedLogin[]
  working: boolean
  onCancel: () => void
  onSave: (input: SavedLoginInput) => void
}) {
  const words = W.savedLogins
  const [site, setSite] = useState(login?.origin ?? '')
  const [label, setLabel] = useState(login?.label ?? '')
  const [username, setUsername] = useState(login?.username ?? '')
  const [ask, setAsk] = useState(login?.askBeforeUse ?? false)
  const [path, setPath] = useState(login?.loginPath ?? '')
  const [isDefault, setIsDefault] = useState(login?.defaultForPath ?? false)
  // 비밀번호는 칸에만 있다. 채웠는지만 센다.
  const password = useRef<HTMLInputElement>(null)
  const [hasPassword, setHasPassword] = useState(false)

  const origin = login ? login.origin : siteOrigin(site)
  const sameSite = all.filter((entry) => entry.id !== login?.id && entry.origin === origin)
  const showPath = sameSite.length > 0 || Boolean(login?.loginPath)
  const samePath = sameSite.filter((entry) => (entry.loginPath ?? '') === (showPath ? path.trim() : ''))
  const showDefault = samePath.length > 0 || Boolean(login?.defaultForPath)
  const ready = Boolean(origin && label.trim() && username.trim() && (login || hasPassword))

  function submit() {
    if (!ready || working) return
    const field = password.current
    const secret = field?.value ?? ''
    // 보낼 값만 꺼내고 칸을 바로 비운다(저장에 실패해도 다시 적게 한다).
    if (field) field.value = ''
    setHasPassword(false)
    const input: SavedLoginInput = {
      ...(login ? { id: login.id } : {}),
      origin,
      label: label.trim(),
      username: username.trim(),
      ...(secret ? { password: secret } : {}),
      askBeforeUse: ask,
      // 경로 칸이 보일 때만 보낸다(빈 값은 경로 지우기). 안 보이면 빼서 지금 값을 그대로 둔다.
      ...(showPath ? { loginPath: path.trim() } : {}),
      // 기본 계정은 늘 지금 값을 보낸다(다른 기본이 있을 때 true 면 뒤쪽이 그 기본을 해제한다).
      defaultForPath: showDefault ? isDefault : false,
    }
    onSave(input)
  }

  return (
    <form
      className="term-form"
      autoComplete="off"
      onKeyDown={(event) => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onCancel() } }}
      onSubmit={(event) => { event.preventDefault(); submit() }}
    >
      <label className="setting-field">
        <span>{words.fieldSite}</span>
        <input value={site} onChange={(event) => setSite(event.target.value)} placeholder="https://" spellCheck={false} disabled={Boolean(login)} autoFocus={!login} />
        {login && <small>{words.siteFixed}</small>}
      </label>
      <label className="setting-field">
        <span>{words.fieldName}</span>
        <input value={label} onChange={(event) => setLabel(event.target.value)} maxLength={100} autoFocus={Boolean(login)} />
      </label>
      <label className="setting-field">
        <span>{words.fieldUsername}</span>
        <input value={username} onChange={(event) => setUsername(event.target.value)} maxLength={256} spellCheck={false} autoComplete="off" />
      </label>
      <label className="setting-field">
        <span>{words.fieldPassword}</span>
        <input
          ref={password}
          type="password"
          maxLength={1024}
          spellCheck={false}
          autoComplete="off"
          placeholder={login ? words.keepPassword : undefined}
          onInput={(event) => setHasPassword(Boolean(event.currentTarget.value))}
        />
      </label>
      {showPath && (
        <label className="setting-field">
          <span>{words.fieldLoginPath}</span>
          <input value={path} onChange={(event) => setPath(event.target.value)} placeholder="/" maxLength={2048} spellCheck={false} />
          <small>{words.loginPathHint}</small>
        </label>
      )}
      {/* 스위치도 다른 칸과 같은 줄 꼴(왼쪽 이름, 오른쪽 값). 켜고 끄는 꼴은 스킬·용어 사전의 스위치. */}
      <div className="setting-field">
        <span>{words.askBeforeUse}</span>
        <button className={ask ? 'skills-switch is-on' : 'skills-switch'} type="button" role="switch" aria-checked={ask} aria-label={words.askBeforeUse} onClick={() => setAsk(!ask)} />
      </div>
      {showDefault && (
        <div className="setting-field">
          <span>{words.defaultForPath}</span>
          <button className={isDefault ? 'skills-switch is-on' : 'skills-switch'} type="button" role="switch" aria-checked={isDefault} aria-label={words.defaultForPath} onClick={() => setIsDefault(!isDefault)} />
          <small>{words.defaultHint}</small>
        </div>
      )}
      <div className="term-form-actions">
        <button className="secondary-button" type="button" onClick={onCancel}>{words.cancel}</button>
        <button className="confirm-button" type="submit" disabled={working || !ready}>{words.save}</button>
      </div>
    </form>
  )
}
