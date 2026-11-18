import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from 'react'
import { createPortal } from 'react-dom'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { ArrowLeft, Check, ChevronRight, FileText, Folder, Globe, PanelRight, Pencil, RefreshCw, Search, Undo2 } from 'lucide-react'
import { PanelBrowserView } from './PanelBrowser'
import { usePanelBrowserHold } from '../panel-browser'
import { observeSize } from '../resize-later'
import { MenuItem, RowMenu } from './ChatSidebar'
import { SourceDeleteDialog, type SourceNotice } from './SourceDelete'
import { openSourceAsset } from '../source-assets'
import { isFirstReceiveRefusal, isFirstReceiving } from '../restore-progress'
import {
  archiveErrorMessage,
  isChangedMeanwhileError,
  isUnsavedError,
  listZets,
  deleteReceipt,
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
import { readAttachmentText } from '../chat-items'
import { ZetMarkdown } from './ZetMarkdown'
import './ZetPanel.css'
import { W } from '../words'
import { ModelName } from './ModelName'
import { PROVIDER_NAME } from '../ai-models'
import type { ChatDocumentItem, ChatItems, ItemBy } from '../chat-items'
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
// 창이 아주 좁을 때(화면 반쪽 맞춤 등, 2026-11-18 실제 앱 점검): 패널은 200 까지 더 줄어 대화 칸에 300 을 남긴다.
const PANEL_WIDTH_NARROW = 200
const CENTER_WIDTH_NARROW = 300
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

  // 앱이 첫 실행 확인 화면부터 그리면 처음에는 이 칸이 없다. 그때 한 번만 재면 너비를 영영 몰라 패널이 가장 좁은 너비에 갇힌다(2026-11-17 찾음).
  // 그래서 칸이 생기거나 바뀔 때마다 다시 붙여 잰다.
  const observed = useRef<HTMLDivElement | null>(null)
  const stopObserving = useRef<(() => void) | null>(null)
  useEffect(() => {
    const element = bodyRef.current
    if (element === observed.current) return
    stopObserving.current?.()
    observed.current = element
    stopObserving.current = null
    if (!element) return
    // 알림 안에서 곧바로 너비를 바꾸면 패널 폭이 같은 그림 안에서 칸을 다시 바꿔 경고가 났다(2026-11-18). 다음 그림으로 미룬다.
    setBodyWidth(element.getBoundingClientRect().width)
    stopObserving.current = observeSize(element, (entry) => setBodyWidth(entry.contentRect.width))
  })
  // 떼어 낼 때(개발 모드의 두 번 붙이기 포함) 잰 칸도 잊어야 다시 붙일 때 새로 잰다.
  useEffect(() => () => { stopObserving.current?.(); stopObserving.current = null; observed.current = null }, [])

  const room = bodyWidth - leftWidth(bodyWidth, listHidden)
  const minWidth = room - CENTER_WIDTH_MIN >= PANEL_WIDTH_MIN
    ? PANEL_WIDTH_MIN
    : clamp(room - CENTER_WIDTH_NARROW, PANEL_WIDTH_NARROW, PANEL_WIDTH_MIN)
  const maxWidth = Math.max(minWidth, room - CENTER_WIDTH_MIN)
  const width = clamp(preferredWidth, minWidth, maxWidth)

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
    const clamped = clamp(Math.round(next), minWidth, maxWidth)
    setPreferredWidth(clamped)
    // 좁은 창에서만 되는 너비(240 아래)는 기억하지 않는다. 창을 넓히면 보통 너비로 돌아온다.
    if (clamped < PANEL_WIDTH_MIN) return
    try {
      window.localStorage.setItem(PANEL_WIDTH_KEY, String(clamped))
    } catch {
      // 너비를 기억하지 못해도 패널은 그대로 쓴다.
    }
  }, [minWidth, maxWidth])

  return { bodyRef, width, minWidth, maxWidth, moving: (moving || changed) && !resizing, resizing, setResizing, changeWidth }
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
  | { key: string; kind: 'file'; id: string; title: string; scope: string }
  /** 브라우저 탭(2026-11-17): 앱 전체에 하나. 채팅방을 바꿔도 남는다(scope 'web'). seq 가 바뀌면 같은 주소라도 다시 연다. */
  /** url 이 null 이면 AI 가 열어 달라고 한 빈 패널(panel_browser_open_agent). */
  | { key: 'web'; kind: 'web'; url: string | null; seq: number; title: string | null; host: string | null; scope: 'web' }

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
  /** 좁은 창에서는 240 아래로 내려간다(useZetPanelFrame). */
  minWidth?: number
  maxWidth: number
  onWidthChange: (width: number) => void
  onResizingChange: (resizing: boolean) => void
  onClose: () => void
  /** 패널을 여닫는 단추를 눌렀을 때. 패널이 열려 있는 동안 단추는 탭 줄 끝에 있다. */
  onToggle: () => void
  onNotice: SourceNotice
  /** 사진·소리·영상 원자료를 열 때 미디어 화면으로. */
  onOpenMedia?: () => void
  /** 지금 연 채팅방(홈의 패널). 새 채팅방이면 null. */
  chatId?: string | null
  /** 그 채팅방에서 다룬 문서와 미디어. undefined 면 명령이 없어 예전처럼 문서 전체를 보인다. null 이면 읽는 중. */
  chatItems?: ChatItems | null
  /** 이 채팅방에서 다룬 것을 읽지 못했다. */
  chatItemsFailed?: boolean
  onRetryChatItems?: () => void
  /** 미디어를 크게 보다가 채팅방 이름을 눌렀을 때. */
  onOpenChat?: (chatId: string) => void
}

