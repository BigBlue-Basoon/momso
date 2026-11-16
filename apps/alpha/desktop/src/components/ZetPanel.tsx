import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from 'react'
import { createPortal } from 'react-dom'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { ArrowLeft, Check, FileText, PanelRight, Pencil, Plus, RefreshCw, ScrollText, Search, Undo2, X } from 'lucide-react'
import {
  archiveErrorMessage,
  isChangedMeanwhileError,
  isUnsavedError,
  listZets,
  openWebAddress,
  readReceipt,
  readZet,
  readZetHistory,
  readZetLinked,
  readZetVersion,
  restoreZetVersion,
  saveZet,
  type ReceiptReference,
  type ReceiptTool,
  type ZetDocument,
  type ZetEdgeType,
  type ZetHistoryEntry,
  type ZetKind,
  type ZetLinked,
  type ZetLinkedEntry,
  type ZetListEntry,
  type ZetStatus,
} from '../archive'
import { ZetMarkdown } from './ZetMarkdown'
import './ZetPanel.css'
import { W } from '../words'
import { ModelName } from './ModelName'
import { PROVIDER_NAME } from '../ai-models'
import type { ChatDocumentItem, ChatItems, ItemBy } from '../chat-items'
import { MediaGallery } from './MediaGallery'
import { ProviderMark, type ProviderMarkId } from './ProviderMark'
import { ShortcutTip } from './ShortcutTip'

// 패널이 열려도 왼쪽 칸(아이콘 띠, 목록)은 그대로 둔다. 가운데 대화 칸이 좁아지고,
// 그래도 자리가 모자라면 패널이 좁아진다.
// 왼쪽 두 칸의 너비는 ZetPanel.css 의 칸 배치와 같다: 아이콘 띠 52, 목록 clamp(220px, 20vw, 282px).
// 창 너비 1120 아래에서는 아이콘 띠 50, 목록 220. 목록을 접으면 아이콘 띠만 남는다.
function leftWidth(bodyWidth: number, listHidden: boolean) {
  const rail = bodyWidth <= 1120 ? 50 : 52
  if (listHidden) return rail
  return rail + (bodyWidth <= 1120 ? 220 : clamp(bodyWidth * 0.2, 220, 282))
}
// 패널이 열렸을 때 가운데 대화 칸이 지키는 너비.
const CENTER_WIDTH_MIN = 360
// 패널이 줄어들 수 있는 가장 좁은 너비. 창이 이보다도 좁으면 기록 보기 칸이 더 좁아진다.
const PANEL_WIDTH_MIN = 240
const PANEL_WIDTH_DEFAULT = 400
const PANEL_WIDTH_KEY = 'momso.zet-panel-width'
const MOVE_MS = 300

const KIND_LABEL: Record<ZetKind, string> = {
  summary: W.zetPanel.text1,
  transcript: W.zetPanel.text2,
  note: W.zetPanel.text3,
}

const STATUS_LABEL: Record<ZetStatus, string> = {
  draft: W.zetPanel.text4,
  confirmed: W.zetPanel.text5,
}

// 이 글에서 다른 글로 나가는 연결.
const OUTGOING_LABEL: Record<ZetEdgeType, string> = {
  summarizes: W.zetPanel.text6,
  derived_from: W.zetPanel.text7,
  continues: W.zetPanel.text8,
  references: W.zetPanel.text9,
  supersedes: W.zetPanel.text10,
}

// 다른 글에서 이 글로 들어오는 연결.
const INCOMING_LABEL: Record<ZetEdgeType, string> = {
  summarizes: W.zetPanel.text11,
  derived_from: W.zetPanel.text12,
  continues: W.zetPanel.text13,
  references: W.zetPanel.text14,
  supersedes: W.zetPanel.text15,
}

const TOOL_LABEL: Record<ReceiptTool, string> = {
  codex: W.provider.codex,
  claude: W.provider.claude,
  gemini: W.provider.gemini,
  person: W.zetPanel.text16,
}

const dateTimeFormat = new Intl.DateTimeFormat('ko-KR', {
  year: 'numeric',
  month: 'long',
  day: 'numeric',
  hour: 'numeric',
  minute: '2-digit',
})

// 저장한 판은 같은 분에 여러 번 저장해도 구분되도록 초까지 보여 준다.
const dateTimeSecondsFormat = new Intl.DateTimeFormat('ko-KR', {
  year: 'numeric',
  month: 'long',
  day: 'numeric',
  hour: 'numeric',
  minute: '2-digit',
  second: '2-digit',
})

const dateFormat = new Intl.DateTimeFormat('ko-KR', { year: 'numeric', month: 'long', day: 'numeric' })
const shortDateFormat = new Intl.DateTimeFormat('ko-KR', { month: 'long', day: 'numeric' })

function formatDateTime(value: string) {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : dateTimeFormat.format(date)
}

function formatDateTimeSeconds(value: string) {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : dateTimeSecondsFormat.format(date)
}

function formatDay(value: string) {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.getFullYear() === new Date().getFullYear()
    ? shortDateFormat.format(date)
    : dateFormat.format(date)
}

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), max)
}

function loadPanelWidth() {
  try {
    const stored = Number(window.localStorage.getItem(PANEL_WIDTH_KEY))
    return Number.isFinite(stored) && stored >= PANEL_WIDTH_MIN ? stored : PANEL_WIDTH_DEFAULT
  } catch {
    return PANEL_WIDTH_DEFAULT
  }
}

/**
 * 패널의 너비를 정한다. 패널은 왼쪽 칸을 접지 않는다. 가운데 칸이 가장 좁은 너비까지 물러난 뒤에는
 * 패널이 남은 자리만큼만 넓어진다.
 */
export function useZetPanelFrame(open: boolean, listHidden = false) {
  const bodyRef = useRef<HTMLDivElement>(null)
  const [bodyWidth, setBodyWidth] = useState(0)
  const [preferredWidth, setPreferredWidth] = useState(loadPanelWidth)
  const [resizing, setResizing] = useState(false)
  const [moving, setMoving] = useState(false)

  useEffect(() => {
    const element = bodyRef.current
    if (!element) return
    const observer = new ResizeObserver(([entry]) => setBodyWidth(entry.contentRect.width))
    observer.observe(element)
    return () => observer.disconnect()
  }, [])

  const maxWidth = Math.max(PANEL_WIDTH_MIN, bodyWidth - leftWidth(bodyWidth, listHidden) - CENTER_WIDTH_MIN)
  const width = clamp(preferredWidth, PANEL_WIDTH_MIN, maxWidth)

  // 열고 닫을 때만 움직임을 켠다. 늘 켜 두면 창 크기를 바꾸거나 경계를 끌 때 화면이 뒤늦게 따라온다.
  // 바뀌는 바로 그 장면부터 켜져 있어야 움직임이 걸리므로, 바뀐 것을 그리는 중에 바로 알아챈다.
  const settled = useRef(open)
  const changed = settled.current !== open
  useEffect(() => {
    if (settled.current === open) return
    settled.current = open
    setMoving(true)
    const timer = window.setTimeout(() => setMoving(false), MOVE_MS)
    return () => window.clearTimeout(timer)
  }, [open])

  const changeWidth = useCallback((next: number) => {
    const clamped = clamp(Math.round(next), PANEL_WIDTH_MIN, maxWidth)
    setPreferredWidth(clamped)
    try {
      window.localStorage.setItem(PANEL_WIDTH_KEY, String(clamped))
    } catch {
      // 너비를 기억하지 못해도 패널은 그대로 쓴다.
    }
  }, [maxWidth])

  return { bodyRef, width, maxWidth, moving: (moving || changed) && !resizing, resizing, setResizing, changeWidth }
}

type Loadable<T> =
  | { state: 'idle' | 'loading' }
  | { state: 'ready'; value: T }
  | { state: 'error'; message: string }

