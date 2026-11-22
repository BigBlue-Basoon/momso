import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { ArrowLeft, Check, FileText, Folder, FolderPlus, LoaderCircle, Pencil } from 'lucide-react'
import { readZetHistory, type ZetHistoryEntry } from '../archive'
import { invoke } from '../host'
import { W } from '../words'
import { MenuItem, RowMenu } from './ChatSidebar'
import { ZetMarkdown } from './ZetMarkdown'
import { QuestionDialog, VersionView, formatDateTimeSeconds } from './ZetPanel'

type DocEntry = { id: string; title: string; folder: string; updatedAt: string }
type DocBody = DocEntry & { summary: string; body: string; revision: string }
type Draft = { title: string; summary: string; body: string }

const day = (value: string) => {
  const at = new Date(value)
  return Number.isNaN(at.getTime()) ? '' : at.toLocaleDateString('ko-KR', { year: 'numeric', month: 'long', day: 'numeric' })
}

/** 데스크톱 문서 화면의 폴더 이름 규칙(ZetPanel validFolderName)과 같다. */
const validFolderName = (name: string) => name.length > 0 && name.trim() === name && !/[\\/]/.test(name) && name !== '.' && name !== '..' && !/[\u0000-\u001f]/.test(name)

const isConflict = (error: unknown) => String((error as { message?: string })?.message ?? error).includes('conflict')
const nameOf = (path: string) => path.split('/').pop() ?? path
const parentOf = (path: string) => path.split('/').slice(0, -1).join('/')

/**
 * 휴대폰 몸소의 문서(줄3 S34 읽기, S75 고치기·폴더). 데스크톱 문서 화면(ZetPanel)은 폴더 나무·옮기기·지난 판·브라우저가 한데 있어,
 * 휴대폰에서는 작은 화면: 맨 위 줄(목록 단추 · "문서" · 새 폴더), 폴더마다 묶은 문서(최근 고친 순, 맨 위 폴더 먼저, 빈 폴더도), 누르면 본문(데스크톱과 같은 ZetMarkdown).
 * 고치기는 데스크톱 문서의 고치기 화면과 같은 꼴(제목·요약·본문 칸, 취소·저장, 그사이 바뀌었으면 묻기). server 의 revision 으로 그사이 바뀐 것을 안다.
 * 폴더: 맨 위 폴더 만들기(데스크톱처럼 "새 폴더" 이름 칸이 열리고 정하면 만든다)와 폴더 이름 바꾸기(데스크톱 줄 메뉴의 "이름 바꾸기"와 같은 칸, S75 둘째).
 * 본문·이전 버전 두 보기(S75 둘째): 데스크톱 문서의 보기 줄에서 "연결된 문서"를 뺀 것. 이전 버전의 판 보기·되돌리기는 데스크톱 VersionView 그대로.
 * 글은 데스크톱 문서 화면의 말 그대로. 데스크톱에 문서를 새로 만드는 화면이 없어 휴대폰에도 두지 않는다.
 */