export function ZetPanel({
  open,
  variant = 'side',
  width,
  minWidth = PANEL_WIDTH_MIN,
  maxWidth,
  onWidthChange,
  onResizingChange,
  onClose,
  onToggle,
  onNotice,
  chatId = null,
  chatItems,
  chatItemsFailed = false,
  onRetryChatItems,
  onOpenMedia,
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
  // 브라우저 탭은 어느 채팅방에서나 보인다(로그인하다 채팅방을 바꿔도 끊기지 않게).
  const shown = tabs.filter((tab) => tab.scope === scope || tab.kind === 'web')
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
  const dirtyKeysRef = useRef(dirtyKeys)
  dirtyKeysRef.current = dirtyKeys
  // 오른쪽 패널(side)은 문서 하나와 브라우저 하나만 둔다(2026-11-18 그의 말: "우측패널에 브라우저, 그 문서보는거 그정도"). 탭 줄 대신 둘 사이를 오가는 단추.
  // 문서 하나씩만(오른쪽 패널과 문서 화면 모두). 다른 문서를 열면 고친 내용이 없는 앞 문서를 바꾼다.
  const single = true
  // 문서 화면의 지금 폴더(몸소 폴더 안 경로, 맨 위는 ''). 문서를 읽다 돌아오면 그 폴더로.
  const [folder, setFolder] = useState('')
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
      // 개발 모드의 두 번 실행이나 연이은 열기에서 같은 목록 탭이 두 번 들어가지 않게 지금 목록으로 다시 본다(같은 key 경고, 2026-11-18).
      setTabs((current) => (current.some((tab) => tab.key === first.key) ? current : [...current, first]))
      // 링크로 패널을 열었으면 브라우저 탭이 이미 골라져 있다. 그때는 고른 탭을 바꾸지 않는다.
      if (!activeKeyRef.current) setActiveKey(first.key)
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
      if (single) return [...keepForNextDocument(current, where), next]
      // 글을 고르는 탭에서 고르면 그 탭이 고른 글로 바뀐다. 글 안에서 눌렀으면 옆에 새 탭으로 열린다.
      const activeIndex = current.findIndex((tab) => tab.key === activeKeyRef.current)
      if (activeIndex >= 0 && current[activeIndex].kind === 'list') {
        return current.map((tab, index) => (index === activeIndex ? next : tab))
      }
      const insertAt = activeIndex >= 0 ? activeIndex + 1 : current.length
      return [...current.slice(0, insertAt), next, ...current.slice(insertAt)]
    })
    setActiveKey(key)
  }, [setActiveKey, single])

  // 바깥 주소는 패널의 브라우저 탭에서 연다(2026-11-17 본부: "우측 패널에서 인터넷"). 탭이 있으면 그 탭에서 이동한다.
  const openWeb = useCallback((url: string | null) => {
    setTabs((current) => {
      const web = current.find((tab) => tab.kind === 'web')
      // AI 의 열기 요청(url null)은 이미 있는 브라우저 탭을 그대로 쓴다(보던 페이지를 바꾸지 않는다).
      if (web && web.kind === 'web') return url === null ? current : current.map((tab) => (tab.kind === 'web' ? { ...web, url, seq: web.seq + 1 } : tab))
      return [...current, { key: 'web', kind: 'web', url, seq: 0, title: null, host: null, scope: 'web' }]
    })
    setActiveKey('web')
  }, [setActiveKey])
  const openAddress = useCallback((url: string) => openWeb(url), [openWeb])
  useEffect(() => {
    const onOpen = (event: Event) => {
      const detail = (event as CustomEvent<{ url: string | null }>).detail
      if (detail && (typeof detail.url === 'string' || detail.url === null)) openWeb(detail.url)
    }
    window.addEventListener('momso:open-web', onOpen)
    return () => window.removeEventListener('momso:open-web', onOpen)
  }, [openWeb])
  const setWebTitle = useCallback((title: string | null, host: string | null) => {
    setTabs((current) => current.map((tab) => (tab.kind === 'web' && (tab.title !== title || tab.host !== host) ? { ...tab, title, host } : tab)))
  }, [])

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

  // 첨부파일의 글(Codex 앱처럼 패널에서 연다). 문서와 같게: 고르는 탭에서 고르면 그 탭이 바뀌고, 이미 열려 있으면 그 탭으로.
  const openFile = useCallback((id: string, title: string) => {
    const where = scopeRef.current
    const key = `${where}|file:${id}`
    setTabs((current) => {
      if (current.some((tab) => tab.key === key)) return current
      const next: PanelTab = { key, kind: 'file', id, title, scope: where }
      if (single) return [...keepForNextDocument(current, where), next]
      const activeIndex = current.findIndex((tab) => tab.key === activeKeyRef.current)
      if (activeIndex >= 0 && current[activeIndex].kind === 'list') return current.map((tab, index) => (index === activeIndex ? next : tab))
      const insertAt = activeIndex >= 0 ? activeIndex + 1 : current.length
      return [...current.slice(0, insertAt), next, ...current.slice(insertAt)]
    })
    setActiveKey(key)
  }, [setActiveKey, single])

  // 원자료 열기(2026-11-17, source_asset_open): 글이면 첨부파일과 같은 길(패널의 새 탭), 사진·소리·영상이면 미디어 화면.
  const openSource = useCallback((id: string, name: string | null) => {
    openSourceAsset(id).then((opened) => {
      if (opened.state === 'ok' && opened.kind && opened.kind !== 'text') { onOpenMedia?.(); return }
      if (opened.state === 'not_found') { onNotice(W.gallery.fileNotFound); return }
      // 글이거나 글로 열 수 없다는 상태(not_text·invalid_encoding)면 패널 탭이 그 상태를 한 줄로 알린다.
      if (opened.state === 'ok' || opened.state === 'not_text' || opened.state === 'invalid_encoding') { openFile(id, name ?? W.rawMaterial); return }
      onNotice(W.sourceDelete.openFailed)
    }).catch(() => onNotice(W.sourceDelete.openFailed))
  }, [onNotice, onOpenMedia, openFile])

  // 문서를 하나 더 열 때 남길 탭: 다른 채팅방의 것, 목록, 브라우저, 고친 내용이 있는 문서(버리지 않게).
  function keepForNextDocument(current: PanelTab[], where: string) {
    return current.filter((tab) => tab.scope !== where || tab.kind === 'list' || tab.kind === 'web' || dirtyKeysRef.current.has(tab.key))
  }
  const activeTab = shown.find((tab) => tab.key === activeKey)
  const mode: 'documents' | 'browser' = activeTab?.kind === 'web' ? 'browser' : 'documents'
  // 문서로 돌아오면 마지막에 보던 문서(없으면 목록).
  const lastDocument = useRef<Record<string, string>>({})
  if (activeTab && activeTab.kind !== 'web') lastDocument.current[scope] = activeTab.key
  function showDocuments() {
    const last = lastDocument.current[scope]
    if (last && shown.some((tab) => tab.key === last)) setActiveKey(last)
    else showList()
  }
  function showBrowser() {
    if (shown.some((tab) => tab.kind === 'web')) setActiveKey('web')
    else openWeb(null)
  }

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
    if (tab.kind === 'file') return tab.title
    if (tab.kind === 'web') return tab.title ?? tab.host ?? (tab.url === null ? W.panelBrowser.newTab : W.panelBrowser.loading)
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
    <SourceOpenContext.Provider value={openSource}>
    <PanelNoticeContext.Provider value={onNotice}>
    <aside className="zet-panel" data-variant={variant} aria-label={W.document} aria-hidden={!open}>
      <div className="zet-panel-frame" inert={!open}>
        <div
          className="zet-panel-resizer"
          role="separator"
          aria-orientation="vertical"
          aria-label={W.zetPanel.aria1}
          aria-valuemin={Math.round(minWidth)}
          aria-valuemax={Math.round(maxWidth)}
          aria-valuenow={Math.round(width)}
          tabIndex={0}
          onPointerDown={startResize}
          onPointerMove={moveResize}
          onPointerUp={endResize}
          onPointerCancel={endResize}
          onKeyDown={resizeWithKeys}
        />

        {/* 오른쪽 패널: 위에 문서·브라우저 두 단추. 문서 화면(가득 보기)은 탭 줄 없이 폴더 목록 → 문서(2026-11-18 그의 말: "문서x 이건 필요 없는거잖아"). */}
        {variant === 'side' && (
        <header className="zet-tabbar zet-modebar">
          <div className="zet-modes" role="tablist" aria-label={W.zetPanel.aria2} onKeyDown={moveBetweenTabs}>
            <button type="button" role="tab" aria-selected={mode === 'documents'} className={mode === 'documents' ? 'is-active' : undefined} onClick={showDocuments}>
              <FileText size={14} strokeWidth={1.75} aria-hidden />{W.document}
            </button>
            <button type="button" role="tab" aria-selected={mode === 'browser'} className={mode === 'browser' ? 'is-active' : undefined} onClick={showBrowser}>
              <Globe size={14} strokeWidth={1.75} aria-hidden />{W.zetPanel.browser}
            </button>
          </div>
          {open && <ZetPanelToggle open onToggle={() => closeAll('close-panel')} />}
        </header>
        )}
        {/* 오른쪽 패널에서 문서를 보는 중이면 목록으로 돌아가는 한 줄. */}
        {single && activeTab && (activeTab.kind === 'zet' || activeTab.kind === 'file' || activeTab.kind === 'receipt') && (
          <div className="zet-backbar">
            <button className="zet-back" type="button" onClick={showList}>
              <ArrowLeft size={15} strokeWidth={1.75} aria-hidden />{variant === 'page' ? (folder ? folder.split('/').pop() : W.document) : W.zetPanel.backToList}
            </button>
          </div>
        )}

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
            <section key={tab.key} className={tab.kind === 'web' ? 'zet-pane panel-browser-pane' : 'zet-pane'} role="tabpanel" aria-label={tabTitle(tab)} hidden={(tab.kind !== 'web' && tab.scope !== scope) || tab.key !== activeKey}>
              {tab.kind === 'web' ? (
                <PanelBrowserView
                  url={tab.url}
                  seq={tab.seq}
                  chatId={chatId}
                  active={open && tab.key === activeKey}
                  onTitle={setWebTitle}
                  onOpenFailed={(code) => {
                    removeTab(tab.key)
                    // Google 로그인은 뒤쪽 이벤트가 따로 알린다.
                    if (code !== 'google_login_required') onNotice(code === 'invalid_url' ? W.panelBrowser.cannotOpen : W.panelBrowser.openFailed)
                  }}
                  onNotice={onNotice}
                />
              ) : tab.kind === 'list' ? (
                byChat && tab.scope !== 'all'
                  ? (tab.scope === scope ? <ChatItemsView items={chatItems ?? null} failed={chatItemsFailed} onRetry={onRetryChatItems} onOpenZet={openZet} onOpenFile={openFile} onNotice={onNotice} /> : null)
                  : <ZetFolderView list={list} folder={folder} onFolder={setFolder} onOpen={openZet} onReload={reloadList} />
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
              ) : tab.kind === 'file' ? (
                <AttachmentTextView assetId={tab.id} title={tab.title} onOpenZet={openZet} onOpenAddress={openAddress} />
              ) : (
                <ReceiptView id={tab.id} onOpenZet={openZet} onOpenAddress={openAddress} onDeleted={() => { removeTab(tab.key); onNotice(W.receiptDelete.done) }} onFailed={(reason) => onNotice(archiveErrorMessage(reason, W.receiptDelete.failed))} />
              )}
            </section>
          ))}
        </div>
      </div>
    </aside>
    </PanelNoticeContext.Provider>
    </SourceOpenContext.Provider>
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
 * 오른쪽 패널의 첫 탭(홈): 지금 연 채팅방에서 다룬 문서, 미디어, 첨부파일만. 둘 이상 있으면 위에 탭(문서 / 미디어 / 첨부파일).
 * 문서는 누르면 패널 안에서 열리고, 미디어는 패널 폭에 맞는 작은 갤러리(누르면 크게).
 * 첨부파일(T42 files, 글 파일 등)은 Codex 앱의 "소스" 패널처럼 파일 이름과 "첨부", 때를 한 줄씩 보이고,
 * 누르면 그 글이 패널의 새 탭으로 열린다(attachment_text_read, 2026-11-16 그의 말 "txt나 md가 오른쪽 패널에 보여야").
 */