function useLoadable<T>(load: () => Promise<T>, fallback: string, enabled: boolean, deps: unknown[]) {
  const [result, setResult] = useState<Loadable<T>>({ state: enabled ? 'loading' : 'idle' })
  const [attempt, setAttempt] = useState(0)
  const loader = useRef(load)
  loader.current = load

  useEffect(() => {
    if (!enabled) return
    let alive = true
    setResult((current) => (current.state === 'ready' ? current : { state: 'loading' }))
    loader.current()
      .then((value) => {
        if (alive) setResult({ state: 'ready', value })
      })
      .catch((error) => {
        if (alive) setResult({ state: 'error', message: archiveErrorMessage(error, fallback) })
      })
    return () => {
      alive = false
    }
  }, [enabled, attempt, fallback, ...deps])

  const reload = useCallback(() => setAttempt((current) => current + 1), [])
  return [result, reload] as const
}

/** 글 화면이 패널에 알려 주는 것: 고치던 내용을 저장하는 방법과 버리는 방법. */
type ZetEditor = {
  /** 저장한다. 저장했으면 true, 못 했으면 false(까닭은 글 화면에 보인다). */
  save: () => Promise<boolean>
  discard: () => void
}

/** 저장하지 않은 내용이 있는 글(key)을 두고 무엇을 닫으려 하는지. */
type Leaving = {
  key: string
  then: 'close-tab' | 'close-panel' | 'close-window'
}

// 탭은 자리(scope)마다 따로 기억한다: 홈의 패널은 채팅방마다('chat:<id>'), 문서 화면과 예전 패널은 하나('all').
// 다른 채팅방의 탭은 숨겨 둘 뿐 닫지 않는다(고치던 글이 사라지지 않게).
type PanelTab =
  | { key: string; kind: 'list'; scope: string }
  | { key: string; kind: 'zet'; id: string; scope: string }
  | { key: string; kind: 'receipt'; id: string; scope: string }

const listTab = (scope: string): PanelTab => ({ key: `${scope}|list`, kind: 'list', scope })

/**
 * 오른쪽 패널을 여닫는 단추. 화면 맨 위 줄의 오른쪽 끝에 하나만 둔다:
 * 패널이 닫혀 있으면 대화 칸 맨 위 줄에, 열려 있으면 패널의 탭 줄 끝에 있다.
 */
export function ZetPanelToggle({ open, onToggle, fresh = false }: { open: boolean; onToggle: () => void; fresh?: boolean }) {
  return (
    <ShortcutTip label={W.document} keys={['Ctrl', ']']}>
      <button
        className={fresh ? 'icon-button zet-panel-toggle has-fresh' : 'icon-button zet-panel-toggle'}
        type="button"
        aria-label={fresh ? W.gallery.newInPanel(W.document) : W.document}
        aria-keyshortcuts="Control+BracketRight"
        aria-pressed={open}
        onClick={onToggle}
      >
        <PanelRight size={17} strokeWidth={1.7} aria-hidden />
      </button>
    </ShortcutTip>
  )
}

type ZetPanelProps = {
  open: boolean
  /** side: 홈의 오른쪽 패널. page: 문서 화면에서 화면 가득(너비 조절과 패널 단추 없음). */
  variant?: 'side' | 'page'
  width: number
  maxWidth: number
  onWidthChange: (width: number) => void
  onResizingChange: (resizing: boolean) => void
  onClose: () => void
  /** 패널을 여닫는 단추를 눌렀을 때. 패널이 열려 있는 동안 단추는 탭 줄 끝에 있다. */
  onToggle: () => void
  onNotice: (message: string) => void
  /** 지금 연 채팅방(홈의 패널). 새 채팅방이면 null. */
  chatId?: string | null
  /** 그 채팅방에서 다룬 문서와 미디어. undefined 면 명령이 없어 예전처럼 문서 전체를 보인다. null 이면 읽는 중. */
  chatItems?: ChatItems | null
  /** 미디어를 크게 보다가 채팅방 이름을 눌렀을 때. */
  onOpenChat?: (chatId: string) => void
}