export function NetworkDocuments({ menu, openId = null, onOpened }: {
  menu: ReactNode
  /** 찾기 창에서 고른 문서(S75 둘째). 열고 나면 onOpened 로 알린다. */
  openId?: string | null
  onOpened?: () => void
}) {
  const words = W.zetPanel
  const [list, setList] = useState<DocEntry[] | null | 'error'>(null)
  const [folders, setFolders] = useState<string[]>([])
  const [open, setOpen] = useState<string | null>(null)
  const [doc, setDoc] = useState<DocBody | null | 'error'>(null)
  const [docSeq, setDocSeq] = useState(0)
  // 새 폴더의 이름 칸(만들기 전). null 이면 닫힘.
  const [naming, setNaming] = useState<string | null>(null)
  // 이름 칸을 이미 닫았는지(Enter 와 칸을 떠남이 둘 다 와도 한 번만 만든다).
  const namingDone = useRef(false)
  const [folderNote, setFolderNote] = useState<string | null>(null)
  const [working, setWorking] = useState(false)
  // 고치는 중이면 고치고 있는 제목·요약·본문. 고치는 중이 아니면 null.
  const [draft, setDraft] = useState<Draft | null>(null)
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [fieldErrors, setFieldErrors] = useState<{ title?: string; summary?: string }>({})
  const [askingDiscard, setAskingDiscard] = useState(false)
  const [changedMeanwhile, setChangedMeanwhile] = useState(false)
  // 본문 / 이전 버전. 이전 버전은 처음 고를 때 읽는다.
  const [view, setView] = useState<'body' | 'history'>('body')
  const [history, setHistory] = useState<ZetHistoryEntry[] | null | 'error'>(null)
  const [version, setVersion] = useState<ZetHistoryEntry | null>(null)
  // 이름을 바꾸는 폴더(전체 경로)와 그 칸의 글.
  const [renaming, setRenaming] = useState<string | null>(null)
  const [renameText, setRenameText] = useState('')
  const renameDone = useRef(false)

  const loadFolders = () => {
    invoke<string[]>('server_document_folders').then(setFolders).catch(() => setFolders([]))
  }
  const loadList = () => {
    setList(null)
    invoke<DocEntry[]>('server_documents_list').then(setList).catch(() => setList('error'))
    loadFolders()
  }
  useEffect(() => { loadList() }, [])
  useEffect(() => {
    if (!openId) return
    setDraft(null)
    setDocSeq(0)
    setOpen(openId)
    onOpened?.()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openId])
  useEffect(() => { setView('body'); setHistory(null); setVersion(null) }, [open])
  const loadHistory = (id: string) => {
    setHistory(null)
    readZetHistory(id).then(setHistory).catch(() => setHistory('error'))
  }
  useEffect(() => {
    if (!open) { setDoc(null); return }
    let alive = true
    if (docSeq === 0) setDoc(null)
    invoke<DocBody>('server_document_read', { id: open }).then((value) => { if (alive) setDoc(value) }).catch(() => { if (alive) setDoc('error') })
    return () => { alive = false }
  }, [open, docSeq])

  const groups = useMemo(() => {
    if (!Array.isArray(list)) return []
    const byFolder = new Map<string, DocEntry[]>()
    for (const entry of list) byFolder.set(entry.folder, [...(byFolder.get(entry.folder) ?? []), entry])
    // 빈 폴더: 문서도 아래 폴더도 없는 폴더만(위 폴더는 아래 폴더의 문서 묶음 이름에 이미 보인다).
    const inside = (folder: string, path: string) => path === folder || path.startsWith(`${folder}/`)
    for (const folder of folders) {
      if (byFolder.has(folder)) continue
      if (list.some((entry) => inside(folder, entry.folder)) || folders.some((other) => other !== folder && other.startsWith(`${folder}/`))) continue
      byFolder.set(folder, [])
    }
    return [...byFolder.entries()]
      .sort(([a], [b]) => (a === '' ? -1 : b === '' ? 1 : a.localeCompare(b, 'ko')))
      .map(([folder, entries]) => ({ folder, entries: [...entries].sort((x, y) => y.updatedAt.localeCompare(x.updatedAt)) }))
  }, [list, folders])

  function startFolder() {
    const taken = new Set(folders.filter((path) => !path.includes('/')))
    let name = words.newFolderName
    for (let n = 2; taken.has(name); n += 1) name = `${words.newFolderName} ${n}`
    setFolderNote(null)
    namingDone.current = false
    setNaming(name)
  }
  function commitFolder(value: string) {
    if (namingDone.current) return
    namingDone.current = true
    const name = value.trim()
    setNaming(null)
    if (!name) return
    if (!validFolderName(name)) { setFolderNote(words.folderBadName); return }
    setWorking(true)
    invoke('server_document_folder_create', { folder: name })
      .then(() => loadFolders())
      .catch((error) => setFolderNote(String((error as { message?: string })?.message ?? error).includes('invalid_folder') ? words.folderBadName : words.folderFailed))
      .finally(() => setWorking(false))
  }

  function startRename(folder: string) {
    renameDone.current = false
    setFolderNote(null)
    setRenameText(nameOf(folder))
    setRenaming(folder)
  }
  function commitRename(folder: string, value: string) {
    if (renameDone.current) return
    renameDone.current = true
    setRenaming(null)
    const name = value.trim()
    if (!name || name === nameOf(folder)) return
    if (!validFolderName(name)) { setFolderNote(words.folderBadName); return }
    const to = parentOf(folder) ? `${parentOf(folder)}/${name}` : name
    setWorking(true)
    invoke('server_document_folder_rename', { from: folder, to })
      .then(() => loadList())
      .catch((error) => setFolderNote(String((error as { message?: string })?.message ?? error).includes('invalid_folder') ? words.folderBadName : words.folderFailed))
      .finally(() => setWorking(false))
  }

  function stopEditing() {
    setDraft(null)
    setSaveError(null)
    setFieldErrors({})
    setAskingDiscard(false)
    setChangedMeanwhile(false)
  }
  const current = doc && doc !== 'error' ? doc : null
  const dirty = draft !== null && current !== null && (draft.title !== current.title || draft.summary !== current.summary || draft.body !== current.body)

  // 저장(PUT /documents/{id}, 읽을 때 받은 revision). overwrite 는 그사이 바뀐 글을 내가 고친 것으로 덮어쓸 때만(사람이 그러라고 한 뒤).
  async function save(overwrite = false) {
    if (!draft || !current || saving) return
    const nextTitle = draft.title.trim()
    const nextSummary = draft.summary.trim()
    const errors = {
      ...(nextTitle ? {} : { title: words.text36 }),
      ...(nextSummary ? {} : { summary: words.text37 }),
    }
    setFieldErrors(errors)
    if (errors.title || errors.summary) return
    if (!dirty && !overwrite) { stopEditing(); return }
    setSaving(true)
    setSaveError(null)
    setChangedMeanwhile(false)
    try {
      const revision = overwrite ? (await invoke<DocBody>('server_document_read', { id: current.id })).revision : current.revision
      // 폴더는 그대로 둔다(server 는 보낸 폴더로 옮긴다).
      await invoke('server_document_save', { id: current.id, title: nextTitle, summary: nextSummary, body: draft.body, folder: current.folder, revision })
    } catch (reason) {
      setSaving(false)
      if (isConflict(reason)) setChangedMeanwhile(true)
      else setSaveError(words.text38('').trim())
      return
    }
    setSaving(false)
    stopEditing()
    setDocSeq((value) => value + 1)
    loadList()
  }

  if (open && draft && current) {
    return (
      <section className="network-docs" aria-label={W.tip.documents}>
        <header className="network-docs-head">
          <button className="icon-button" type="button" aria-label={W.tip.back} title={W.tip.back} disabled={saving} onClick={() => (dirty ? setAskingDiscard(true) : stopEditing())}><ArrowLeft size={18} aria-hidden /></button>
          <h1>{current.title}</h1>
        </header>
        <div className="network-docs-body is-editing">
          <article className="zet-document zet-document--editing">
            <header className="zet-head">
              <input
                className="zet-edit-title"
                value={draft.title}
                aria-label={words.aria6}
                aria-invalid={Boolean(fieldErrors.title)}
                placeholder={words.aria6}
                disabled={saving}
                onChange={(event) => {
                  setDraft({ ...draft, title: event.target.value })
                  if (fieldErrors.title) setFieldErrors({ ...fieldErrors, title: undefined })
                }}
              />
              {fieldErrors.title && <p className="zet-edit-error" role="alert">{fieldErrors.title}</p>}
              <textarea
                className="zet-edit-summary"
                value={draft.summary}
                aria-label={words.text1}
                aria-invalid={Boolean(fieldErrors.summary)}
                placeholder={words.text1}
                rows={2}
                disabled={saving}
                onChange={(event) => {
                  setDraft({ ...draft, summary: event.target.value })
                  if (fieldErrors.summary) setFieldErrors({ ...fieldErrors, summary: undefined })
                }}
              />
              {fieldErrors.summary && <p className="zet-edit-error" role="alert">{fieldErrors.summary}</p>}
            </header>
            <div className="zet-viewbar zet-viewbar--editing">
              <button className="secondary-button" type="button" disabled={saving} onClick={() => (dirty ? setAskingDiscard(true) : stopEditing())}>{words.text40}</button>
              <button className="confirm-button" type="button" disabled={saving} onClick={() => void save()}>
                <Check size={14} aria-hidden />{words.text22}</button>
            </div>
            <div className="zet-view">
              {saveError && <p className="zet-edit-error zet-edit-error--save" role="alert">{saveError}</p>}
              <textarea
                className="zet-edit-body"
                value={draft.body}
                aria-label={words.text30}
                placeholder={words.text30}
                spellCheck={false}
                disabled={saving}
                onChange={(event) => setDraft({ ...draft, body: event.target.value })}
              />
            </div>
          </article>
        </div>
        {changedMeanwhile && (
          <QuestionDialog
            title={words.title2}
            text={words.text41}
            working={saving}
            onDismiss={() => setChangedMeanwhile(false)}
            actions={[
              { label: words.text20, onSelect: () => setChangedMeanwhile(false), autoFocus: true },
              { label: words.text42, onSelect: () => { stopEditing(); setDocSeq((value) => value + 1) } },
              { label: words.text43, primary: true, onSelect: () => void save(true) },
            ]}
          />
        )}
        {askingDiscard && (
          <QuestionDialog
            title={words.title3}
            text={words.text44}
            onDismiss={() => setAskingDiscard(false)}
            actions={[
              { label: words.text20, onSelect: () => setAskingDiscard(false), autoFocus: true },
              { label: words.text21, primary: true, onSelect: stopEditing },
            ]}
          />
        )}
      </section>
    )
  }

  if (open) {
    return (
      <section className="network-docs" aria-label={W.tip.documents}>
        <header className="network-docs-head">
          <button className="icon-button" type="button" aria-label={W.tip.back} title={W.tip.back} onClick={() => { setOpen(null); setDocSeq(0) }}><ArrowLeft size={18} aria-hidden /></button>
          <h1>{current ? current.title : W.tip.documents}</h1>
          {current && view === 'body' && (
            <button className="secondary-button zet-edit-start" type="button" onClick={() => setDraft({ title: current.title, summary: current.summary, body: current.body })}>
              <Pencil size={13} aria-hidden />{words.text45}</button>
          )}
        </header>
        <div className="network-docs-body">
          {doc === null && <p className="network-docs-note" role="status"><LoaderCircle size={14} className="is-spinning" aria-hidden />{words.text39}</p>}
          {doc === 'error' && <p className="network-docs-note" role="alert">{words.text33}</p>}
          {current && (
            <div className="glass-segmented zet-views network-doc-views" data-active={view} role="tablist" aria-label={words.aria7}>
              <span className="glass-slider" aria-hidden />
              {(['body', 'history'] as const).map((id) => (
                <button
                  key={id}
                  type="button"
                  role="tab"
                  aria-selected={view === id}
                  className={view === id ? 'active' : ''}
                  onClick={() => { setView(id); setVersion(null); if (id === 'history') loadHistory(current.id) }}
                >
                  {id === 'body' ? words.text30 : words.text32}
                </button>
              ))}
            </div>
          )}
          {current && view === 'history' && (
            version ? (
              <VersionView
                id={current.id}
                entry={version}
                latestRevision={Array.isArray(history) ? history[0]?.revision ?? null : null}
                onBack={() => setVersion(null)}
                onOpenZet={(id) => { setDocSeq(0); setOpen(id) }}
                onOpenAddress={(address) => { window.open(address, '_blank', 'noopener') }}
                onRestored={() => { setVersion(null); setView('body'); setDocSeq((value) => value + 1); loadList() }}
              />
            ) : history === null ? <p className="network-docs-note" role="status"><LoaderCircle size={14} className="is-spinning" aria-hidden />{words.text48}</p>
              : history === 'error' ? <p className="network-docs-note" role="alert">{words.text35} <button className="secondary-button" type="button" onClick={() => loadHistory(current.id)}>{W.firstRun.retry}</button></p>
                : history.length === 0 ? <p className="network-docs-note">{words.text49}</p>
                  : (
                    <ul className="zet-links">
                      {history.map((entry) => (
                        <li key={entry.revision}>
                          <button className="zet-link-row" type="button" data-revision={entry.revision} onClick={() => setVersion(entry)}>
                            <strong>{formatDateTimeSeconds(entry.time)}</strong>
                          </button>
                        </li>
                      ))}
                    </ul>
                  )
          )}
          {current && view === 'body' && (
            <article className="network-doc">
              <p className="network-doc-meta">{[current.folder, day(current.updatedAt)].filter(Boolean).join(' · ')}</p>
              {current.body.trim()
                ? <ZetMarkdown text={current.body} onOpenZet={(id) => { setDocSeq(0); setOpen(id) }} onOpenAddress={(address) => { window.open(address, '_blank', 'noopener') }} />
                : <p className="network-docs-note">{words.text46}</p>}
            </article>
          )}
        </div>
      </section>
    )
  }

  return (
    <section className="network-docs" aria-label={W.tip.documents} aria-busy={working}>
      <header className="network-docs-head">
        {menu}
        <h1>{W.tip.documents}</h1>
        {Array.isArray(list) && (
          <button className="secondary-button network-docs-tool" type="button" disabled={working || naming !== null} onClick={startFolder}>
            <FolderPlus size={14} strokeWidth={1.75} aria-hidden />{words.newFolder}
          </button>
        )}
      </header>
      <div className="network-docs-body">
        {list === null && <p className="network-docs-note" role="status"><LoaderCircle size={14} className="is-spinning" aria-hidden />{words.text26}</p>}
        {list === 'error' && (
          <p className="network-docs-note" role="alert">
            {words.text17} <button className="secondary-button" type="button" onClick={loadList}>{W.firstRun.retry}</button>
          </p>
        )}
        {folderNote && <p className="network-docs-note" role="alert">{folderNote}</p>}
        {naming !== null && (
          <div className="zet-row zet-folder-row is-renaming network-docs-naming">
            <span className="zet-folder-name">
              <Folder size={15} strokeWidth={1.75} aria-hidden />
              <input
                autoFocus
                value={naming}
                aria-label={words.renameLabel(words.newFolderName)}
                onFocus={(event) => event.currentTarget.select()}
                onChange={(event) => setNaming(event.target.value)}
                onBlur={(event) => commitFolder(event.currentTarget.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') { event.preventDefault(); commitFolder(event.currentTarget.value) }
                  if (event.key === 'Escape') { event.preventDefault(); namingDone.current = true; setNaming(null) }
                }}
              />
            </span>
          </div>
        )}
        {Array.isArray(list) && groups.length === 0 && naming === null && <p className="network-docs-note">{words.text27}</p>}
        {groups.map(({ folder, entries }) => (
          <section key={folder || '/'} className="network-docs-folder">
            {folder && (renaming === folder ? (
              <div className="zet-row zet-folder-row is-renaming network-docs-naming">
                <span className="zet-folder-name">
                  <Folder size={15} strokeWidth={1.75} aria-hidden />
                  <input
                    autoFocus
                    value={renameText}
                    aria-label={words.renameLabel(nameOf(folder))}
                    onFocus={(event) => event.currentTarget.select()}
                    onChange={(event) => setRenameText(event.target.value)}
                    onBlur={(event) => commitRename(folder, event.currentTarget.value)}
                    onKeyDown={(event) => {
                      if (event.key === 'Enter') { event.preventDefault(); commitRename(folder, event.currentTarget.value) }
                      if (event.key === 'Escape') { event.preventDefault(); renameDone.current = true; setRenaming(null) }
                    }}
                  />
                </span>
              </div>
            ) : (
              <div className="network-docs-folder-head">
                <h2>{folder}</h2>
                <RowMenu label={words.itemMenu(nameOf(folder))}>
                  {(close) => <MenuItem close={close} onSelect={() => startRename(folder)}>{words.rename}</MenuItem>}
                </RowMenu>
              </div>
            ))}
            {entries.length === 0 ? <p className="network-docs-note">{words.folderEmpty}</p> : (
              <ul>
                {entries.map((entry) => (
                  <li key={entry.id}>
                    <button type="button" className="network-docs-row" onClick={() => { setDocSeq(0); setOpen(entry.id) }}>
                      <FileText size={16} aria-hidden />
                      <span className="network-docs-title">{entry.title || entry.id}</span>
                      <span className="network-docs-day">{day(entry.updatedAt)}</span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </section>
        ))}
      </div>
    </section>
  )
}
