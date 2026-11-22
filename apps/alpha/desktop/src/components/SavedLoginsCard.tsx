import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { ArrowDownUp, Check, ChevronDown, Copy, Eye, EyeOff, Globe, KeyRound, ListFilter, LoaderCircle, Plus, Search, Star, Trash2 } from 'lucide-react'
import { copySavedLoginPassword, revealSavedLogin, savedLoginErrorCode, siteHost, type SavedLogin, type SavedLoginInput } from '../saved-logins'
import { W } from '../words'
import { MenuItem, RowMenu } from './ChatSidebar'
import { useLoginDraft, useSavedLogins } from './SavedLoginsSettings'
import { QuestionDialog } from './ZetPanel'

type Mode = { kind: 'view' } | { kind: 'add' } | { kind: 'edit'; id: string }
type Scope = 'site' | 'all'
type Sort = 'created' | 'updated' | 'name' | 'site'
type Filter = 'all' | 'favorite' | 'ask'
type Group = { key: string; label: string | null; entries: SavedLogin[] }

/**
 * 아이콘 칸: 사이트 그림(S24 iconDataUrl, 로컬 data: PNG)이 있으면 그것, 없거나 그리지 못하면 지구본(사이트) 그림.
 * 이름 앞 두 글자는 뜻 없는 글자로 보여(2026-11-21 확장의 "MI" 를 본 그의 말) 쓰지 않는다.
 */
function SiteIcon({ src, className }: { src?: string | null; className: string }) {
  const [broken, setBroken] = useState(false)
  useEffect(() => { setBroken(false) }, [src])
  return (
    <span className={className} aria-hidden>
      {src && !broken ? <img src={src} alt="" onError={() => setBroken(true)} /> : <Globe size={className.includes('is-large') ? 22 : 16} strokeWidth={1.6} />}
    </span>
  )
}

const DAY = 86_400_000
function startOfDay(date: Date) {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime()
}

/** 저장한 때·고친 때의 묶음: 오늘 · 어제 · 이번 주(월요일부터) · 이번 달 · 그 전 달마다 · 날짜 모름(옛 항목). order 가 작을수록 위. */
function dateBucket(at: string | null, now: Date): { key: string; label: string; order: number } {
  const words = W.savedLogins
  const date = at ? new Date(at) : null
  if (!date || Number.isNaN(date.getTime())) return { key: 'unknown', label: words.dateGroup.unknown, order: Number.MAX_SAFE_INTEGER }
  const today = startOfDay(now)
  const days = Math.round((today - startOfDay(date)) / DAY)
  if (days <= 0) return { key: 'today', label: words.dateGroup.today, order: 0 }
  if (days === 1) return { key: 'yesterday', label: words.dateGroup.yesterday, order: 1 }
  const weekStart = today - ((now.getDay() + 6) % 7) * DAY
  if (startOfDay(date) >= weekStart) return { key: 'week', label: words.dateGroup.week, order: 2 }
  if (date.getFullYear() === now.getFullYear() && date.getMonth() === now.getMonth()) return { key: 'month', label: words.dateGroup.month, order: 3 }
  const monthsAgo = (now.getFullYear() - date.getFullYear()) * 12 + (now.getMonth() - date.getMonth())
  return { key: `m-${date.getFullYear()}-${date.getMonth() + 1}`, label: words.dateGroupMonth(date.getFullYear(), date.getMonth() + 1), order: 3 + monthsAgo }
}

/** 사이트 주소 판: 로그인 화면 주소(siteUrl)가 있으면 그것, 없으면 사이트 + 로그인 화면 경로. https:// 와 경로는 흐리게, 도메인은 진하게. */
function siteParts(login: SavedLogin) {
  try {
    const url = new URL(login.siteUrl ?? login.origin)
    const rest = login.siteUrl ? `${url.pathname === '/' ? '' : url.pathname}${url.search}` : login.loginPath ?? ''
    return { host: url.host, rest }
  } catch {
    return { host: siteHost(login.origin), rest: login.loginPath ?? '' }
  }
}