export function ZetPanel({
  open,
  variant = 'side',
  width,
  maxWidth,
  onWidthChange,
  onResizingChange,
  onClose,
  onToggle,
  onNotice,
  chatId = null,
  chatItems,
  onOpenChat,
}: ZetPanelProps) {
  const byChat = variant === 'side' && chatItems !== undefined
  const scope = byChat ? `chat:${chatId ?? 'new'}` : 'all'
  const scopeRef = useRef(scope)
  scopeRef.current = scope
  const [tabs, setTabs] = useState<PanelTab[]>([])
  const [activeByScope, setActiveByScope] = useState<Record<string, string>>({})
  const activeKey = activeByScope[scope] ?? ''
  const setActiveKey = useCallback((key: string) => {
    setActiveByScope((current) => (current[scopeRef.current] === key ? current : { ...current, [scopeRef.current]: key }))
  }, [])
  const shown = tabs.filter((tab) => tab.scope === scope)
  const [knownTitles, setKnownTitles] = useState<Record<string, string>>({})
  // 패널을 연 횟수. 열 때마다 글 목록을 새로 읽는다.
  const [openCount, setOpenCount] = useState(0)
  const drag = useRef<{ startX: number; startWidth: number } | null>(null)
  const tabsRef = useRef(tabs)
  tabsRef.current = tabs
  const activeKeyRef = useRef(activeKey)
  activeKeyRef.current = activeKey
  // 고치는 중인 글: 탭마다 저장하지 않은 내용이 있는지와, 저장하거나 버리는 방법을 글 화면이 알려 준다.
  const [dirtyKeys, setDirtyKeys] = useState<ReadonlySet<string>>(new Set())
  const editors = useRef(new Map<string, ZetEditor>())
  // 저장하지 않은 내용이 있는 탭이나 패널을 닫으려 할 때 띄우는 물음.
  const [leaving, setLeaving] = useState<Leaving | null>(null)
  const [leaveWorking, setLeaveWorking] = useState(false)

  const [list, reloadList] = useLoadable(listZets, W.zetPanel.text17, openCount > 0, [openCount])

  const entries = list.state === 'ready' ? list.value : null
  const entriesById = useMemo(
    () => new Map((entries ?? []).map((entry) => [entry.id, entry])),
    [entries],
  )

  // 열린 글이 없으면 글을 고르는 탭으로 시작한다.
  useEffect(() => {
    if (!open) return
    setOpenCount((current) => current + 1)
    // 이 자리(채팅방)에 열린 탭이 없으면 고르는 탭으로 시작한다. 채팅방을 바꾸면 그 채팅방의 탭이 돌아온다.
    if (!tabsRef.current.some((tab) => tab.scope === scope)) {
      const first = listTab(scope)
      setTabs((current) => [...current, first])
      setActiveKey(first.key)
    }
  }, [open, scope, setActiveKey])

  const titleOf = useCallback(
    (id: string) => entriesById.get(id)?.title ?? knownTitles[id] ?? null,
    [entriesById, knownTitles],
  )

  const rememberTitle = useCallback((id: string, title: string) => {
    setKnownTitles((current) => (current[id] === title ? current : { ...current, [id]: title }))
  }, [])

  const openDocument = useCallback((kind: 'zet' | 'receipt', id: string) => {
    const where = scopeRef.current
    const key = `${where}|${kind}:${id}`
    setTabs((current) => {
      if (current.some((tab) => tab.key === key)) return current
      const next: PanelTab = { key, kind, id, scope: where }
      // 글을 고르는 탭에서 고르면 그 탭이 고른 글로 바뀐다. 글 안에서 눌렀으면 옆에 새 탭으로 열린다.
      const activeIndex = current.findIndex((tab) => tab.key === activeKeyRef.current)
      if (activeIndex >= 0 && current[activeIndex].kind === 'list') {
        return current.map((tab, index) => (index === activeIndex ? next : tab))
      }
      const insertAt = activeIndex >= 0 ? activeIndex + 1 : current.length
      return [...current.slice(0, insertAt), next, ...current.slice(insertAt)]
    })
    setActiveKey(key)
  }, [setActiveKey])

  // 바깥 주소는 기본 브라우저에서 연다. 열지 못하면 그 사실만 알린다.
  const openAddress = useCallback((address: string) => {
    openWebAddress(address).catch(() => onNotice(W.zetPanel.text18))
  }, [onNotice])

  const openZet = useCallback((id: string) => openDocument('zet', id), [openDocument])

  // 다른 곳(채팅방의 답 안 문서 링크 등)에서 문서를 열어 달라고 하면 연다.
  useEffect(() => {
    const onOpen = (event: Event) => {
      const id = (event as CustomEvent<{ id: string }>).detail?.id
      if (id) openZet(id)
    }
    window.addEventListener('momso:open-document', onOpen)
    return () => window.removeEventListener('momso:open-document', onOpen)
  }, [openZet])
  const openReceipt = useCallback((id: string) => openDocument('receipt', id), [openDocument])

  function showList() {
    const first = listTab(scope)
    setTabs((current) => (current.some((tab) => tab.key === first.key) ? current : [...current, first]))
    setActiveKey(first.key)
    reloadList()
  }

  const registerEditor = useCallback((key: string, editor: ZetEditor | null) => {
    if (editor) editors.current.set(key, editor)
    else editors.current.delete(key)
  }, [])

  const changeDirty = useCallback((key: string, dirty: boolean) => {
    setDirtyKeys((current) => {
      if (current.has(key) === dirty) return current
      const next = new Set(current)
      if (dirty) next.add(key)
      else next.delete(key)
      return next
    })
  }, [])

  function removeTab(key: string) {
    const index = shown.findIndex((tab) => tab.key === key)
    if (index < 0) return
    setTabs(tabs.filter((tab) => tab.key !== key))
    const next = shown.filter((tab) => tab.key !== key)
    if (next.length === 0) {
      // 마지막 탭을 닫으면 패널도 닫힌다.
      setActiveKey('')
      onClose()
      return
    }
    if (key === activeKey) setActiveKey(next[Math.min(index, next.length - 1)].key)
  }

  // 저장하지 않은 내용이 있는 탭은 닫기 전에 한 번 묻는다.
  function closeTab(key: string) {
    if (dirtyKeys.has(key)) {
      setActiveKey(key)
      setLeaving({ key, then: 'close-tab' })
      return
    }
    removeTab(key)
  }

  // 패널이나 앱 창을 닫을 때도 저장하지 않은 내용이 있는 글이 있으면 그 글을 보여 주고 묻는다.
  // 그런 글이 없으면(또는 다 답했으면) 닫는다.
  function closeAll(then: 'close-panel' | 'close-window', skipKey?: string) {
    // 패널만 닫을 때는 지금 채팅방의 탭만 본다(다른 채팅방의 탭은 숨어 있을 뿐 그대로 남는다). 창을 닫을 때는 전부.
    const dirty = (then === 'close-panel' ? shown : tabs).find((tab) => tab.key !== skipKey && dirtyKeys.has(tab.key))
    if (dirty) {
      setActiveKey(dirty.key)
      setLeaving({ key: dirty.key, then })
      return
    }
    setLeaving(null)
    if (then === 'close-panel') onToggle()
    else void getCurrentWindow().destroy()
  }

  // 물음에 답한 뒤: 닫으려던 것을 마저 닫는다.
  function finishLeaving(answered: Leaving) {
    setLeaving(null)
    if (answered.then === 'close-tab') removeTab(answered.key)
    else closeAll(answered.then, answered.key)
  }

  // 고치던 글이 있을 때 앱 창을 닫으려 하면 붙잡고 묻는다. 고치던 글이 없으면 그냥 닫힌다.
  const askBeforeWindowClose = useRef<() => boolean>(() => false)
  askBeforeWindowClose.current = () => {
    if (!tabs.some((tab) => dirtyKeys.has(tab.key))) return false
    closeAll('close-window')
    return true
  }
  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    void getCurrentWindow().onCloseRequested((event) => {
      if (askBeforeWindowClose.current()) event.preventDefault()
    }).then((stop) => {
      if (disposed) stop()
      else unlisten = stop
    }).catch(() => {})
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  async function answerLeaving(answer: 'save' | 'discard' | 'stay') {
    if (!leaving) return
    if (answer === 'stay') {
      setLeaving(null)
      return
    }
    const editor = editors.current.get(leaving.key)
    if (answer === 'discard') {
      editor?.discard()
      finishLeaving(leaving)
      return
    }
    setLeaveWorking(true)
    const saved = editor ? await editor.save() : true
    setLeaveWorking(false)
    // 저장하지 못했으면 닫지 않는다. 왜 못 했는지는 글 화면에 보인다.
    if (saved) finishLeaving(leaving)
    else setLeaving(null)
  }

  function tabTitle(tab: PanelTab) {
    if (tab.kind === 'list') return byChat ? W.gallery.chatTab : W.document
    if (tab.kind === 'receipt') return W.workLog
    return titleOf(tab.id) ?? W.document
  }

  function startResize(event: ReactPointerEvent<HTMLDivElement>) {
    if (event.button !== 0) return
    event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    drag.current = { startX: event.clientX, startWidth: width }
    onResizingChange(true)
  }

  function moveResize(event: ReactPointerEvent<HTMLDivElement>) {
    if (!drag.current) return
    onWidthChange(drag.current.startWidth + (drag.current.startX - event.clientX))
  }

  function endResize(event: ReactPointerEvent<HTMLDivElement>) {
    if (!drag.current) return
    drag.current = null
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId)
    }
    onResizingChange(false)
  }

  function resizeWithKeys(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return
    event.preventDefault()
    onWidthChange(width + (event.key === 'ArrowLeft' ? 16 : -16))
  }

  return (
    <aside className="zet-panel" data-variant={variant} aria-label={W.document} aria-hidden={!open}>
      <div className="zet-panel-frame" inert={!open}>
        <div
          className="zet-panel-resizer"
          role="separator"
          aria-orientation="vertical"
          aria-label={W.zetPanel.aria1}
          aria-valuemin={PANEL_WIDTH_MIN}
          aria-valuemax={Math.round(maxWidth)}
          aria-valuenow={Math.round(width)}
          tabIndex={0}
          onPointerDown={startResize}
          onPointerMove={moveResize}
          onPointerUp={endResize}
          onPointerCancel={endResize}
          onKeyDown={resizeWithKeys}
        />

        <header className="zet-tabbar">
          <div className="zet-tabs" role="tablist" aria-label={W.zetPanel.aria2}>
            {shown.map((tab) => {
              const title = tabTitle(tab)
              const isActive = tab.key === activeKey
              return (
                <div
                  key={tab.key}
                  className={isActive ? 'zet-tab active' : 'zet-tab'}
                  onAuxClick={(event) => {
                    if (event.button === 1) closeTab(tab.key)
                  }}
                >
                  <button
                    className="zet-tab-label"
                    type="button"
                    role="tab"
                    aria-selected={isActive}
                    title={title}
                    onClick={() => setActiveKey(tab.key)}
                  >
                    {tab.kind === 'receipt'
                      ? <ScrollText size={13} strokeWidth={1.75} aria-hidden />
                      : tab.kind === 'zet'
                        ? <FileText size={13} strokeWidth={1.75} aria-hidden />
                        : null}
                    <span>{title}</span>
                    {dirtyKeys.has(tab.key) && <i className="zet-tab-dirty" role="img" aria-label={W.zetPanel.aria3} />}
                  </button>
                  <button
                    className="zet-tab-close"
                    type="button"
                    aria-label={W.zetPanel.aria4(title)}
                    title={W.zetPanel.tip1}
                    onClick={() => closeTab(tab.key)}
                  >
                    <X size={13} strokeWidth={1.75} aria-hidden />
                  </button>
                </div>
              )
            })}
          </div>
          {!shown.some((tab) => tab.kind === 'list') && (
            <button className="icon-button zet-tab-add" type="button" aria-label={W.zetPanel.aria5} title={W.zetPanel.aria5} onClick={showList}>
              <Plus size={16} aria-hidden />
            </button>
          )}
          {open && variant === 'side' && <ZetPanelToggle open onToggle={() => closeAll('close-panel')} />}
        </header>

        {leaving && (
          <QuestionDialog
            title={W.zetPanel.title1}
            text={W.zetPanel.text19(tabTitle(tabs.find((tab) => tab.key === leaving.key) ?? listTab(scope)))}
            working={leaveWorking}
            onDismiss={() => void answerLeaving('stay')}
            actions={[
              { label: W.zetPanel.text20, onSelect: () => void answerLeaving('stay'), autoFocus: true },
              { label: W.zetPanel.text21, onSelect: () => void answerLeaving('discard') },
              { label: W.zetPanel.text22, primary: true, onSelect: () => void answerLeaving('save') },
            ]}
          />
        )}

        <div className="zet-panel-body">
          {tabs.map((tab) => (
            <section key={tab.key} className="zet-pane" role="tabpanel" aria-label={tabTitle(tab)} hidden={tab.scope !== scope || tab.key !== activeKey}>
              {tab.kind === 'list' ? (
                byChat && tab.scope !== 'all'
                  ? (tab.scope === scope ? <ChatItemsView items={chatItems ?? null} onOpenZet={openZet} onOpenChat={onOpenChat} /> : null)
                  : <ZetListView list={list} onOpen={openZet} onReload={reloadList} />
              ) : tab.kind === 'zet' ? (
                <ZetDocumentView
                  id={tab.id}
                  tabKey={tab.key}
                  onEditor={registerEditor}
                  onDirtyChange={changeDirty}
                  onSaved={() => {
                    reloadList()
                    onNotice(W.zetPanel.text23)
                  }}
                  onOpenZet={openZet}
                  onOpenReceipt={openReceipt}
                  onOpenAddress={openAddress}
                  onTitle={rememberTitle}
                  onRestored={() => {
                    reloadList()
                    onNotice(W.zetPanel.text24)
                  }}
                />
              ) : (
                <ReceiptView id={tab.id} onOpenZet={openZet} onOpenAddress={openAddress} />
              )}
            </section>
          ))}
        </div>
      </div>
    </aside>
  )
}

