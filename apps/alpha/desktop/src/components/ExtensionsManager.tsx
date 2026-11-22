import { useCallback, useEffect, useState } from 'react'
import { FolderOpen, LoaderCircle, Pin, PinOff, Puzzle, Trash2 } from 'lucide-react'
import {
  extensionErrorCode, importCandidates, importExtension, installExtensionFolder, installExtensionFromStore, listenExtensionsChanged,
  listExtensions, pinnedExtensions, removeExtension, setExtensionEnabled, setExtensionPinned, type ExtensionCandidate, type ExtensionsSection, type PanelExtension,
} from '../panel-extensions'
import { W } from '../words'
import { ExtensionIcon } from './ExtensionsMenu'
import { QuestionDialog } from './ZetPanel'
import './Extensions.css'

/**
 * 확장 프로그램 관리 화면(어사이드 aside-4: 탭 하나를 쓰는 관리 화면). 몸소에서는 그 채팅방 패널 브라우저의 자리를 이 화면이 쓰고(페이지는 잠시 숨김),
 * 주소 줄에 "확장 프로그램", 뒤로를 누르면 페이지로 돌아간다.
 * 위: 크롬 웹 스토어 주소·ID 로 설치, 폴더에서 설치, 크롬·엣지에서 가져오기(펼침). 권한 안내 한 줄. 아래: 설치된 확장마다 카드(글자 그림·이름·ID·고정·삭제·사용 스위치).
 */