/**
 * 오른쪽 패널 브라우저의 열쇠 카드 안쪽(2026-11-21 본부가 어사이드 열쇠 창 Aside Vault 를 풀어 보고 적어 준 꼴, S14 뒤쪽으로 빠졌던 것을 채움).
 * 좌우 두 칸: 왼쪽 목록(찾기 칸과 + 단추, 고르기 줄과 거르기·순서 그림 단추, 저장한 때로 묶은 제목, 아이콘·이름·아이디 줄, 이 칸만 굴림),
 * 오른쪽 흰 판(위 줄 왼쪽 작은 글·오른쪽 …, 가운데 큰 아이콘·이름·사이트, "로그인 정보" 판의 아이디·비밀번호(줄에 올리면 보기·복사), "사이트 주소" 판).
 * … 메뉴: 열고 채우기, 수정, 즐겨찾기, 삭제(어사이드의 Open and Fill · Edit · Favorite · Delete. QR 은 뒤쪽에 없음).
 * + 나 … › 수정이면 오른쪽 판이 그대로 입력 꼴이 된다(위 줄 오른쪽 취소·저장, 큰 아이콘 아래 이름 칸, 판의 줄이 입력 칸).
 *
 * 어사이드와 다른 곳: 사이트 그림 대신 이름 앞 두 글자(계약에 그림 없음), "All accounts"(보관함) 대신 이 사이트·모든 로그인 고르기,
 * 비밀번호 만들기 없음, + 의 종류 메뉴 없음(로그인만: + 는 바로 새 로그인). 몸소에만 있는 것: 로그인 화면 경로·쓸 때마다 묻기·기본 계정("사용 설정" 판).
 * 비밀번호: 입력 칸에만 있다가 보낼 때 비운다. 보기로 받은 글은 그 줄에만 두고, 숨기기·다른 로그인 고르기·카드 닫기·창을 떠나면 버린다.
 */