const MARK: Record<'codex' | 'claude' | 'gemini', ProviderMarkId> = { codex: 'chatgpt', claude: 'claude', gemini: 'gemini' }
const itemTimeFormat = new Intl.DateTimeFormat('ko-KR', { month: 'long', day: 'numeric', hour: 'numeric', minute: '2-digit' })

/**
 * 그 채팅방에서 무엇을, 누가(AI 면 회사 CI 와 모델), 언제 했는지. 사용자가 한 것이면 누가는 적지 않는다.
 * 뒤쪽이 null 로 준 칸은 비워 둔다(짐작해 채우지 않는다). 겹쳐 답한 채팅방이 있어 가를 수 없으면 by 가 null 이라 누가는 적지 않는다.
 */
function ItemDid({ how, by, at }: { how: string | null; by: ItemBy | null; at: string | null }) {
  const howText = how ? W.gallery.how[how] ?? null : null
  if (!howText && by?.kind !== 'ai' && !at) return null
  return (
    <span className="item-did">
      {howText && <span className="item-did-how">{howText}</span>}
      {by?.kind === 'ai' && (
        <span className="item-did-by">
          <ProviderMark provider={MARK[by.provider]} className="item-did-mark" />
          {by.model ? <ModelName label={by.model} /> : PROVIDER_NAME[by.provider]}
        </span>
      )}
      {at && <time dateTime={at}>{itemTimeFormat.format(new Date(at))}</time>}
    </span>
  )
}

/**
 * 오른쪽 패널의 첫 탭(홈): 지금 연 채팅방에서 다룬 문서와 미디어만. 둘 다 있으면 위에 탭 둘(문서 / 미디어).
 * 문서는 누르면 패널 안에서 열리고, 미디어는 패널 폭에 맞는 작은 갤러리(누르면 크게).
 */