export function ExtensionsManager({ section, onPinsChanged }: { section: ExtensionsSection; onPinsChanged?: () => void }) {
  const words = W.extensions
  const [list, setList] = useState<PanelExtension[] | null>(null)
  const [pins, setPins] = useState<string[]>([])
  const [store, setStore] = useState('')
  const [working, setWorking] = useState<string | null>(null)
  const [message, setMessage] = useState<{ text: string; alert: boolean } | null>(null)
  const [importing, setImporting] = useState(section === 'import')
  const [candidates, setCandidates] = useState<ExtensionCandidate[] | null | 'failed'>(null)
  const [removing, setRemoving] = useState<PanelExtension | null>(null)

  const reload = useCallback(() => {
    Promise.all([listExtensions(), pinnedExtensions().catch(() => [] as string[])])
      .then(([items, pinned]) => { setList(items); setPins(pinned) })
      .catch((error) => { setList([]); setMessage({ text: errorText(error, words.loadFailed), alert: true }) })
  }, [words])
  useEffect(() => { reload() }, [reload])
  useEffect(() => { setImporting(section === 'import') }, [section])
  useEffect(() => {
    let stop: (() => void) | undefined
    let disposed = false
    void listenExtensionsChanged(reload).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [reload])
  useEffect(() => {
    if (!importing || candidates !== null) return
    importCandidates().then(setCandidates).catch(() => setCandidates('failed'))
  }, [importing, candidates])

  function errorText(error: unknown, plain: string) {
    const code = extensionErrorCode(error)
    return words.errors[code] ?? plain
  }

  /** 한 번에 하나. 끝나면 목록을 다시 읽고 한 줄로 알린다. 그만둠(cancelled)은 글 없이, 시간 넘김(timeout)은 목록을 다시 읽었다고. */
  async function run(key: string, task: () => Promise<unknown>, done: string | null) {
    if (working) return
    setWorking(key)
    setMessage(null)
    try {
      const result = await task()
      if (result !== null && done) setMessage({ text: done, alert: false })
      reload()
      return true
    } catch (error) {
      const code = extensionErrorCode(error)
      if (code !== 'cancelled') setMessage({ text: errorText(error, words.errors.failed), alert: code !== 'timeout' })
      reload()
      return false
    } finally {
      setWorking(null)
    }
  }

  async function pin(item: PanelExtension, pinned: boolean) {
    try {
      await setExtensionPinned(item.id, pinned)
      setPins((now) => (pinned ? [...now.filter((id) => id !== item.id), item.id] : now.filter((id) => id !== item.id)))
      onPinsChanged?.()
    } catch (error) { setMessage({ text: errorText(error, words.errors.failed), alert: true }) }
  }

  const busy = working !== null
  return (
    <section className="ext-page" aria-labelledby="ext-page-title">
      <header className="ext-page-head">
        <h2 id="ext-page-title"><Puzzle size={17} aria-hidden />{words.pageTitle}</h2>
        {list && <span className="ext-page-count">{words.total(list.length)}</span>}
      </header>
      <form className="ext-store" onSubmit={(event) => { event.preventDefault(); const value = store.trim(); if (value) void run('store', () => installExtensionFromStore(value), words.installedDone).then((ok) => { if (ok) setStore('') }) }}>
        <label className="ext-field">
          <span>{words.storeLabel}</span>
          <input value={store} placeholder={words.storeExample} spellCheck={false} disabled={busy} onChange={(event) => setStore(event.target.value)} />
        </label>
        <button className="secondary-button ext-primary" type="submit" disabled={busy || !store.trim()}>
          {working === 'store' ? <LoaderCircle size={13} className="is-spinning" aria-hidden /> : null}{words.storeInstall}
        </button>
      </form>
      <div className="ext-actions">
        <button className="secondary-button" type="button" disabled={busy} onClick={() => void run('folder', installExtensionFolder, words.installedDone)}>
          {working === 'folder' ? <LoaderCircle size={13} className="is-spinning" aria-hidden /> : <FolderOpen size={13} aria-hidden />}{words.folderInstall}
        </button>
        <button className="secondary-button" type="button" aria-expanded={importing} onClick={() => setImporting((now) => !now)}>{words.importTitle}</button>
      </div>
      <p className="ext-note">{words.permissionNote}</p>
      {message && <p className={`ext-message${message.alert ? ' is-alert' : ''}`} role={message.alert ? 'alert' : 'status'}>{message.text}</p>}
      {importing && (
        <section className="ext-import" aria-label={words.importTitle}>
          <h3>{words.importTitle}</h3>
          <p className="ext-note">{words.importText} {words.importIdNote}</p>
          {candidates === null && <p className="ext-note" role="status"><LoaderCircle size={13} className="is-spinning" aria-hidden />{words.loading}</p>}
          {candidates === 'failed' && <p className="ext-message is-alert" role="alert">{words.errors.source_unavailable}</p>}
          {Array.isArray(candidates) && candidates.length === 0 && <p className="ext-note">{words.importNone}</p>}
          {Array.isArray(candidates) && candidates.length > 0 && (
            <ul className="ext-candidates">
              {candidates.map((item) => {
                const key = `${item.browser}|${item.profile}|${item.extensionId}|${item.version}`
                return (
                  <li key={key}>
                    <ExtensionIcon />
                    <span className="ext-candidate-text">
                      <strong>{item.name}</strong>
                      <span>{words.browserName[item.browser] ?? item.browser} · {item.profile} · {item.version}</span>
                    </span>
                    <button className="secondary-button" type="button" disabled={busy} onClick={() => void run(key, () => importExtension(item), words.installedDone)}>
                      {working === key ? <LoaderCircle size={13} className="is-spinning" aria-hidden /> : null}{words.importButton}
                    </button>
                  </li>
                )
              })}
            </ul>
          )}
        </section>
      )}
      <h3 className="ext-list-title">{words.installed}</h3>
      {list === null && <p className="ext-note" role="status"><LoaderCircle size={13} className="is-spinning" aria-hidden />{words.loading}</p>}
      {list?.length === 0 && <p className="ext-note">{words.none}</p>}
      <ul className="ext-cards">
        {list?.map((item) => {
          const pinned = pins.includes(item.id)
          return (
            <li key={item.id} className={`ext-card${item.enabled ? '' : ' is-off'}`}>
              <ExtensionIcon large src={item.iconDataUrl} />
              <div className="ext-card-text">
                <strong title={item.name}>{item.name}</strong>
                <span className="ext-card-id">ID: {item.id}</span>
              </div>
              <div className="ext-card-actions">
                <button className={`icon-button ext-pin${pinned ? ' is-pinned' : ''}`} type="button" aria-pressed={pinned} aria-label={`${pinned ? words.unpin : words.pin}: ${item.name}`} title={pinned ? words.unpin : words.pin} onClick={() => void pin(item, !pinned)}>
                  {pinned ? <PinOff size={14} aria-hidden /> : <Pin size={14} aria-hidden />}
                </button>
                <button className="icon-button" type="button" aria-label={`${words.remove}: ${item.name}`} title={words.remove} disabled={busy} onClick={() => setRemoving(item)}>
                  <Trash2 size={14} aria-hidden />
                </button>
                <button
                  className={`ext-switch${item.enabled ? ' is-on' : ''}`}
                  type="button"
                  role="switch"
                  aria-checked={item.enabled}
                  aria-label={`${item.enabled ? words.disable : words.enable}: ${item.name}`}
                  disabled={busy}
                  onClick={() => void run(`toggle-${item.id}`, () => setExtensionEnabled(item.id, !item.enabled), null)}
                ><span /></button>
              </div>
            </li>
          )
        })}
      </ul>
      {removing && (
        <QuestionDialog
          title={words.removeTitle(removing.name)}
          text={words.removeText}
          working={busy}
          onDismiss={() => setRemoving(null)}
          actions={[
            { label: W.savedLogins.cancel, onSelect: () => setRemoving(null), autoFocus: true },
            { label: words.remove, primary: true, onSelect: () => { const item = removing; setRemoving(null); void run(`remove-${item.id}`, () => removeExtension(item.id), words.removed) } },
          ]}
        />
      )}
    </section>
  )
}
