import { Fragment, type KeyboardEvent as ReactKeyboardEvent, type ReactNode, useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { FileArchive, FileText, FolderOpen, MessageSquare, Search, Settings, SquarePen, X } from 'lucide-react'
import { listZets, searchSources, type SourceSearchResult, type ZetListEntry } from '../archive'
import { searchChatsPage, type ChatEntry, type ChatGroup, type ChatSearchResult } from '../chats'
import { W } from '../words'
import { usePanelBrowserHold } from '../panel-browser'
import './SearchDialog.css'

type Tab = 'all' | 'chats' | 'documents' | 'sources' | 'actions'

export type SearchAction = { id: string; label: string; icon: 'new' | 'documents' | 'settings'; run: () => void }

type Item =
  | { kind: 'chat'; id: string; title: string; detail: string | null; snippet: string | null }
  | { kind: 'document'; id: string; title: string; detail: string | null; snippet: string | null }
  | { kind: 'source'; id: string; title: string; detail: string | null; snippet: null; documentId: string | null; media: boolean }
  | { kind: 'action'; id: string; title: string; icon: SearchAction['icon']; run: () => void }

const TABS: Array<{ id: Tab; label: string }> = [
  { id: 'all', label: W.searchDialog.text1 },
  { id: 'chats', label: W.chatRoom },
  { id: 'documents', label: W.document },
  { id: 'sources', label: W.rawMaterial },
  { id: 'actions', label: W.searchDialog.text2 },
]

// 원자료를 눌렀을 때 갈 곳: 이어진 문서가 있으면 그 문서, 사진·소리·영상이면 미디어 화면. 그 밖(이어진 문서 없는 글 파일 등)은
// 원자료 하나를 여는 뒤쪽 명령이 아직 없어(넷째 줄이 만드는 중) 눌리지 않는 줄로 둔다.
const isMediaType = (type: string | null) => Boolean(type && /^(audio|video|image)\//.test(type))

const RECENT = 6
const dateFormat = new Intl.DateTimeFormat('ko-KR', { month: 'long', day: 'numeric' })
function shortDate(value: string) {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? null : dateFormat.format(date)
}

/** 찾은 것을 한 번에 그리는 수(아래로 내리면 이만큼씩 더). */
const SEARCH_PAGE = 80

/**
 * 검색 창: 채팅방(이름과 말), 문서(제목과 요약), 원자료(파일 이름, source_search), 할 일을 한 창에서 찾는다. Claude 앱의 검색 창 틀(제목과 닫기,
 * 위에 종류 탭, 아래에 키 안내)을 따랐다. 찾는 말이 없으면 최근 것. 키보드만으로 쓸 수 있다
 * (↑↓ 고르기, Enter 열기, ← → 종류 바꾸기, Esc·Ctrl+K 닫기).
 */
export function SearchDialog({
  chats,
  groups,
  actions,
  onClose,
  onOpenChat,
  onOpenDocument,
  onOpenMedia,
  onDownloadSource,
}: {
  chats: ChatEntry[]
  groups: ChatGroup[] | null
  actions: SearchAction[]
  onClose: () => void
  onOpenChat: (id: string) => void
  onOpenDocument: (id: string) => void
  /** 사진·소리·영상 원자료를 고르면 미디어 화면으로. */
  onOpenMedia?: () => void
  /** 휴대폰 몸소(S75 둘째): 문서에 이어지지 않은 원자료를 고르면 내려받는다(미디어 화면이 없음). */
  onDownloadSource?: (id: string, name: string) => void
}) {
  const tabs = TABS
  // 화면을 덮는 창이라 열려 있는 동안 패널 브라우저를 숨긴다.
  usePanelBrowserHold()
  const [query, setQuery] = useState('')
  const [tab, setTab] = useState<Tab>('all')
  const [active, setActive] = useState(0)
  const [chatResults, setChatResults] = useState<ChatSearchResult[] | null>(null)
  // 채팅방 찾기의 다음 쪽(chat_search_page). 새 단추 없이 목록 끝까지 내리면 이어 받는다.
  const [chatNext, setChatNext] = useState<number | null>(null)
  const chatLoadingMore = useRef(false)
  const chatQuery = useRef('')
  const [documents, setDocuments] = useState<ZetListEntry[] | null>(null)
  const [sourceResults, setSourceResults] = useState<SourceSearchResult[] | null>(null)
  const [leaving, setLeaving] = useState(false)
  // 뒤쪽 찾기(채팅방·원자료)가 실패했는지와 다시 시도(2026-11-17: 조용히 실패하지 않게).
  const [searchFailed, setSearchFailed] = useState(false)
  const [retry, setRetry] = useState(0)
  const input = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const text = query.trim()

  // 열 때 초점이 있던 곳(입력 칸, 고치던 문서)을 기억했다가 닫으면 돌려놓는다.
  useEffect(() => {
    const before = document.activeElement as HTMLElement | null
    input.current?.focus()
    listZets().then(setDocuments).catch(() => setDocuments([]))
    return () => { before?.focus?.() }
  }, [])

  // 채팅방은 백엔드에서 찾는다(이름과 말). 적는 동안 매번 묻지 않게 잠깐 기다린다.
  // 찾는 말이나 채팅방 목록(chats)이 바뀌면 처음부터 다시 찾는다(쪽을 이어 받는 동안 결과가 바뀌면 겹치거나 빠질 수 있어서).
  useEffect(() => {
    setChatResults(null)
    setChatNext(null)
    setSearchFailed(false)
    chatQuery.current = text
    chatLoadingMore.current = false
    if (!text) return
    const timer = window.setTimeout(() => {
      searchChatsPage(text).then((page) => {
        if (chatQuery.current !== text) return
        setChatResults(page.items)
        setChatNext(page.hasMore ? page.nextOffset : null)
      }).catch(() => { setChatResults([]); setSearchFailed(true) })
    }, 180)
    return () => window.clearTimeout(timer)
  }, [text, retry, chats])

  // 다음 쪽을 받아 붙인다(이미 받은 채팅방은 빼고). 찾는 말이 그사이 바뀌었으면 버린다.
  const loadMoreChats = () => {
    if (chatNext === null || chatLoadingMore.current || !text) return
    chatLoadingMore.current = true
    const asked = text
    searchChatsPage(asked, chatNext).then((page) => {
      if (chatQuery.current !== asked) return
      setChatResults((current) => {
        const seen = new Set((current ?? []).map((item) => item.id))
        return [...(current ?? []), ...page.items.filter((item) => !seen.has(item.id))]
      })
      setChatNext(page.hasMore ? page.nextOffset : null)
      // 받은 만큼 그릴 수도 늘린다(끝에 서 있으면 스크롤이 다시 일어나지 않으므로).
      setBudget((current) => current + page.items.length)
    }).catch(() => setChatNext(null)).finally(() => { if (chatQuery.current === asked) chatLoadingMore.current = false })
  }

  // 원자료도 뒤쪽에서 찾는다(파일 이름). 찾는 말이 있을 때만(빈 말이면 최근 원자료를 늘어놓지 않는다).
  useEffect(() => {
    setSourceResults(null)
    if (!text) return
    const timer = window.setTimeout(() => {
      searchSources(text).then(setSourceResults).catch(() => { setSourceResults([]); setSearchFailed(true) })
    }, 180)
    return () => window.clearTimeout(timer)
  }, [text, retry])

  const groupName = (id: string | null | undefined) => groups?.find((group) => group.id === id)?.name ?? null

  const sections = useMemo(() => {
    const lower = text.toLocaleLowerCase('ko-KR')
    const chatItems: Item[] = (text ? chatResults ?? [] : chats.slice(0, RECENT)).map((chat) => ({
      kind: 'chat',
      id: chat.id,
      title: chat.title || W.searchDialog.text3(W.chatRoom),
      detail: groupName(chat.groupId) ?? shortDate(chat.updatedAt),
      snippet: text ? (chat as ChatSearchResult).snippet ?? null : null,
    }))
    const docSource = [...(documents ?? [])].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
    const docItems: Item[] = (text
      ? docSource.filter((entry) => `${entry.title} ${entry.summary}`.toLocaleLowerCase('ko-KR').includes(lower))
      : docSource.slice(0, RECENT)
    ).map((entry) => ({ kind: 'document', id: entry.id, title: entry.title, detail: shortDate(entry.updatedAt), snippet: text ? entry.summary : null }))
    const sourceItems: Item[] = (text ? sourceResults ?? [] : []).map((entry) => ({
      kind: 'source',
      id: entry.id,
      title: entry.originalFileName,
      detail: shortDate(entry.createdAt),
      snippet: null,
      documentId: entry.linkedDocumentIds[0] ?? null,
      media: isMediaType(entry.mediaType),
    }))
    const actionItems: Item[] = actions
      .filter((action) => !text || action.label.toLocaleLowerCase('ko-KR').includes(lower))
      .map((action) => ({ kind: 'action', id: action.id, title: action.label, icon: action.icon, run: action.run }))
    const all: Array<{ key: Tab; title: string; items: Item[] }> = [
      { key: 'chats', title: text ? W.chatRoom : W.searchDialog.text4(W.chatRoom), items: chatItems },
      { key: 'documents', title: text ? W.document : W.searchDialog.text4(W.document), items: docItems },
      { key: 'sources', title: W.rawMaterial, items: sourceItems },
      { key: 'actions', title: W.searchDialog.text2, items: actionItems },
    ]
    return all.filter((section) => (tab === 'all' || tab === section.key) && section.items.length > 0)
  }, [text, chatResults, sourceResults, chats, documents, actions, tab, groups])

  // 찾은 것이 많으면(채팅방 500개·문서 1,000개에서 한 글자) 위에서부터 SEARCH_PAGE 개씩만 그리고, 아래로 내리거나 화살표로 끝에 닿으면
  // 이어서 그린다. 찾은 것과 그 차례는 같다(2026-11-17 많이 쌓였을 때 재기: 글자 하나에 0.58초).
  const [budget, setBudget] = useState(SEARCH_PAGE)
  useEffect(() => { setBudget(SEARCH_PAGE) }, [text, tab])
  const total = sections.reduce((sum, section) => sum + section.items.length, 0)
  const shownSections = useMemo(() => {
    let left = budget
    return sections.map((section) => {
      const items = section.items.slice(0, Math.max(0, left))
      left -= items.length
      return { ...section, items }
    }).filter((section) => section.items.length > 0)
  }, [sections, budget])
  const flat = shownSections.flatMap((section) => section.items)
  const more = flat.length < total
  // 그린 것이 끝에 닿으면 더 그리고, 다 그렸는데 뒤쪽에 채팅방이 더 있으면 다음 쪽을 받는다.
  const showMore = () => { if (more) setBudget((current) => current + SEARCH_PAGE); else if (tab === 'all' || tab === 'chats') loadMoreChats() }

  useEffect(() => { setActive(0) }, [text, tab])
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView({ block: 'nearest' })
  }, [active])

  const close = () => {
    setLeaving(true)
    window.setTimeout(onClose, 120)
  }

  const canOpen = (item: Item) => item.kind !== 'source' || Boolean(item.documentId) || (item.media && Boolean(onOpenMedia)) || Boolean(onDownloadSource)
  const choose = (item: Item | undefined) => {
    if (!item || !canOpen(item)) return
    close()
    window.setTimeout(() => {
      if (item.kind === 'chat') onOpenChat(item.id)
      else if (item.kind === 'document') onOpenDocument(item.id)
      else if (item.kind === 'source') {
        if (item.documentId) onOpenDocument(item.documentId)
        else if (item.media && onOpenMedia) onOpenMedia()
        else onDownloadSource?.(item.id, item.title)
      }
      else item.run()
    }, 130)
  }

  const onKey = (event: ReactKeyboardEvent) => {
    if (event.nativeEvent.isComposing) return
    if (event.key === 'Escape' || ((event.ctrlKey || event.metaKey) && event.code === 'KeyK')) {
      event.preventDefault()
      event.stopPropagation()
      close()
      return
    }
    if (event.key === 'ArrowDown') { event.preventDefault(); if (active >= flat.length - 2) showMore(); setActive((i) => Math.min(flat.length - 1, i + 1)) }
    if (event.key === 'ArrowUp') { event.preventDefault(); setActive((i) => Math.max(0, i - 1)) }
    if (event.key === 'Enter') { event.preventDefault(); choose(flat[active]) }
    if ((event.key === 'ArrowLeft' || event.key === 'ArrowRight') && !event.altKey && !event.shiftKey) {
      event.preventDefault()
      const at = tabs.findIndex((item) => item.id === tab)
      const next = (at + (event.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length
      setTab(tabs[next].id)
    }
  }

  let index = -1
  return createPortal(
    <div className="search-backdrop" data-leaving={leaving} onMouseDown={(event) => { if (event.target === event.currentTarget) close() }}>
      <div className="search-dialog" role="dialog" aria-modal="true" aria-label={W.search} onKeyDown={onKey}>
        <header className="search-head">
          <h2>{W.search}</h2>
          <button className="search-close" type="button" aria-label={W.searchDialog.aria1} onClick={close}>
            <X size={16} aria-hidden />
          </button>
        </header>
        <div className="search-tabs" role="tablist" aria-label={W.searchDialog.aria2}>
          {tabs.map((item) => (
            <button key={item.id} type="button" role="tab" aria-selected={tab === item.id} className="search-tab" onClick={() => { setTab(item.id); input.current?.focus() }}>
              {item.label}
            </button>
          ))}
        </div>
        <label className="search-field">
          <Search size={16} aria-hidden />
          <input
            ref={input}
            value={query}
            placeholder={W.searchDialog.hint2(W.chatRoom, W.document, W.rawMaterial)}
            aria-label={W.search}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <div
          ref={listRef}
          className="search-results"
          role="listbox"
          aria-label={W.searchDialog.aria3}
          onScroll={(event) => { const el = event.currentTarget; if (el.scrollTop + el.clientHeight >= el.scrollHeight - 240) showMore() }}
        >
          {/* 찾는 말이 있을 때: 실패하면 그 줄과 다시 시도, 아직 찾는 중이고 보일 것이 없으면 "검색하는 중", 다 찾았는데 없으면 "찾은 것이 없습니다". */}
          {text && searchFailed && (
            <div className="search-state" role="alert">
              <span>{W.searchDialog.failed}</span>
              <button className="secondary-button" type="button" onClick={() => setRetry((n) => n + 1)}>{W.firstRun.retry}</button>
            </div>
          )}
          {text && !searchFailed && shownSections.length === 0 && (chatResults === null || sourceResults === null) && (
            <p className="search-state" role="status">{W.searchDialog.searching}</p>
          )}
          {text && !searchFailed && shownSections.length === 0 && chatResults !== null && sourceResults !== null && (
            <p className="search-state" role="status">{W.searchDialog.empty}</p>
          )}
          {shownSections.map((section) => (
            <section key={section.key} className="search-section" aria-label={section.title}>
              <h3>{section.title}</h3>
              {section.items.map((item) => {
                index += 1
                const at = index
                return (
                  <button
                    key={`${item.kind}-${item.id}`}
                    type="button"
                    role="option"
                    aria-selected={at === active}
                    data-index={at}
                    className="search-item"
                    aria-disabled={!canOpen(item) || undefined}
                    onMouseMove={() => setActive(at)}
                    onClick={() => choose(item)}
                  >
                    <ItemIcon item={item} />
                    <span className="search-item-text">
                      <span className="search-item-title">{item.title}</span>
                      {item.kind !== 'action' && item.snippet && <span className="search-item-snippet">{item.snippet}</span>}
                    </span>
                    {item.kind !== 'action' && item.detail && <span className="search-item-detail">{item.detail}</span>}
                  </button>
                )
              })}
            </section>
          ))}
        </div>
        <footer className="search-keys" aria-label={W.searchDialog.aria4}>
          <KeyHint label={W.searchDialog.aria1} keys={['Esc']} />
          <KeyHint label={W.searchDialog.label1} keys={['↑', '↓']} />
          <KeyHint label={W.searchDialog.label2} keys={['Enter']} />
          <KeyHint label={W.searchDialog.label3} keys={['←', '→']} />
        </footer>
      </div>
    </div>,
    document.body,
  )
}

function ItemIcon({ item }: { item: Item }) {
  let icon: ReactNode
  if (item.kind === 'chat') icon = <MessageSquare size={15} aria-hidden />
  else if (item.kind === 'document') icon = <FileText size={15} aria-hidden />
  else if (item.kind === 'source') icon = <FileArchive size={15} aria-hidden />
  else icon = item.icon === 'new' ? <SquarePen size={15} aria-hidden /> : item.icon === 'documents' ? <FolderOpen size={15} aria-hidden /> : <Settings size={15} aria-hidden />
  return <span className="search-item-icon">{icon}</span>
}

function KeyHint({ label, keys }: { label: string; keys: string[] }) {
  return (
    <span className="search-key">
      <span>{label}</span>
      {keys.map((key, i) => (
        <Fragment key={key}>
          {i > 0 && <span className="search-key-gap" />}
          <kbd>{key}</kbd>
        </Fragment>
      ))}
    </span>
  )
}