export function SavedLoginsCard({ site, pageUrl = null, onOpenAndFill }: {
  /** 지금 페이지의 사이트(https://호스트[:포트]). 없으면 null. */
  site: string | null
  /** 지금 페이지의 주소(https). 새 로그인의 사이트 주소 칸에 채워 둔다(경로까지면 로그인 화면 주소로 저장된다). */
  pageUrl?: string | null
  /** 열고 채우기(이 채팅방 패널). 돌려준 status 가 filled 면 부르는 쪽이 카드를 닫고 알린다. 없으면 메뉴에 두지 않는다. */
  onOpenAndFill?: (login: SavedLogin) => Promise<string>
}) {
  const words = W.savedLogins
  const { list, problem, working, message, setMessage, reload, save, remove } = useSavedLogins()
  const [mode, setMode] = useState<Mode>({ kind: 'view' })
  const [chosen, setChosen] = useState<string | null>(null)
  const [scope, setScope] = useState<Scope | null>(null)
  const [sort, setSort] = useState<Sort>('created')
  const [filter, setFilter] = useState<Filter>('all')
  const [query, setQuery] = useState('')
  const [asking, setAsking] = useState<SavedLogin | null>(null)
  const [filling, setFilling] = useState(false)
  // 저장한 뒤 고를 로그인(목록을 다시 읽은 뒤 사이트·이름으로 찾는다).
  const [pending, setPending] = useState<{ origin: string; label: string } | null>(null)
  const listBox = useRef<HTMLDivElement>(null)

  const siteLogins = useMemo(() => (list && site ? list.filter((entry) => entry.origin === site) : []), [list, site])
  // 처음 읽었을 때(그리고 사이트가 바뀔 때): 이 사이트 로그인이 있으면 이 사이트, 없으면 모든 로그인.
  useEffect(() => { if (list && scope === null) setScope(siteLogins.length > 0 ? 'site' : 'all') }, [list, scope, siteLogins.length])
  const lastSite = useRef(site)
  useEffect(() => {
    if (lastSite.current === site) return
    lastSite.current = site
    setMessage(null)
    setScope(null)
  }, [site, setMessage])

  const searching = query.trim().length > 0
  const visible = useMemo(() => {
    if (!list) return []
    let rows = list
    if (searching) {
      const needle = query.trim().toLowerCase()
      rows = list.filter((entry) => [entry.label, entry.username, siteHost(entry.origin), entry.siteUrl ?? ''].some((text) => text.toLowerCase().includes(needle)))
    } else if (scope === 'site' && site) {
      rows = siteLogins
    }
    if (filter === 'favorite') rows = rows.filter((entry) => entry.favorite)
    if (filter === 'ask') rows = rows.filter((entry) => entry.askBeforeUse)
    return rows
  }, [list, searching, query, scope, site, siteLogins, filter])

  // 묶음: 저장한 때·고친 때(어사이드의 "This Week" 꼴), 이름(묶지 않음), 사이트(지금 사이트 먼저).
  const groups = useMemo<Group[]>(() => {
    if (sort === 'name') {
      return visible.length ? [{ key: 'all', label: null, entries: [...visible].sort((a, b) => a.label.localeCompare(b.label, 'ko')) }] : []
    }
    if (sort === 'site') {
      const map = new Map<string, SavedLogin[]>()
      for (const entry of visible) map.set(entry.origin, [...(map.get(entry.origin) ?? []), entry])
      return [...map.entries()]
        .sort(([a], [b]) => (a === site ? -1 : b === site ? 1 : siteHost(a).localeCompare(siteHost(b))))
        .map(([origin, entries]) => ({ key: origin, label: siteHost(origin), entries }))
    }
    const now = new Date()
    const field = sort === 'updated' ? 'updatedAt' : 'createdAt'
    const map = new Map<string, { label: string; order: number; entries: SavedLogin[] }>()
    for (const entry of visible) {
      const bucket = dateBucket(entry[field] ?? (field === 'updatedAt' ? entry.createdAt : null), now)
      const group = map.get(bucket.key) ?? { label: bucket.label, order: bucket.order, entries: [] }
      group.entries.push(entry)
      map.set(bucket.key, group)
    }
    const time = (entry: SavedLogin) => Date.parse(entry[field] ?? '') || 0
    return [...map.entries()]
      .sort(([, a], [, b]) => a.order - b.order)
      .map(([key, group]) => ({ key, label: group.label, entries: group.entries.sort((a, b) => time(b) - time(a)) }))
  }, [visible, sort, site])
  const ordered = groups.flatMap((group) => group.entries)

  useEffect(() => {
    if (!list || !pending) return
    const found = list.find((entry) => entry.origin === pending.origin && entry.label === pending.label)
    setPending(null)
    if (found) {
      setChosen(found.id)
      setQuery('')
      setFilter('all')
      setScope(site && found.origin === site ? 'site' : 'all')
    }
  }, [list, pending, site])
  // 고른 것이 보이는 목록에 없으면 첫 줄.
  const current = (chosen && ordered.find((entry) => entry.id === chosen)) || ordered[0] || null
  const editing = mode.kind === 'edit' ? list?.find((entry) => entry.id === mode.id) ?? null : null
  useEffect(() => { if (mode.kind === 'edit' && list && !editing) setMode({ kind: 'view' }) }, [mode, list, editing])

  function focusRow(id: string) {
    requestAnimationFrame(() => listBox.current?.querySelector<HTMLElement>(`[data-login-id="${CSS.escape(id)}"]`)?.focus())
  }
  function leaveForm() {
    setMode({ kind: 'view' })
    if (current) focusRow(current.id)
  }
  async function submit(input: SavedLoginInput) {
    if (!(await save(input))) return
    setPending({ origin: input.origin, label: input.label })
    setMode({ kind: 'view' })
  }
  async function copyUsername(login: SavedLogin) {
    try {
      await navigator.clipboard.writeText(login.username)
      setMessage({ text: words.copiedUsername, alert: false })
    } catch {
      setMessage({ text: words.copyUsernameFailed, alert: true })
    }
  }
  async function toggleFavorite(login: SavedLogin) {
    if (await save({ id: login.id, origin: login.origin, label: login.label, username: login.username, favorite: !login.favorite })) {
      setMessage({ text: login.favorite ? words.unfavorited : words.favorited, alert: false })
    }
  }
  async function openFill(login: SavedLogin) {
    if (!onOpenAndFill || filling) return
    setMessage(null)
    setFilling(true)
    try {
      const status = await onOpenAndFill(login)
      if (status !== 'filled') setMessage({ text: words.openFillStatus[status] ?? words.openFillFailed, alert: true })
    } catch (error) {
      const code = savedLoginErrorCode(error)
      setMessage({ text: (code && (words.openFillStatus[code] ?? words.errors[code])) || words.openFillFailed, alert: true })
    } finally {
      setFilling(false)
    }
  }

  const scopeName = scope === 'site' && site ? words.scopeSite : words.scopeAll
  const note = message && <p className={message.alert ? 'login-vault-note is-alert' : 'login-vault-note'} role={message.alert ? 'alert' : 'status'}>{message.text}</p>
  const choice = (label: string, on: boolean, pick: () => void, close: () => void) => (
    <button key={label} className="chat-menu-item login-vault-choice" type="button" role="menuitemradio" aria-checked={on} onClick={() => { close(); pick() }}>
      <span>{label}</span>{on && <Check size={13} aria-hidden />}
    </button>
  )

  let pane
  if (problem) {
    pane = (
      <div className="login-vault-pane login-vault-empty" role="alert">
        <p>{problem}</p>
        <button className="secondary-button" type="button" onClick={() => void reload()}>{W.firstRun.retry}</button>
      </div>
    )
  } else if (!list) {
    pane = <div className="login-vault-pane login-vault-empty" role="status"><LoaderCircle size={16} className="is-spinning" aria-hidden /><p>{W.firstRun.checking}</p></div>
  } else if (mode.kind === 'add' || (mode.kind === 'edit' && editing)) {
    pane = (
      <LoginPaneForm
        key={mode.kind === 'edit' ? mode.id : 'add'}
        login={editing ?? undefined}
        site={mode.kind === 'add' ? pageUrl ?? site ?? undefined : undefined}
        all={list}
        working={working}
        note={note}
        onCancel={() => { setMessage(null); leaveForm() }}
        onSave={(input) => void submit(input)}
      />
    )
  } else if (current) {
    const login = current
    const parts = siteParts(login)
    pane = (
      <div className="login-vault-pane">
        <div className="login-vault-top">
          <span className="login-vault-crumb">{words.title} › {words.onThisComputer}</span>
          <RowMenu label={words.more} buttonClassName="icon-button login-vault-more">
            {(close) => (
              <>
                {onOpenAndFill && <MenuItem close={close} onSelect={() => void openFill(login)}>{words.openFill}</MenuItem>}
                <MenuItem close={close} onSelect={() => { setMessage(null); setMode({ kind: 'edit', id: login.id }) }}>{words.edit}</MenuItem>
                <MenuItem close={close} onSelect={() => void toggleFavorite(login)}>{login.favorite ? words.unfavorite : words.favorite}</MenuItem>
                <button className="chat-menu-item login-vault-delete" type="button" role="menuitem" onClick={() => { close(); setAsking(login) }}>
                  <Trash2 size={14} aria-hidden />{words.remove}
                </button>
              </>
            )}
          </RowMenu>
        </div>
        {filling ? <p className="login-vault-note" role="status"><LoaderCircle size={12} className="is-spinning" aria-hidden /> {words.openFill}</p> : note}
        <div className="login-vault-hero">
          <SiteIcon className="login-vault-icon is-large" src={login.iconDataUrl} />
          <h3>{login.label}{login.favorite && <Star size={14} className="login-vault-star" fill="currentColor" aria-label={words.favorite} />}</h3>
          <p><bdi>{siteHost(login.origin)}</bdi></p>
        </div>
        <p className="login-vault-caption">{words.credentials}</p>
        <dl className="login-vault-box">
          <div className="login-vault-line">
            <dt>{words.fieldUsername}</dt>
            <dd>
              <strong>{login.username}</strong>
              <span className="login-vault-line-actions">
                <button className="icon-button" type="button" aria-label={words.copyUsername} title={words.copyUsername} onClick={() => void copyUsername(login)}>
                  <Copy size={14} aria-hidden />
                </button>
              </span>
            </dd>
          </div>
          <PasswordLine key={login.id} login={login} onNote={(text, alert) => setMessage({ text, alert })} />
        </dl>
        <p className="login-vault-caption">{words.fieldSite}</p>
        <div className="login-vault-box">
          <div className="login-vault-line">
            <span className="login-vault-url"><bdi><span>https://</span><b>{parts.host}</b><span>{parts.rest}</span></bdi></span>
          </div>
        </div>
        {(login.askBeforeUse || login.defaultForPath) && (
          <>
            <p className="login-vault-caption">{words.usage}</p>
            <div className="login-vault-box">
              {login.defaultForPath && <div className="login-vault-line"><span className="login-vault-flag">{words.defaultForPath}</span></div>}
              {login.askBeforeUse && <div className="login-vault-line"><span className="login-vault-flag">{words.askBeforeUse}</span></div>}
            </div>
          </>
        )}
      </div>
    )
  } else {
    pane = (
      <div className="login-vault-pane login-vault-empty">
        {note}
        <span className="login-vault-mark" aria-hidden><KeyRound size={26} /></span>
        <p>{searching || filter !== 'all' ? words.noMatch : words.empty}</p>
      </div>
    )
  }

  return (
    <div className="login-vault">
      <div className="login-vault-side">
        <div className="login-vault-find">
          <label className="login-vault-search">
            <Search size={14} aria-hidden />
            <input value={query} placeholder={words.searchAll} aria-label={words.searchAll} spellCheck={false} onChange={(event) => setQuery(event.target.value)} />
          </label>
          <button
            className="login-vault-add"
            type="button"
            aria-label={words.add}
            title={words.add}
            disabled={!list || mode.kind === 'add'}
            onClick={() => { setMessage(null); setMode({ kind: 'add' }) }}
          >
            <Plus size={16} aria-hidden />
          </button>
        </div>
        <div className="login-vault-scope">
          {site && !searching ? (
            <RowMenu
              label={scopeName}
              buttonClassName="login-vault-scope-button"
              button={<><KeyRound size={14} aria-hidden /><span>{scopeName}</span><ChevronDown size={13} aria-hidden /></>}
            >
              {(close) => (
                <>
                  <MenuItem close={close} onSelect={() => setScope('site')}>{words.scopeSite} · {siteHost(site)}</MenuItem>
                  <MenuItem close={close} onSelect={() => setScope('all')}>{words.scopeAll}</MenuItem>
                </>
              )}
            </RowMenu>
          ) : (
            <span className="login-vault-scope-button is-static"><KeyRound size={14} aria-hidden /><span>{words.scopeAll}</span></span>
          )}
          <span className="login-vault-tools">
            <RowMenu label={words.filter} buttonClassName={filter === 'all' ? 'icon-button login-vault-tool' : 'icon-button login-vault-tool is-on'} button={<ListFilter size={15} aria-hidden />}>
              {(close) => [
                choice(words.filterAll, filter === 'all', () => setFilter('all'), close),
                choice(words.favorite, filter === 'favorite', () => setFilter('favorite'), close),
                choice(words.askBeforeUse, filter === 'ask', () => setFilter('ask'), close),
              ]}
            </RowMenu>
            <RowMenu label={words.sort} buttonClassName="icon-button login-vault-tool" button={<ArrowDownUp size={15} aria-hidden />}>
              {(close) => [
                choice(words.sortCreated, sort === 'created', () => setSort('created'), close),
                choice(words.sortUpdated, sort === 'updated', () => setSort('updated'), close),
                choice(words.sortName, sort === 'name', () => setSort('name'), close),
                choice(words.sortSite, sort === 'site', () => setSort('site'), close),
              ]}
            </RowMenu>
          </span>
        </div>
        <div ref={listBox} className="login-vault-rows">
          {groups.map((group) => (
            <section key={group.key} aria-label={group.label ?? undefined}>
              {group.label && <p className="login-vault-group"><bdi>{group.label}</bdi></p>}
              {group.entries.map((login) => {
                const on = current?.id === login.id && mode.kind !== 'add'
                return (
                  <button
                    key={login.id}
                    className={on ? 'login-vault-row is-chosen' : 'login-vault-row'}
                    type="button"
                    data-login-id={login.id}
                    aria-current={on ? 'true' : undefined}
                    onClick={() => { setChosen(login.id); setMessage(null); if (mode.kind !== 'view') setMode({ kind: 'view' }) }}
                  >
                    <SiteIcon className="login-vault-icon" src={login.iconDataUrl} />
                    <span className="login-vault-row-text">
                      <strong>{login.label}{login.favorite && <Star size={11} className="login-vault-star" fill="currentColor" aria-label={words.favorite} />}</strong>
                      <span>{login.username}</span>
                    </span>
                  </button>
                )
              })}
            </section>
          ))}
          {/* 없을 때·찾는 것이 없을 때의 한 줄은 오른쪽 판에만 둔다(같은 말을 두 번 두지 않는다). */}
        </div>
      </div>
      {pane}
      {asking && (
        <QuestionDialog
          title={words.removeTitle(asking.label)}
          text={words.removeText}
          working={working}
          actions={[
            { label: words.cancel, onSelect: () => setAsking(null), autoFocus: true },
            {
              label: words.remove,
              primary: true,
              onSelect: () => {
                const target = asking
                setAsking(null)
                void remove(target).then((gone) => { if (gone) setChosen(null) })
              },
            },
          ]}
          onDismiss={() => setAsking(null)}
        />
      )}
    </div>
  )
}