function ChatItemsView({ items, failed = false, onRetry, onOpenZet, onOpenFile, onNotice }: {
  items: ChatItems | null
  failed?: boolean
  onRetry?: () => void
  onOpenZet: (id: string) => void
  onOpenFile: (id: string, title: string) => void
  onNotice?: SourceNotice
}) {
  // 잘못 첨부한 파일 지우기(첨부파일 줄의 메뉴). 지우거나 되살리면 이 채팅방에서 다룬 것을 다시 읽는다.
  const [deleting, setDeleting] = useState<{ id: string; name: string } | null>(null)
  // 읽지 못했으면 "불러오는 중"에 멈춰 있지 않게 그 줄과 다시 불러오기, 읽는 동안은 "불러오는 중"(2026-11-17 본부 결정).
  if (!items && failed) return <PaneMessage text={W.zetPanel.chatItemsFailed} onRetry={onRetry} />
  if (!items) return <PaneMessage text={W.zetPanel.chatItemsLoading} />
  // 사진과 동영상은 따로 화면이 있어 패널에서는 문서와 첨부파일만 보인다(2026-11-18 그의 말: 패널은 브라우저와 문서 보기).
  if (items.documents.length === 0 && items.files.length === 0) return <PaneMessage text={W.gallery.chatEmpty} />
  const docs = [...items.documents].sort((a: ChatDocumentItem, b: ChatDocumentItem) => (b.at ?? b.updatedAt ?? '').localeCompare(a.at ?? a.updatedAt ?? ''))
  const files = [...items.files].sort((a, b) => (b.at ?? b.createdAt ?? '').localeCompare(a.at ?? a.createdAt ?? ''))
  // 문서와 첨부파일을 한 목록에(문서 먼저). 위쪽의 "문서" 단추와 같은 말이 두 번 보이지 않게 칸 고르기를 두지 않는다(2026-11-18).
  return (
    <div className="chat-items">
      {docs.length > 0 && (
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
      )}
      {files.length > 0 && (
        <>
          {docs.length > 0 && <h3 className="chat-items-heading">{W.gallery.files}</h3>}
          <ul className="zet-rows chat-item-rows chat-file-rows">
            {files.map((entry) => (
              <li key={entry.id} className="chat-file-item">
                <button className="zet-row chat-file-row" type="button" onClick={() => onOpenFile(entry.id, entry.title)}>
                  <span className="chat-file-name"><FileText size={14} strokeWidth={1.75} aria-hidden /><strong>{entry.title}</strong></span>
                  <span className="zet-row-meta"><ItemDid how={entry.how} by={entry.by} at={entry.at ?? entry.createdAt} /></span>
                </button>
                {onNotice && (
                  <RowMenu label={W.sourceDelete.menu(entry.title)}>
                    {(close) => <MenuItem close={close} onSelect={() => setDeleting({ id: entry.id, name: entry.title })}>{W.sourceDelete.trash}</MenuItem>}
                  </RowMenu>
                )}
              </li>
            ))}
          </ul>
        </>
      )}
      {deleting && onNotice && (
        <SourceDeleteDialog assetId={deleting.id} name={deleting.name} onClose={() => setDeleting(null)} onNotice={onNotice} onChanged={() => onRetry?.()} />
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

/**
 * 첨부파일의 글을 패널에서 읽기만 한다. 파일 이름은 탭에 있으니 안에 다시 적지 않는다. .md·.markdown 은 답과 같은 도구로 그리고, 그 밖의 글은 줄바꿈 그대로.
 * 큰 파일은 64KiB 씩 받는다: 글 끝 가까이 내려가면 nextCursor 로 다음 쪽을 이어 받는다(nextCursor 가 null 이면 끝).
 * not_found / not_text / invalid_encoding 은 한 줄 알림, invalid_cursor 는 그때까지 받은 글만 둔다.
 */
function AttachmentTextView({ assetId, title, onOpenZet, onOpenAddress }: {
  assetId: string
  title: string
  onOpenZet: (id: string) => void
  onOpenAddress: (address: string) => void
}) {
  const [text, setText] = useState<string | null>(null)
  const [problem, setProblem] = useState<string | null>(null)
  const [next, setNext] = useState<number | null>(null)
  const loading = useRef(false)
  const end = useRef<HTMLDivElement>(null)
  const markdown = /[.](md|markdown)$/i.test(title)

  const load = useCallback(async (cursor: number | null) => {
    if (loading.current) return
    loading.current = true
    try {
      const page = await readAttachmentText(assetId, cursor)
      if (page.state === 'ok') {
        setText((current) => (cursor ? (current ?? '') : '') + (page.content ?? ''))
        setNext(typeof page.nextCursor === 'number' ? page.nextCursor : null)
      } else if (page.state === 'invalid_cursor' && cursor) {
        setNext(null)
      } else {
        setProblem(page.state === 'not_found' ? W.gallery.fileNotFound
          : page.state === 'invalid_encoding' ? W.gallery.fileNotUtf8
            : page.state === 'not_text' ? W.gallery.fileNotText : W.gallery.fileFailed)
        setNext(null)
      }
    } catch (error) {
      if (!cursor) setProblem(archiveErrorMessage(error, W.gallery.fileFailed))
      setNext(null)
    } finally {
      loading.current = false
    }
  }, [assetId])

  useEffect(() => { void load(null) }, [load])

  // 다음 쪽이 있으면 글 끝이 보일 때 이어 받는다.
  useEffect(() => {
    const target = end.current
    if (next === null || !target) return
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) void load(next)
    }, { rootMargin: '400px' })
    observer.observe(target)
    return () => observer.disconnect()
  }, [next, load])

  if (problem) return <PaneMessage text={problem} />
  if (text === null) return <PaneMessage text={W.gallery.fileLoading} />
  return (
    <div className="attachment-text">
      {markdown
        ? <ZetMarkdown text={text} onOpenZet={onOpenZet} onOpenAddress={onOpenAddress} />
        : <pre className="attachment-text-plain">{text}</pre>}
      <div ref={end} aria-hidden="true" />
    </div>
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

type FolderNode = { name: string; path: string; children: FolderNode[]; docs: ZetListEntry[]; total: number }

/** 문서들의 folder 값으로 폴더 나무를 만든다. 폴더 값이 없으면 맨 위. 폴더는 이름 차례, 문서는 새로 고친 것부터. */
function folderTree(entries: ZetListEntry[]): FolderNode {
  const root: FolderNode = { name: '', path: '', children: [], docs: [], total: 0 }
  const byPath = new Map<string, FolderNode>([['', root]])
  const nodeFor = (path: string): FolderNode => {
    const found = byPath.get(path)
    if (found) return found
    const cut = path.lastIndexOf('/')
    const parent = nodeFor(cut < 0 ? '' : path.slice(0, cut))
    const node: FolderNode = { name: cut < 0 ? path : path.slice(cut + 1), path, children: [], docs: [], total: 0 }
    parent.children.push(node)
    byPath.set(path, node)
    return node
  }
  for (const entry of entries) {
    const path = (entry.folder ?? '').split('/').map((part) => part.trim()).filter(Boolean).join('/')
    nodeFor(path).docs.push(entry)
  }
  const finish = (node: FolderNode): number => {
    node.children.sort((a, b) => a.name.localeCompare(b.name, 'ko-KR', { numeric: true }))
    node.docs.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
    node.total = node.docs.length + node.children.reduce((sum, child) => sum + finish(child), 0)
    return node.total
  }
  finish(root)
  return root
}

function findFolder(root: FolderNode, path: string): FolderNode | null {
  if (!path) return root
  let node: FolderNode | undefined = root
  for (const part of path.split('/')) {
    node = node?.children.find((child) => child.name === part)
    if (!node) return null
  }
  return node ?? null
}

/**
 * 문서 화면(2026-11-18, 그의 말 "구글드라이브? 그런 트리구조처럼"): 왼쪽에 접고 펴는 폴더 나무, 오른쪽에 지금 폴더의 경로 줄과
 * 그 안의 폴더·문서 목록(Windows 탐색기·Finder·Drive 의 흔한 꼴). 찾기는 모든 폴더에서 하고, 결과마다 폴더를 적는다.
 * 창이 좁으면 나무를 숨기고 경로 줄로만 오간다. 폴더가 하나도 없으면 나무 없이 목록만.
 */
function ZetFolderView({
  list,
  folder,
  onFolder,
  onOpen,
  onReload,
}: {
  list: Loadable<ZetListEntry[]>
  folder: string
  onFolder: (path: string) => void
  onOpen: (id: string) => void
  onReload: () => void
}) {
  const [query, setQuery] = useState('')
  // 펼친 폴더. 처음에는 맨 위 폴더들과 지금 폴더까지.
  const [openFolders, setOpenFolders] = useState<ReadonlySet<string>>(new Set())
  const entries = list.state === 'ready' ? list.value : null
  const root = useMemo(() => folderTree(entries ?? []), [entries])
  useEffect(() => {
    if (!folder) return
    setOpenFolders((current) => {
      const next = new Set(current)
      const parts = folder.split('/')
      for (let i = 1; i <= parts.length; i += 1) next.add(parts.slice(0, i).join('/'))
      return next.size === current.size ? current : next
    })
  }, [folder])

  if (list.state === 'error') return <PaneMessage text={list.message} onRetry={onReload} />
  if (list.state !== 'ready') return <PaneMessage text={W.zetPanel.text26} />
  if (list.value.length === 0) return <PaneMessage text={W.zetPanel.text27} />

  const current = findFolder(root, folder) ?? root
  const hasFolders = root.children.length > 0
  const normalized = query.trim().toLocaleLowerCase('ko-KR')
  const found = normalized
    ? list.value
      .filter((entry) => `${entry.title} ${entry.summary}`.toLocaleLowerCase('ko-KR').includes(normalized))
      .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
    : null
  const crumbs = folder ? folder.split('/') : []
  const toggle = (path: string) => setOpenFolders((now) => {
    const next = new Set(now)
    if (next.has(path)) next.delete(path)
    else next.add(path)
    return next
  })

  const renderTree = (nodes: FolderNode[], depth: number): ReactNode => nodes.map((node) => {
    const expanded = openFolders.has(node.path)
    return (
      <li key={node.path} role="treeitem" aria-expanded={node.children.length > 0 ? expanded : undefined} aria-selected={node.path === current.path}>
        <div className={node.path === current.path ? 'zet-tree-row is-current' : 'zet-tree-row'} style={{ paddingLeft: 6 + depth * 14 }}>
          {node.children.length > 0 ? (
            <button className="zet-tree-twist" type="button" aria-label={expanded ? W.zetPanel.folderCollapse(node.name) : W.zetPanel.folderExpand(node.name)} onClick={() => toggle(node.path)}>
              <ChevronRight size={13} strokeWidth={1.75} aria-hidden className={expanded ? 'is-open' : undefined} />
            </button>
          ) : <span className="zet-tree-twist" aria-hidden />}
          <button className="zet-tree-name" type="button" onClick={() => { setQuery(''); onFolder(node.path) }}>
            <Folder size={14} strokeWidth={1.75} aria-hidden />
            <span>{node.name}</span>
          </button>
        </div>
        {expanded && node.children.length > 0 && <ul role="group">{renderTree(node.children, depth + 1)}</ul>}
      </li>
    )
  })

  const docRow = (entry: ZetListEntry, showFolder: boolean) => (
    <li key={entry.id}>
      <button className="zet-row" type="button" onClick={() => onOpen(entry.id)}>
        <strong>{entry.title}</strong>
        <span className="zet-row-summary">{entry.summary}</span>
        <span className="zet-row-meta">
          {showFolder && entry.folder && <span className="zet-row-folder"><Folder size={12} strokeWidth={1.75} aria-hidden />{entry.folder.split('/').join(' › ')}</span>}
          <span>{KIND_LABEL[entry.kind] ?? entry.kind}</span>
          <span className={`zet-status ${entry.status}`}>{STATUS_LABEL[entry.status] ?? entry.status}</span>
          <time dateTime={entry.updatedAt}>{formatDay(entry.updatedAt)}</time>
        </span>
      </button>
    </li>
  )

  return (
    <div className={hasFolders ? 'zet-folders' : 'zet-folders is-flat'}>
      {hasFolders && (
        <nav className="zet-tree" aria-label={W.zetPanel.folders}>
          <ul role="tree">
            <li role="treeitem" aria-selected={current.path === ''}>
              <div className={current.path === '' ? 'zet-tree-row is-current' : 'zet-tree-row'} style={{ paddingLeft: 6 }}>
                <span className="zet-tree-twist" aria-hidden />
                <button className="zet-tree-name" type="button" onClick={() => { setQuery(''); onFolder('') }}>
                  <FileText size={14} strokeWidth={1.75} aria-hidden />
                  <span>{W.document}</span>
                </button>
              </div>
            </li>
            {renderTree(root.children, 0)}
          </ul>
        </nav>
      )}
      <div className="zet-list zet-folder-main">
        <div className="zet-folder-bar">
          {found === null && hasFolders ? (
            <nav className="zet-crumbs" aria-label={W.zetPanel.folderPath}>
              <button type="button" className={crumbs.length === 0 ? 'is-current' : undefined} aria-current={crumbs.length === 0 ? 'page' : undefined} onClick={() => onFolder('')}>{W.document}</button>
              {crumbs.map((part, index) => {
                const path = crumbs.slice(0, index + 1).join('/')
                const last = index === crumbs.length - 1
                return (
                  <span key={path} className="zet-crumb">
                    <ChevronRight size={13} strokeWidth={1.75} aria-hidden />
                    <button type="button" className={last ? 'is-current' : undefined} aria-current={last ? 'page' : undefined} onClick={() => onFolder(path)}>{part}</button>
                  </span>
                )
              })}
            </nav>
          ) : <span />}
          <label className="zet-search">
            <Search size={15} aria-hidden />
            <span className="sr-only">{W.zetPanel.text28}</span>
            <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={W.zetPanel.text28} type="search" />
          </label>
        </div>
        {found !== null ? (
          found.length === 0 ? <PaneMessage text={W.zetPanel.text29} /> : <ul className="zet-rows">{found.map((entry) => docRow(entry, true))}</ul>
        ) : current.children.length === 0 && current.docs.length === 0 ? (
          <PaneMessage text={W.zetPanel.folderEmpty} />
        ) : (
          <ul className="zet-rows">
            {current.children.map((child) => (
              <li key={child.path}>
                <button className="zet-row zet-folder-row" type="button" onClick={() => onFolder(child.path)}>
                  <span className="zet-folder-name"><Folder size={15} strokeWidth={1.75} aria-hidden /><strong>{child.name}</strong></span>
                  <span className="zet-row-meta"><span>{W.zetPanel.folderCount(child.total)}</span></span>
                </button>
              </li>
            ))}
            {current.docs.map((entry) => docRow(entry, false))}
          </ul>
        )}
      </div>
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
  const notice = useContext(PanelNoticeContext)
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

    if (isFirstReceiving()) { setSaveError(W.restoreProgress.docBusy); return false }
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
      // 받는 동안이라 뒤쪽이 거절했으면(first_receive_in_progress) 받아 오는 동안의 그 글로.
      else if (isFirstReceiveRefusal(reason)) setSaveError(W.restoreProgress.docBusy)
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
        <div className="glass-segmented zet-views" data-active={view} role="tablist" aria-label={W.zetPanel.aria7} onKeyDown={moveBetweenTabs}>
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
              // 처음 받아 오는 동안은 뒤쪽이 문서 쓰기를 거절한다. 고치기를 열지 않고 알림 줄 하나(채팅방을 막을 때와 같은 방식).
              if (isFirstReceiving()) { notice?.(W.restoreProgress.docBusy); return }
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
              onBack={() => {
                // 이전 버전 목록의 그 줄로 초점을 돌려놓는다(키보드로 열었을 때).
                const revision = version.revision
                setVersion(null)
                window.requestAnimationFrame(() => document.querySelector<HTMLElement>(`.zet-link-row[data-revision="${CSS.escape(revision)}"]`)?.focus())
              }}
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
                          <button className="zet-link-row" type="button" data-revision={entry.revision} onClick={() => setVersion(entry)}>
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
  detail,
  onDismiss,
}: {
  title: string
  text: string
  /** 글 아래에 붙는 목록 같은 것(없으면 그리지 않는다). */
  detail?: ReactNode
  actions: QuestionAction[]
  working?: boolean
  onDismiss: () => void
}) {
  const titleId = useId()
  const textId = useId()
  const dialog = useRef<HTMLDivElement>(null)
  // 화면 가운데를 덮는 창이라, 열려 있는 동안 패널 브라우저(따로 뜨는 층)를 숨긴다.
  usePanelBrowserHold()

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
        {detail}
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

/**
 * 탭 줄에서 ← → Home End 로 옆 탭에 초점을 옮기고 그 탭을 연다(2026-11-17 키보드 점검에서 남긴 것: 패널 탭은 Tab 으로만 옮겨졌다).
 * 탭 안의 닫기 단추 등 탭이 아닌 것에 초점이 있으면 손대지 않는다.
 */
function moveBetweenTabs(event: ReactKeyboardEvent<HTMLElement>) {
  if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
  const tabs = Array.from(event.currentTarget.querySelectorAll<HTMLElement>('[role="tab"]'))
  const at = tabs.indexOf(document.activeElement as HTMLElement)
  if (at < 0 || tabs.length < 2) return
  event.preventDefault()
  const next = event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1
    : (at + (event.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length
  tabs[next].focus()
  tabs[next].click()
}

const SourceOpenContext = createContext<((id: string, name: string | null) => void) | null>(null)
/** 패널 안 깊은 곳에서 알림 줄을 띄운다(받아 오는 동안 문서를 고치려 할 때 등). */
const PanelNoticeContext = createContext<SourceNotice | null>(null)

/** 이어진 것·작업 내역의 원자료 한 줄. 누르면 연다(글은 패널의 새 탭, 사진·소리·영상은 미디어 화면). */
function AssetRow({ id, name }: { id: string; name: string | null }) {
  const open = useContext(SourceOpenContext)
  const label = name ?? W.zetPanel.text57(W.rawMaterial)
  if (!open) {
    return (
      <div className="zet-link-row plain">
        <strong>{label}</strong>
      </div>
    )
  }
  return (
    <button className="zet-link-row" type="button" onClick={() => open(id, name)}>
      <strong>{label}</strong>
    </button>
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
              <li key={asset.id}><AssetRow id={asset.id} name={asset.originalFileName} /></li>
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
    <div className="zet-version" onKeyDown={(event) => { if (event.key === 'Escape' && !confirming) { event.preventDefault(); event.stopPropagation(); onBack() } }}>
      <div className="zet-version-bar">
        {/* 열면 초점을 "목록으로" 단추에 둔다(키보드로 열었을 때 화면 밖으로 빠지지 않게). Esc 도 목록으로. */}
        <button className="secondary-button" type="button" autoFocus onClick={onBack}>
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
    if (isFirstReceiving()) { setError(W.restoreProgress.docBusy); return }
    setWorking(true)
    setError(null)
    try {
      await restoreZetVersion(id, entry.revision)
      onRestored()
      return
    } catch (reason) {
      if (!isUnsavedError(reason)) {
        setError(isFirstReceiveRefusal(reason) ? W.restoreProgress.docBusy : archiveErrorMessage(reason, W.zetPanel.text64))
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
      setError(isFirstReceiveRefusal(reason) ? W.restoreProgress.docBusy : archiveErrorMessage(reason, W.zetPanel.text64))
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
  onDeleted,
  onFailed,
}: {
  id: string
  onOpenZet: (id: string) => void
  onOpenAddress: (address: string) => void
  /** 지운 뒤(탭을 닫고 알림 줄). */
  onDeleted?: () => void
  onFailed?: (reason: unknown) => void
}) {
  const [receipt, reloadReceipt] = useLoadable(() => readReceipt(id), W.zetPanel.text71(W.workLog), true, [id])
  // 작업 내역 지우기(2026-11-17 본부 결정): 머리의 ⋯ 메뉴 "삭제" → 묻는 창 하나.
  const [asking, setAsking] = useState(false)
  const [deleting, setDeleting] = useState(false)
  async function remove() {
    setDeleting(true)
    try {
      await deleteReceipt(id)
      setAsking(false)
      onDeleted?.()
    } catch (reason) {
      setDeleting(false)
      setAsking(false)
      onFailed?.(reason)
    }
  }

  if (receipt.state === 'error') return <PaneMessage text={receipt.message} onRetry={reloadReceipt} />
  if (receipt.state !== 'ready') return <PaneMessage text={W.zetPanel.text72(W.workLog)} />

  const { head, body } = receipt.value

  // 읽은 것과 쓴 것은 백엔드가 제목과 파일 이름을 붙여서 준다.
  function reference(item: ReceiptReference) {
    return item.type === 'zet'
      ? <LinkedZetRow entry={item} onOpenZet={onOpenZet} />
      : <AssetRow id={item.id} name={item.originalFileName} />
  }

  return (
    <article className="zet-document">
      <header className="zet-head">
        <div className="receipt-head-title">
          <h1>{W.workLog}</h1>
          {onDeleted && (
            <RowMenu label={W.sourceDelete.menu(W.workLog)}>
              {(close) => <MenuItem close={close} onSelect={() => setAsking(true)}>{W.sourceDelete.trash}</MenuItem>}
            </RowMenu>
          )}
        </div>
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
      {asking && (
        <QuestionDialog
          title={W.receiptDelete.title}
          text={W.receiptDelete.text}
          working={deleting}
          actions={[
            { label: W.chatKeep.cancel, onSelect: () => setAsking(false), autoFocus: true },
            { label: W.sourceDelete.trash, primary: true, onSelect: () => void remove() },
          ]}
          onDismiss={() => setAsking(false)}
        />
      )}
    </article>
  )
}