function ChatItemsView({ items, onOpenZet, onOpenChat }: {
  items: ChatItems | null
  onOpenZet: (id: string) => void
  onOpenChat?: (chatId: string) => void
}) {
  const [view, setView] = useState<'documents' | 'media'>('documents')
  if (!items) return <PaneMessage text={W.zetPanel.text26} />
  const hasDocs = items.documents.length > 0
  const hasMedia = items.media.length > 0
  if (!hasDocs && !hasMedia) return <PaneMessage text={W.gallery.chatEmpty} />
  const current = hasDocs && hasMedia ? view : hasDocs ? 'documents' : 'media'
  const docs = [...items.documents].sort((a: ChatDocumentItem, b: ChatDocumentItem) => (b.at ?? b.updatedAt ?? '').localeCompare(a.at ?? a.updatedAt ?? ''))
  return (
    <div className="chat-items">
      {hasDocs && hasMedia && (
        <div className="chat-items-tabs" role="tablist">
          {(['documents', 'media'] as const).map((id) => (
            <button key={id} type="button" role="tab" aria-selected={current === id} className={current === id ? 'is-active' : undefined} onClick={() => setView(id)}>
              {id === 'documents' ? W.document : W.mediaView.aria1}
              <span className="chat-items-count">{id === 'documents' ? items.documents.length : items.media.length}</span>
            </button>
          ))}
        </div>
      )}
      {current === 'documents' ? (
        <ul className="zet-rows chat-item-rows">
          {docs.map((entry) => (
            <li key={`${entry.id}-${entry.how}`}>
              <button className="zet-row" type="button" onClick={() => onOpenZet(entry.id)}>
                <strong>{entry.title}</strong>
                {entry.summary && <span className="zet-row-summary">{entry.summary}</span>}
                <span className="zet-row-meta"><ItemDid how={entry.how} by={entry.by} at={entry.at} /></span>
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <div className="chat-items-media">
          <MediaGallery items={items.media} rowHeight={104} groupByDay={false} onOpenChat={onOpenChat} />
        </div>
      )}
    </div>
  )
}

/** 요약 아래에 이어 보이는, 그 요약이 요약한 글(보통 보정된 전사). 제목을 누르면 그 글이 따로 탭으로 열린다(고치기는 거기서). */
function SummarizedBelow({ targets, onOpenZet, onOpenAddress }: { targets: string[]; onOpenZet: (id: string) => void; onOpenAddress: (address: string) => void }) {
  const [docs, setDocs] = useState<Array<{ id: string; title: string; kind: ZetKind; body: string }> | null>(null)
  const key = targets.join(',')
  useEffect(() => {
    let alive = true
    if (!key) { setDocs([]); return }
    Promise.all(key.split(',').map((id) => readZet(id).then((doc) => ({ id, title: doc.head.title, kind: doc.head.kind, body: doc.body })).catch(() => null)))
      .then((list) => { if (alive) setDocs(list.filter((item): item is NonNullable<typeof item> => item !== null)) })
    return () => { alive = false }
  }, [key])
  if (!docs || docs.length === 0) return null
  return (
    <>
      {docs.map((doc) => (
        <section key={doc.id} className="zet-summarized" aria-label={doc.title}>
          <header className="zet-summarized-head">
            <span className="zet-summarized-kind">{KIND_LABEL[doc.kind] ?? doc.kind}</span>
            <button type="button" className="zet-summarized-title" onClick={() => onOpenZet(doc.id)}>{doc.title}</button>
          </header>
          {doc.body.trim() ? <ZetMarkdown text={doc.body} onOpenZet={onOpenZet} onOpenAddress={onOpenAddress} /> : <PaneMessage text={W.zetPanel.text46} />}
        </section>
      ))}
    </>
  )
}

function PaneMessage({ text, onRetry }: { text: string; onRetry?: () => void }) {
  return (
    <div className="zet-message" role={onRetry ? 'alert' : 'status'}>
      <span>{text}</span>
      {onRetry && (
        <button className="secondary-button" type="button" onClick={onRetry}>
          <RefreshCw size={13} aria-hidden />{W.zetPanel.text25}</button>
      )}
    </div>
  )
}

function ZetListView({
  list,
  onOpen,
  onReload,
}: {
  list: Loadable<ZetListEntry[]>
  onOpen: (id: string) => void
  onReload: () => void
}) {
  const [query, setQuery] = useState('')

  if (list.state === 'error') return <PaneMessage text={list.message} onRetry={onReload} />
  if (list.state !== 'ready') return <PaneMessage text={W.zetPanel.text26} />
  if (list.value.length === 0) return <PaneMessage text={W.zetPanel.text27} />

  const normalized = query.trim().toLocaleLowerCase('ko-KR')
  const visible = normalized
    ? list.value.filter((entry) => (
        `${entry.title} ${entry.summary}`.toLocaleLowerCase('ko-KR').includes(normalized)
      ))
    : list.value

  return (
    <div className="zet-list">
      <label className="zet-search">
        <Search size={15} aria-hidden />
        <span className="sr-only">{W.zetPanel.text28}</span>
        <input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder={W.zetPanel.text28}
          type="search"
        />
      </label>
      {visible.length === 0 ? (
        <PaneMessage text={W.zetPanel.text29} />
      ) : (
        <ul className="zet-rows">
          {visible.map((entry) => (
            <li key={entry.id}>
              <button className="zet-row" type="button" onClick={() => onOpen(entry.id)}>
                <strong>{entry.title}</strong>
                <span className="zet-row-summary">{entry.summary}</span>
                <span className="zet-row-meta">
                  <span>{KIND_LABEL[entry.kind] ?? entry.kind}</span>
                  <span className={`zet-status ${entry.status}`}>{STATUS_LABEL[entry.status] ?? entry.status}</span>
                  <time dateTime={entry.updatedAt}>{formatDay(entry.updatedAt)}</time>
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

type DocumentView = 'body' | 'linked' | 'history'

const DOCUMENT_VIEWS: Array<{ id: DocumentView; label: string }> = [
  { id: 'body', label: W.zetPanel.text30 },
  { id: 'linked', label: W.zetPanel.text31 },
  { id: 'history', label: W.zetPanel.text32 },
]

type ZetDraft = {
  title: string
  summary: string
  body: string
}

function ZetDocumentView({
  id,
  tabKey,
  onEditor,
  onDirtyChange,
  onSaved,
  onOpenZet,
  onOpenReceipt,
  onOpenAddress,
  onTitle,
  onRestored,
}: {
  id: string
  tabKey: string
  onEditor: (key: string, editor: ZetEditor | null) => void
  onDirtyChange: (key: string, dirty: boolean) => void
  onSaved: () => void
  onOpenZet: (id: string) => void
  onOpenReceipt: (id: string) => void
  onOpenAddress: (address: string) => void
  onTitle: (id: string, title: string) => void
  onRestored: () => void
}) {
  const viewsId = useId()
  const [view, setView] = useState<DocumentView>('body')
  const [seen, setSeen] = useState({ linked: false, history: false })
  const [version, setVersion] = useState<ZetHistoryEntry | null>(null)
  // 고치는 중이면 고치고 있는 제목·요약·본문이 들어 있다. 고치는 중이 아니면 null.
  const [draft, setDraft] = useState<ZetDraft | null>(null)
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [fieldErrors, setFieldErrors] = useState<{ title?: string; summary?: string }>({})
  const [askingDiscard, setAskingDiscard] = useState(false)
  // 저장하려는데 글이 그사이 다른 곳에서 바뀌어 있었을 때 띄우는 물음.
  const [changedMeanwhile, setChangedMeanwhile] = useState(false)
  // 고치기 시작할 때 읽은 글의 지문. 저장할 때 돌려줘서 그사이 바뀌었는지 알아본다.
  const baseHash = useRef('')

  const [doc, reloadDocument] = useLoadable(() => readZet(id), W.zetPanel.text33, true, [id])
  const [linked, reloadLinked] = useLoadable(() => readZetLinked(id), W.zetPanel.text34, seen.linked, [id])
  const [history, reloadHistory] = useLoadable(() => readZetHistory(id), W.zetPanel.text35, seen.history, [id])

  const current = doc.state === 'ready' ? doc.value : null
  const title = current?.head.title ?? null
  useEffect(() => {
    if (title) onTitle(id, title)
  }, [id, onTitle, title])

  const dirty = draft !== null && current !== null && (
    draft.title !== current.head.title
    || draft.summary !== current.head.summary
    || draft.body !== current.body
  )

  useEffect(() => {
    onDirtyChange(tabKey, dirty)
  }, [dirty, onDirtyChange, tabKey])
  useEffect(() => () => onDirtyChange(tabKey, false), [onDirtyChange, tabKey])

  function stopEditing() {
    setDraft(null)
    setSaveError(null)
    setFieldErrors({})
    setAskingDiscard(false)
    setChangedMeanwhile(false)
  }

  // 저장: 파일에 쓰고 이력에 남긴다(zet_save 한 번). 저장했으면 true.
  // overwrite 는 그사이 바뀐 글을 내가 고친 것으로 덮어쓸 때만 true 다(사람이 그러라고 한 뒤).
  async function save(overwrite = false): Promise<boolean> {
    if (!draft || !current || saving) return false
    const nextTitle = draft.title.trim()
    const nextSummary = draft.summary.trim()
    const errors = {
      ...(nextTitle ? {} : { title: W.zetPanel.text36 }),
      ...(nextSummary ? {} : { summary: W.zetPanel.text37 }),
    }
    setFieldErrors(errors)
    if (errors.title || errors.summary) return false
    if (!dirty && !overwrite) {
      stopEditing()
      return true
    }

    setSaving(true)
    setSaveError(null)
    setChangedMeanwhile(false)
    try {
      // 덮어쓸 때는 지금 파일의 지문을 새로 읽어서 준다.
      if (overwrite) baseHash.current = (await readZet(id)).contentHash
      await saveZet(id, { title: nextTitle, summary: nextSummary, body: draft.body }, baseHash.current)
    } catch (reason) {
      setSaving(false)
      if (isChangedMeanwhileError(reason)) setChangedMeanwhile(true)
      else setSaveError(W.zetPanel.text38(archiveErrorMessage(reason, '')).trim())
      return false
    }

    setSaving(false)
    setDraft(null)
    setFieldErrors({})
    reloadDocument()
    if (seen.history) reloadHistory()
    if (seen.linked) reloadLinked()
    onSaved()
    return true
  }

  // 패널이 탭이나 패널을 닫을 때 쓸 수 있게 저장하는 방법과 버리는 방법을 알려 둔다.
  const editor = useRef<ZetEditor>({ save: () => save(), discard: stopEditing })
  editor.current = { save: () => save(), discard: stopEditing }
  useEffect(() => {
    onEditor(tabKey, { save: () => editor.current.save(), discard: () => editor.current.discard() })
    return () => onEditor(tabKey, null)
  }, [onEditor, tabKey])

  function selectView(next: DocumentView) {
    setView(next)
    if (next !== 'body') setSeen((previous) => ({ ...previous, [next]: true }))
    if (next !== 'history') setVersion(null)
  }

  if (doc.state === 'error') return <PaneMessage text={doc.message} onRetry={reloadDocument} />
  if (!current) return <PaneMessage text={W.zetPanel.text39} />

  const { head, body } = current
  // 가장 최근에 저장한 판. 되돌리기 전에 저장하지 않은 내용이 있는지 견줘 보는 데 쓴다.
  const latestRevision = history.state === 'ready' ? history.value[0]?.revision ?? null : null

  if (draft) {
    return (
      <article
        className="zet-document zet-document--editing"
        onKeyDown={(event) => {
          if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') {
            event.preventDefault()
            void save()
          }
        }}
      >
        <header className="zet-head">
          <input
            className="zet-edit-title"
            value={draft.title}
            aria-label={W.zetPanel.aria6}
            aria-invalid={Boolean(fieldErrors.title)}
            placeholder={W.zetPanel.aria6}
            disabled={saving}
            onChange={(event) => {
              setDraft({ ...draft, title: event.target.value })
              // 다시 적기 시작하면 알림을 지운다.
              if (fieldErrors.title) setFieldErrors({ ...fieldErrors, title: undefined })
            }}
          />
          {fieldErrors.title && <p className="zet-edit-error" role="alert">{fieldErrors.title}</p>}
          <textarea
            className="zet-edit-summary"
            value={draft.summary}
            aria-label={W.zetPanel.text1}
            aria-invalid={Boolean(fieldErrors.summary)}
            placeholder={W.zetPanel.text1}
            rows={2}
            disabled={saving}
            onChange={(event) => {
              setDraft({ ...draft, summary: event.target.value })
              if (fieldErrors.summary) setFieldErrors({ ...fieldErrors, summary: undefined })
            }}
          />
          {fieldErrors.summary && <p className="zet-edit-error" role="alert">{fieldErrors.summary}</p>}
          <ZetMeta head={head} />
        </header>

        <div className="zet-viewbar zet-viewbar--editing">
          <button className="secondary-button" type="button" disabled={saving} onClick={() => (dirty ? setAskingDiscard(true) : stopEditing())}>{W.zetPanel.text40}</button>
          <button className="confirm-button" type="button" disabled={saving} onClick={() => void save()}>
            <Check size={14} aria-hidden />{W.zetPanel.text22}</button>
        </div>

        <div className="zet-view">
          {saveError && <p className="zet-edit-error zet-edit-error--save" role="alert">{saveError}</p>}
          <textarea
            className="zet-edit-body"
            value={draft.body}
            aria-label={W.zetPanel.text30}
            placeholder={W.zetPanel.text30}
            spellCheck={false}
            disabled={saving}
            onChange={(event) => setDraft({ ...draft, body: event.target.value })}
          />
        </div>

        {changedMeanwhile && (
          <QuestionDialog
            title={W.zetPanel.title2}
            text={W.zetPanel.text41}
            working={saving}
            onDismiss={() => setChangedMeanwhile(false)}
            actions={[
              { label: W.zetPanel.text20, onSelect: () => setChangedMeanwhile(false), autoFocus: true },
              {
                label: W.zetPanel.text42,
                onSelect: () => {
                  stopEditing()
                  reloadDocument()
                  if (seen.history) reloadHistory()
                },
              },
              { label: W.zetPanel.text43, primary: true, onSelect: () => void save(true) },
            ]}
          />
        )}

        {askingDiscard && (
          <QuestionDialog
            title={W.zetPanel.title3}
            text={W.zetPanel.text44}
            onDismiss={() => setAskingDiscard(false)}
            actions={[
              { label: W.zetPanel.text20, onSelect: () => setAskingDiscard(false), autoFocus: true },
              { label: W.zetPanel.text21, primary: true, onSelect: stopEditing },
            ]}
          />
        )}
      </article>
    )
  }

  return (
    <article className="zet-document">
      <ZetHeadBlock head={head} />

      <div className="zet-viewbar">
        <div className="glass-segmented zet-views" data-active={view} role="tablist" aria-label={W.zetPanel.aria7}>
          <span className="glass-slider" aria-hidden />
          {DOCUMENT_VIEWS.map((item) => (
            <button
              key={item.id}
              id={`${viewsId}-${item.id}-tab`}
              type="button"
              role="tab"
              aria-selected={view === item.id}
              aria-controls={`${viewsId}-${item.id}`}
              className={view === item.id ? 'active' : ''}
              onClick={() => selectView(item.id)}
            >
              {item.label}
            </button>
          ))}
        </div>
        {/* 고치기는 지금 글의 본문을 보고 있을 때만 있다. 지난 판과 이어진 것을 볼 때는 없다. */}
        {view === 'body' && (
          <button
            className="secondary-button zet-edit-start"
            type="button"
            onClick={() => {
              baseHash.current = current.contentHash
              setDraft({ title: head.title, summary: head.summary, body })
            }}
          >
            <Pencil size={13} aria-hidden />{W.zetPanel.text45}</button>
        )}
      </div>

      <div className="zet-view" key={view} id={`${viewsId}-${view}`} role="tabpanel" aria-labelledby={`${viewsId}-${view}-tab`}>
        {view === 'body' && (
          body.trim()
            ? <ZetMarkdown text={body} onOpenZet={onOpenZet} onOpenAddress={onOpenAddress} />
            : <PaneMessage text={W.zetPanel.text46} />
        )}
        {/* 요약과 전사는 저장은 따로, 화면에서는 같이(방향 문서 L329): 요약이 가리키는(summarizes) 글을 요약 아래에 이어서 보인다. */}
        {view === 'body' && head.kind === 'summary' && (
          <SummarizedBelow targets={head.edges.filter((edge) => edge.type === 'summarizes').map((edge) => edge.target)} onOpenZet={onOpenZet} onOpenAddress={onOpenAddress} />
        )}

        {view === 'linked' && (
          linked.state === 'error'
            ? <PaneMessage text={linked.message} onRetry={reloadLinked} />
            : linked.state !== 'ready'
              ? <PaneMessage text={W.zetPanel.text47} />
              : <LinkedView linked={linked.value} onOpenZet={onOpenZet} onOpenReceipt={onOpenReceipt} />
        )}

        {view === 'history' && (
          version ? (
            <VersionView
              id={id}
              entry={version}
              latestRevision={latestRevision}
              onBack={() => setVersion(null)}
              onOpenZet={onOpenZet}
              onOpenAddress={onOpenAddress}
              onRestored={() => {
                setVersion(null)
                setView('body')
                reloadDocument()
                reloadHistory()
                if (seen.linked) reloadLinked()
                onRestored()
              }}
            />
          ) : history.state === 'error'
            ? <PaneMessage text={history.message} onRetry={reloadHistory} />
            : history.state !== 'ready'
              ? <PaneMessage text={W.zetPanel.text48} />
              : history.value.length === 0
                ? <PaneMessage text={W.zetPanel.text49} />
                : (
                  <ul className="zet-links">
                    {history.value.map((entry) => {
                      // 그때의 제목은 지금 제목과 다를 때만 보여 준다(같은 제목을 줄마다 되풀이하지 않는다).
                      const notes = [
                        entry.title && entry.title !== head.title ? W.zetPanel.text50(entry.title) : null,
                        entry.otherZets > 0 ? W.zetPanel.text51(entry.otherZets) : null,
                      ].filter(Boolean)
                      return (
                        <li key={entry.revision}>
                          <button className="zet-link-row" type="button" onClick={() => setVersion(entry)}>
                            <strong>{formatDateTimeSeconds(entry.time)}</strong>
                            {notes.map((note) => <span key={note}>{note}</span>)}
                          </button>
                        </li>
                      )
                    })}
                  </ul>
                )
        )}
      </div>
    </article>
  )
}

export type QuestionAction = {
  label: string
  onSelect: () => void
  primary?: boolean
  autoFocus?: boolean
}

/** 한 번 묻는 창. 창 전체 위에 뜨고, Esc 나 바깥을 누르면 onDismiss 가 불린다. */
export function QuestionDialog({
  title,
  text,
  actions,
  working = false,
  onDismiss,
}: {
  title: string
  text: string
  actions: QuestionAction[]
  working?: boolean
  onDismiss: () => void
}) {
  const titleId = useId()
  const textId = useId()
  const dialog = useRef<HTMLDivElement>(null)

  function handleKeys(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.key === 'Escape' && !working) {
      event.stopPropagation()
      onDismiss()
      return
    }
    if (event.key !== 'Tab') return
    // 창이 떠 있는 동안 초점이 창 밖으로 나가지 않게 한다.
    const buttons = dialog.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')
    if (!buttons || buttons.length === 0) return
    const first = buttons[0]
    const last = buttons[buttons.length - 1]
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault()
      last.focus()
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault()
      first.focus()
    }
  }

  return createPortal(
    <div
      className="zet-confirm-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !working) onDismiss()
      }}
    >
      <div
        ref={dialog}
        className="zet-confirm"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={textId}
        onKeyDown={handleKeys}
      >
        <h2 id={titleId}>{title}</h2>
        <p id={textId}>{text}</p>
        <div className="zet-confirm-actions">
          {actions.map((action) => (
            <button
              key={action.label}
              className={action.primary ? 'confirm-button' : 'secondary-button'}
              type="button"
              autoFocus={action.autoFocus}
              disabled={working}
              onClick={action.onSelect}
            >
              {action.label}
            </button>
          ))}
        </div>
      </div>
    </div>,
    document.body,
  )
}

function ZetMeta({ head }: { head: ZetDocument['head'] }) {
  return (
    <dl className="zet-meta">
      <div>
        <dt>{W.zetPanel.text52}</dt>
        <dd>{KIND_LABEL[head.kind] ?? head.kind}</dd>
      </div>
      <div>
        <dt>{W.zetPanel.text53}</dt>
        <dd className={`zet-status ${head.status}`}>{STATUS_LABEL[head.status] ?? head.status}</dd>
      </div>
      <div>
        <dt>{W.zetPanel.text54}</dt>
        <dd><time dateTime={head.created_at}>{formatDateTime(head.created_at)}</time></dd>
      </div>
      <div>
        <dt>{W.zetPanel.text55}</dt>
        <dd><time dateTime={head.updated_at}>{formatDateTime(head.updated_at)}</time></dd>
      </div>
    </dl>
  )
}

function ZetHeadBlock({ head }: { head: ZetDocument['head'] }) {
  return (
    <header className="zet-head">
      <h1>{head.title}</h1>
      <p>{head.summary}</p>
      <ZetMeta head={head} />
    </header>
  )
}

type ZetReference = Pick<ZetLinkedEntry, 'id' | 'title' | 'zetKind' | 'status' | 'missing'>

// 다른 글을 가리키는 줄. 연결의 종류(label)는 제목 아래에 둔다(제목 위에 작은 글씨를 올리지 않는다).
function LinkedZetRow({
  label,
  entry,
  onOpenZet,
}: {
  label?: string
  entry: ZetReference
  onOpenZet: (id: string) => void
}) {
  // 가리키던 글이 없어졌으면 누를 수 없는 줄로, 없다는 사실만 보여 준다.
  if (entry.missing || !entry.title) {
    return (
      <div className="zet-link-row missing">
        <strong>{W.zetPanel.text56}</strong>
        {label && <span>{label}</span>}
      </div>
    )
  }
  return (
    <button className="zet-link-row" type="button" onClick={() => onOpenZet(entry.id)}>
      <strong>{entry.title}</strong>
      <span className="zet-row-meta">
        {label && <span>{label}</span>}
        {entry.zetKind && <span>{KIND_LABEL[entry.zetKind] ?? entry.zetKind}</span>}
        {entry.status && (
          <span className={`zet-status ${entry.status}`}>{STATUS_LABEL[entry.status] ?? entry.status}</span>
        )}
      </span>
    </button>
  )
}

function AssetRow({ name }: { name: string | null }) {
  return (
    <div className="zet-link-row plain">
      <strong>{name ?? W.zetPanel.text57(W.rawMaterial)}</strong>
    </div>
  )
}

function LinkedView({
  linked,
  onOpenZet,
  onOpenReceipt,
}: {
  linked: ZetLinked
  onOpenZet: (id: string) => void
  onOpenReceipt: (id: string) => void
}) {
  const zetCount = linked.outgoing.length + linked.incoming.length
  if (linked.assets.length + zetCount + linked.receipts.length === 0) {
    return <PaneMessage text={W.zetPanel.text58} />
  }

  return (
    <div className="zet-linked">
      {linked.assets.length > 0 && (
        <section>
          <h2>{W.rawMaterial}</h2>
          <ul className="zet-links">
            {linked.assets.map((asset) => (
              <li key={asset.id}><AssetRow name={asset.originalFileName} /></li>
            ))}
          </ul>
        </section>
      )}

      {zetCount > 0 && (
        <section>
          <h2>{W.zetPanel.text59}</h2>
          <ul className="zet-links">
            {linked.outgoing.map((edge) => (
              <li key={`out-${edge.type}-${edge.id}`}>
                <LinkedZetRow
                  label={OUTGOING_LABEL[edge.type] ?? W.zetPanel.text9}
                  entry={edge}
                  onOpenZet={onOpenZet}
                />
              </li>
            ))}
            {linked.incoming.map((edge) => (
              <li key={`in-${edge.type}-${edge.id}`}>
                <LinkedZetRow
                  label={INCOMING_LABEL[edge.type] ?? W.zetPanel.text14}
                  entry={edge}
                  onOpenZet={onOpenZet}
                />
              </li>
            ))}
          </ul>
        </section>
      )}

      {linked.receipts.length > 0 && (
        <section>
          <h2>{W.workLog}</h2>
          <ul className="zet-links">
            {linked.receipts.map((receipt) => (
              <li key={receipt.id}>
                <button className="zet-link-row" type="button" onClick={() => onOpenReceipt(receipt.id)}>
                  <strong>{formatDateTime(receipt.createdAt)}</strong>
                  <span>
                    {[TOOL_LABEL[receipt.tool] ?? receipt.tool, receipt.model].filter(Boolean).join(' · ')}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  )
}

function VersionView({
  id,
  entry,
  latestRevision,
  onBack,
  onOpenZet,
  onOpenAddress,
  onRestored,
}: {
  id: string
  entry: ZetHistoryEntry
  latestRevision: string | null
  onBack: () => void
  onOpenZet: (id: string) => void
  onOpenAddress: (address: string) => void
  onRestored: () => void
}) {
  const [version, reloadVersion] = useLoadable(
    () => readZetVersion(id, entry.revision),
    W.zetPanel.text35,
    true,
    [id, entry.revision],
  )
  const [confirming, setConfirming] = useState(false)
  // 제목과 요약을 읽을 수 없는 판은 되돌릴 수 없어서 되돌리기 단추를 두지 않는다.
  const restorable = version.state === 'ready' && !version.value.headUnreadable && version.value.head !== null

  return (
    <div className="zet-version">
      <div className="zet-version-bar">
        <button className="secondary-button" type="button" onClick={onBack}>
          <ArrowLeft size={13} aria-hidden />{W.zetPanel.text60}</button>
        {restorable && (
          <button className="secondary-button" type="button" onClick={() => setConfirming(true)}>
            <Undo2 size={13} aria-hidden />{W.zetPanel.text61}</button>
        )}
      </div>

      {version.state === 'error' ? (
        <PaneMessage text={version.message} onRetry={reloadVersion} />
      ) : version.state !== 'ready' ? (
        <PaneMessage text={W.zetPanel.text48} />
      ) : version.value.head && !version.value.headUnreadable ? (
        <>
          <header className="zet-version-head">
            <h2>{version.value.head.title}</h2>
            <p>{version.value.head.summary}</p>
            <p className="zet-version-time">{W.zetPanel.text62(formatDateTimeSeconds(entry.time))}</p>
          </header>
          {version.value.body.trim()
            ? <ZetMarkdown text={version.value.body} onOpenZet={onOpenZet} onOpenAddress={onOpenAddress} />
            : <PaneMessage text={W.zetPanel.text46} />}
        </>
      ) : (
        <>
          <header className="zet-version-head">
            <p>{W.zetPanel.text63}</p>
            <p className="zet-version-time">{W.zetPanel.text62(formatDateTimeSeconds(entry.time))}</p>
          </header>
          <pre className="zet-version-raw">{version.value.body}</pre>
        </>
      )}

      {confirming && (
        <RestoreConfirm
          id={id}
          entry={entry}
          latestRevision={latestRevision}
          onCancel={() => setConfirming(false)}
          onRestored={onRestored}
        />
      )}
    </div>
  )
}

function sameHead(a: ZetDocument['head'], b: ZetDocument['head']) {
  const pick = (head: ZetDocument['head']) => JSON.stringify([
    head.title, head.summary, head.kind, head.status, head.created_at, head.updated_at, head.assets, head.edges,
  ])
  return pick(a) === pick(b)
}

/** 이 글에 저장하지 않은 내용이 있는지: 지금 글을 가장 최근에 저장한 판과 견준다. 알 수 없으면 false. */
async function hasUnsavedContent(id: string, latestRevision: string | null) {
  if (!latestRevision) return false
  try {
    const [current, saved] = await Promise.all([readZet(id), readZetVersion(id, latestRevision)])
    if (!saved.head) return false
    return current.body !== saved.body || !sameHead(current.head, saved.head)
  } catch {
    return false
  }
}

/**
 * 되돌리기 확인 창. 한 번만 묻는다.
 * 저장하지 않은 내용이 없으면 "이 판으로 되돌릴까요?"를, 있으면 그 내용이 사라진다는 것을 묻는다.
 * 되돌리기는 먼저 그냥 부르고, 저장하지 않은 내용이 있다는 답이 오면 사람이 이미 그러라고 했을 때만 다시 부른다.
 */
function RestoreConfirm({
  id,
  entry,
  latestRevision,
  onCancel,
  onRestored,
}: {
  id: string
  entry: ZetHistoryEntry
  latestRevision: string | null
  onCancel: () => void
  onRestored: () => void
}) {
  const titleId = useId()
  const textId = useId()
  const dialog = useRef<HTMLDivElement>(null)
  // checking: 저장하지 않은 내용이 있는지 보는 중(창을 아직 띄우지 않는다).
  const [question, setQuestion] = useState<'checking' | 'restore' | 'discard'>('checking')
  const [working, setWorking] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let alive = true
    void hasUnsavedContent(id, latestRevision).then((unsaved) => {
      if (alive) setQuestion(unsaved ? 'discard' : 'restore')
    })
    return () => {
      alive = false
    }
  }, [id, latestRevision])

  async function restore() {
    setWorking(true)
    setError(null)
    try {
      await restoreZetVersion(id, entry.revision)
      onRestored()
      return
    } catch (reason) {
      if (!isUnsavedError(reason)) {
        setError(archiveErrorMessage(reason, W.zetPanel.text64))
        setWorking(false)
        return
      }
    }
    // 저장하지 않은 내용이 있다는 답이 왔다. 아직 그것을 묻지 않았으면 같은 창에서 묻는다.
    if (question !== 'discard') {
      setQuestion('discard')
      setWorking(false)
      return
    }
    try {
      await restoreZetVersion(id, entry.revision, true)
      onRestored()
    } catch (reason) {
      setError(archiveErrorMessage(reason, W.zetPanel.text64))
      setWorking(false)
    }
  }

  function handleKeys(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.key === 'Escape' && !working) {
      event.stopPropagation()
      onCancel()
      return
    }
    if (event.key !== 'Tab') return
    // 확인 창이 떠 있는 동안 초점이 창 밖으로 나가지 않게 한다.
    const buttons = dialog.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')
    if (!buttons || buttons.length === 0) return
    const first = buttons[0]
    const last = buttons[buttons.length - 1]
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault()
      last.focus()
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault()
      first.focus()
    }
  }

  if (question === 'checking') return null

  const savedAt = formatDateTimeSeconds(entry.time)

  // 패널 안의 움직임 효과에 갇히지 않도록 창 전체 위에 띄운다.
  return createPortal(
    <div
      className="zet-confirm-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !working) onCancel()
      }}
    >
      <div
        ref={dialog}
        className="zet-confirm"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={textId}
        onKeyDown={handleKeys}
      >
        {question === 'discard' ? (
          <>
            <h2 id={titleId}>{W.zetPanel.text65}</h2>
            <p id={textId}>{W.zetPanel.text66(savedAt)}</p>
          </>
        ) : (
          <>
            <h2 id={titleId}>{W.zetPanel.text67}</h2>
            <p id={textId}>{W.zetPanel.text68(savedAt)}</p>
          </>
        )}
        {error && <p className="zet-confirm-error" role="alert">{error}</p>}
        <div className="zet-confirm-actions">
          <button className="secondary-button" type="button" autoFocus disabled={working} onClick={onCancel}>{W.zetPanel.text69}</button>
          <button className="confirm-button" type="button" disabled={working} onClick={() => void restore()}>
            {question === 'discard' ? W.zetPanel.text70 : W.zetPanel.text61}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  )
}

function ReceiptView({
  id,
  onOpenZet,
  onOpenAddress,
}: {
  id: string
  onOpenZet: (id: string) => void
  onOpenAddress: (address: string) => void
}) {
  const [receipt, reloadReceipt] = useLoadable(() => readReceipt(id), W.zetPanel.text71(W.workLog), true, [id])

  if (receipt.state === 'error') return <PaneMessage text={receipt.message} onRetry={reloadReceipt} />
  if (receipt.state !== 'ready') return <PaneMessage text={W.zetPanel.text72(W.workLog)} />

  const { head, body } = receipt.value

  // 읽은 것과 쓴 것은 백엔드가 제목과 파일 이름을 붙여서 준다.
  function reference(item: ReceiptReference) {
    return item.type === 'zet'
      ? <LinkedZetRow entry={item} onOpenZet={onOpenZet} />
      : <AssetRow name={item.originalFileName} />
  }

  return (
    <article className="zet-document">
      <header className="zet-head">
        <h1>{W.workLog}</h1>
        <dl className="zet-meta">
          <div>
            <dt>{W.zetPanel.text73}</dt>
            <dd>{TOOL_LABEL[head.tool] ?? head.tool}</dd>
          </div>
          {head.model && (
            <div>
              <dt>{W.zetPanel.text74}</dt>
              <dd>{head.model}</dd>
            </div>
          )}
          <div>
            <dt>{W.zetPanel.text75}</dt>
            <dd><time dateTime={head.created_at}>{formatDateTime(head.created_at)}</time></dd>
          </div>
        </dl>
      </header>

      <div className="zet-view">
        <div className="zet-linked zet-linked--document">
          {head.read.length > 0 && (
            <section>
              <h2>{W.zetPanel.text76}</h2>
              <ul className="zet-links">
                {head.read.map((item) => <li key={`${item.type}-${item.id}`}>{reference(item)}</li>)}
              </ul>
            </section>
          )}
          {head.wrote.length > 0 && (
            <section>
              <h2>{W.zetPanel.text77}</h2>
              <ul className="zet-links">
                {head.wrote.map((item) => <li key={`${item.type}-${item.id}`}>{reference(item)}</li>)}
              </ul>
            </section>
          )}
        </div>
        <ZetMarkdown text={body} onOpenZet={onOpenZet} onOpenAddress={onOpenAddress} />
      </div>
    </article>
  )
}