/**
 * 비밀번호 줄: 평소엔 점, 줄에 올리면 보기(눈)·복사(어사이드 꼴). 보기는 saved_login_reveal 로 받은 글을 이 줄에만 두고, 숨기기·창을 떠남(blur·숨김)에 버린다.
 * 부르는 쪽이 로그인마다 key 를 달아, 다른 로그인을 고르거나 카드가 닫히면 이 줄이 사라지며 글도 버려진다. 복사는 뒤쪽이 클립보드에 직접 쓴다.
 */
function PasswordLine({ login, onNote }: { login: SavedLogin; onNote: (text: string, alert: boolean) => void }) {
  const words = W.savedLogins
  const [shown, setShown] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (shown === null) return
    const hide = () => setShown(null)
    const away = () => { if (document.hidden) hide() }
    window.addEventListener('blur', hide)
    document.addEventListener('visibilitychange', away)
    return () => {
      window.removeEventListener('blur', hide)
      document.removeEventListener('visibilitychange', away)
    }
  }, [shown])

  async function toggle() {
    if (shown !== null) { setShown(null); return }
    setBusy(true)
    try {
      setShown(await revealSavedLogin(login.id))
    } catch (error) {
      const code = savedLoginErrorCode(error)
      onNote((code && W.savedLogins.errors[code]) || words.revealFailed, true)
    } finally {
      setBusy(false)
    }
  }
  async function copy() {
    try {
      await copySavedLoginPassword(login.id)
      onNote(words.copiedPassword, false)
    } catch {
      onNote(words.copyPasswordFailed, true)
    }
  }

  return (
    <div className={shown !== null ? 'login-vault-line is-revealed' : 'login-vault-line'}>
      <dt>{words.fieldPassword}</dt>
      <dd>
        {shown !== null ? <span className="login-vault-revealed">{shown}</span> : <span className="login-vault-secret" aria-hidden>••••••••••</span>}
        <span className="login-vault-line-actions">
          <button className="icon-button" type="button" aria-label={shown !== null ? words.hidePassword : words.showPassword} title={shown !== null ? words.hidePassword : words.showPassword} aria-pressed={shown !== null} disabled={busy} onClick={() => void toggle()}>
            {shown !== null ? <EyeOff size={14} aria-hidden /> : <Eye size={14} aria-hidden />}
          </button>
          <button className="icon-button" type="button" aria-label={words.copyPassword} title={words.copyPassword} onClick={() => void copy()}>
            <Copy size={14} aria-hidden />
          </button>
        </span>
      </dd>
    </div>
  )
}

/** 오른쪽 판의 입력 꼴(새 로그인·수정). 보기와 같은 자리에 같은 판으로, 줄이 입력 칸이 된다. 규칙은 설정 칸과 같다(useLoginDraft). */
function LoginPaneForm({ login, site: startSite, all, working, note, onCancel, onSave }: {
  login?: SavedLogin
  site?: string
  all: SavedLogin[]
  working: boolean
  note: ReactNode
  onCancel: () => void
  onSave: (input: SavedLoginInput) => void
}) {
  const words = W.savedLogins
  const draft = useLoginDraft({ login, site: startSite, all })
  const { site, setSite, label, setLabel, username, setUsername, ask, setAsk, path, setPath, isDefault, setIsDefault, password, setHasPassword, showPath, showDefault, ready } = draft

  return (
    <form
      className="login-vault-pane is-form"
      autoComplete="off"
      onKeyDown={(event) => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onCancel() } }}
      onSubmit={(event) => {
        event.preventDefault()
        if (!ready || working) return
        const input = draft.take()
        if (input) onSave(input)
      }}
    >
      <div className="login-vault-top">
        <span className="login-vault-crumb">{words.title} › {words.onThisComputer}</span>
        <span className="login-vault-form-actions">
          <button className="login-vault-cancel" type="button" onClick={onCancel}>{words.cancel}</button>
          <button className="login-vault-save" type="submit" disabled={working || !ready}>{words.save}</button>
        </span>
      </div>
      {note}
      <div className="login-vault-hero">
        <SiteIcon className="login-vault-icon is-large is-plain" />
        <input className="login-vault-name" value={label} placeholder={words.add} aria-label={words.fieldName} maxLength={100} autoFocus onChange={(event) => setLabel(event.target.value)} />
      </div>
      <p className="login-vault-caption">{words.credentials}</p>
      <div className="login-vault-box">
        <label className="login-vault-line">
          <span>{words.fieldUsername}</span>
          <input value={username} placeholder={words.usernameExample} maxLength={256} spellCheck={false} autoComplete="off" onChange={(event) => setUsername(event.target.value)} />
        </label>
        <label className="login-vault-line">
          <span>{words.fieldPassword}</span>
          <input
            ref={password}
            type="password"
            maxLength={1024}
            spellCheck={false}
            autoComplete="off"
            placeholder={login ? words.keepPassword : words.fieldPassword}
            onInput={(event) => setHasPassword(Boolean(event.currentTarget.value))}
          />
        </label>
      </div>
      <p className="login-vault-caption">{words.fieldSite}</p>
      <div className="login-vault-box">
        <label className="login-vault-line">
          <input value={site} placeholder={words.siteExample} aria-label={words.fieldSite} spellCheck={false} disabled={Boolean(login)} onChange={(event) => setSite(event.target.value)} />
          {login && <small>{words.siteFixed}</small>}
        </label>
      </div>
      <p className="login-vault-caption">{words.usage}</p>
      <div className="login-vault-box">
        {showPath && (
          <label className="login-vault-line">
            <span>{words.fieldLoginPath}</span>
            <input value={path} placeholder="/" maxLength={2048} spellCheck={false} onChange={(event) => setPath(event.target.value)} />
            <small>{words.loginPathHint}</small>
          </label>
        )}
        <div className="login-vault-line is-switch">
          <span>{words.askBeforeUse}</span>
          <button className={ask ? 'skills-switch is-on' : 'skills-switch'} type="button" role="switch" aria-checked={ask} aria-label={words.askBeforeUse} onClick={() => setAsk(!ask)} />
        </div>
        {showDefault && (
          <div className="login-vault-line is-switch">
            <span>{words.defaultForPath}</span>
            <button className={isDefault ? 'skills-switch is-on' : 'skills-switch'} type="button" role="switch" aria-checked={isDefault} aria-label={words.defaultForPath} onClick={() => setIsDefault(!isDefault)} />
            <small>{words.defaultHint}</small>
          </div>
        )}
      </div>
    </form>
  )
}
