import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import { ActivityRail, type AppScreen } from './components/ActivityRail'
import { MediaView } from './components/MediaView'
import { SearchDialog, type SearchAction } from './components/SearchDialog'
import { useAppUpdate } from './components/AppUpdate'
import { QuestionDialog } from './components/ZetPanel'
import { restartApp } from './app-update'
import { isMissingCommand as isMissingMediaCommand } from './ai-models'
import { EMPTY_CHAT_ITEMS, attachFileToChat, chatItemCount, getChatItems, listenChatItems, type ChatItems } from './chat-items'
import { listSkills, type SkillEntry } from './skills'
import { listMedia } from './media'
import { AssistantPanel, type ChatViewMessage } from './components/AssistantPanel'
import type { ApprovalItem } from './components/ApprovalCard'
import { listenApprovals, respondApproval, type ApprovalChoice } from './approvals'
import { browserSessionStatus, listenBrowser } from './browser'
import type { SecretItem } from './components/SecretRequestCard'
import { cancelSecret, listenSecretRequests, submitSecret } from './setup-secrets'
import { ChatSidebar } from './components/ChatSidebar'
import { Composer, type PermissionState } from './components/Composer'
import { SettingsView, type SettingsSection } from './components/SettingsView'
import { WindowTitlebar } from './components/WindowTitlebar'
import { ZetPanel, ZetPanelToggle, useZetPanelFrame } from './components/ZetPanel'
import { FirstRun } from './components/FirstRun'
import { useProviderAuth } from './components/ProviderSignIn'
import { isConnected } from './provider-auth'
import { listenChatRestoreProgress, watchFirstReceive, watchStartup, type RestoreProgress, type StartupProgress } from './restore-progress'
import { watchChatArchive } from './chat-archive-status'
import { effortKorean } from './permission-labels'
import { checkFirstRun, type FirstRunCheck } from './first-run'
import './App.css'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import {
  initializeRuntime,
  isDesktopRuntime,
  openRuntimeDataRoot,
  isSelectionExpired,
  selectLocalPaths,
  type RuntimeSession,
} from './runtime'
import { archiveErrorMessage } from './archive'
import { GROUP_WORD, W } from './words'
import {
  cancelChatMessage,
  chatRedactionCount,
  listenChatArchiveChanged,
  createChat,
  probeSendableProviders,
  createChatGroup,
  listChatGroups,
  removeChatGroup,
  renameChatGroup,
  reorderChatGroups,
  setChatGroup,
  type ChatGroup,
  archiveChat,
  listArchivedChats,
  listenChatListUpdated,
  setChatPinned,
  listenChatEvents,
  listChatsPage,
  readChat,
  readChatContext,
  renameChat,
  sendChatMessage,
  type ChatContext,
  type ChatDocument,
  type ChatEntry,
} from './chats'
import {
  PROVIDER_NAME,
  defaultModel,
  findModelName,
  resolveEffort,
  resolveFast,
  getPermissionMode,
  isMissingCommand,
  listAiModels,
  listPermissionModes,
  setPermissionMode,
  type AiProvider,
  type ChosenModel,
  type ProviderModels,
} from './ai-models'
import {
  loadChatModels,
  loadPreferences,
  saveChatModels,
  savePreferences,
} from './preferences'

const INITIAL_SESSION: RuntimeSession = {
  mode: 'starting',
  info: null,
  error: null,
}

type DirectoryPickerWindow = Window & {
  showDirectoryPicker?: (options?: { mode?: 'read' | 'readwrite' }) => Promise<unknown>
}

// 모델 목록을 읽지 못했을 때 고를 수 있는 곳: 설정의 AI 연결에 있는 두 곳.
const FALLBACK_PROVIDERS: AiProvider[] = ['codex', 'claude']

// 뒤로·앞으로가 오가는 곳. 'new' 는 새 대화, 'settings' 는 설정, 'chat:번호' 는 그 대화.
type Place = 'new' | 'settings' | 'documents' | 'media' | `chat:${string}`

const retryClock = new Intl.DateTimeFormat('ko-KR', { hour: 'numeric', minute: '2-digit' })
const retryDay = new Intl.DateTimeFormat('ko-KR', { month: 'long', day: 'numeric', hour: 'numeric', minute: '2-digit' })
/**
 * 회사가 준 "다시 쓸 수 있는 때"를 화면 글로. 숫자면 초(10^12 보다 크면 밀리초)로 읽고, 글이면 날짜로 읽는다.
 * 지금보다 앞서거나 40일보다 먼 값은 단위를 잘못 읽었을 수 있어 보이지 않는다(클로드는 단위가 확인되지 않음, G18).
 */
function retryTimeText(value: number | string | null) {
  if (value === null || value === undefined || value === '') return null
  const ms = typeof value === 'number' ? (value > 1e12 ? value : value * 1000) : Date.parse(value)
  if (!Number.isFinite(ms)) return null
  const now = Date.now()
  if (ms <= now || ms - now > 40 * 86_400_000) return null
  const date = new Date(ms)
  return new Date(now).toDateString() === date.toDateString() ? retryClock.format(date) : retryDay.format(date)
}

type ShortcutAction = 'back' | 'forward' | 'toggleList' | 'togglePanel' | 'search'

/**
 * 단축키 다섯(김성균 개발자가 정함, 2026-11-15). 다른 앱의 단축키는 받지 않는다.
 *   뒤로 Alt+← · 앞으로 Alt+→ · 왼쪽 목록 열고 닫기 Ctrl+[ · 오른쪽 문서 패널 열고 닫기 Ctrl+] · 검색 창 열고 닫기 Ctrl+K
 * 글 칸 안에서도 받는다(Windows 의 글 칸에서 이 키들은 글 편집에 쓰이지 않는다). 한글 조합 중에는 받지 않는다.
 * 자판이 달라도 같은 자리의 키로 알아보도록 event.code 를 본다.
 */
function shortcutFor(event: KeyboardEvent): ShortcutAction | null {
  const { ctrlKey: ctrl, altKey: alt, shiftKey: shift, metaKey: meta, code } = event
  if (shift || meta) return null
  if (alt && !ctrl) {
    if (code === 'ArrowLeft') return 'back'
    if (code === 'ArrowRight') return 'forward'
  }
  if (ctrl && !alt) {
    if (code === 'BracketLeft') return 'toggleList'
    if (code === 'BracketRight') return 'togglePanel'
    if (code === 'KeyK') return 'search'
  }
  return null
}

function App() {
  const desktopRuntime = isDesktopRuntime()
  // 첫 실행 화면을 보일지(설치판과 개발용 앱 모두). 'checking' 동안은 빈 창만 둔다.
  // 개발용 앱에서 작업 화면을 바로 볼 때는 VITE_MOMSO_OPEN_WORKBENCH=1 을 주고 개발 서버를 켠다.
  const skipFirstRun = !desktopRuntime || (import.meta.env.DEV && import.meta.env.VITE_MOMSO_OPEN_WORKBENCH === '1')
  const [firstRun, setFirstRun] = useState<FirstRunCheck | 'checking' | null>(skipFirstRun ? null : 'checking')
  useEffect(() => {
    if (skipFirstRun) return
    let alive = true
    void checkFirstRun().then((result) => { if (alive) setFirstRun(result.show ? result : null) })
    return () => { alive = false }
  }, [skipFirstRun])
  const [preferences, setPreferences] = useState(loadPreferences)
  const [session, setSession] = useState<RuntimeSession>(INITIAL_SESSION)
  // 지금 화면: 홈(채팅방), 문서(읽는 글 모아 보기), 미디어, 설정.
  const [screen, setScreen] = useState<AppScreen>('home')
  const settingsOpen = screen === 'settings'
  // 미디어를 주는 백엔드(media_list)가 있는지. 없으면 미디어 단추는 눌리지 않는다.
  const [mediaAvailable, setMediaAvailable] = useState(false)

  // 대화. selectedChatId 가 null 이면 새 대화를 쓰는 중이다(첫 말을 보낼 때 만들어진다).
  const [chats, setChats] = useState<ChatEntry[]>([])
  const [chatsError, setChatsError] = useState<string | null>(null)
  // 채팅방 목록을 한 번이라도 읽었는지(읽는 동안 왼쪽 목록에 "불러오는 중", 2026-11-17).
  const [chatsLoaded, setChatsLoaded] = useState(false)
  // 채팅방 묶음. null 이면 묶음 백엔드가 없다(묶음 만들기가 눌리지 않는다).
  const [groups, setGroups] = useState<ChatGroup[] | null>(null)
  const [selectedChatId, setSelectedChatId] = useState<string | null>(null)
  const [chat, setChat] = useState<ChatDocument | null>(null)
  // 보냈지만 아직 대화에 붙지 않은 말(화면에 먼저 보여 준다).
  const [pendingMessage, setPendingMessage] = useState<ChatViewMessage | null>(null)
  // 채팅방 고정·보관 명령이 있는지(앱을 열 때 알아본다).
  const [chatKeep, setChatKeep] = useState(false)
  // 보관 명령이 있는지 아직 모르면(읽는 중) 설정의 "보관된 채팅방" 칸을 숨기지 않는다(늦게 나타나지 않게, 2026-11-17).
  const [chatKeepKnown, setChatKeepKnown] = useState(false)
  const stopChatListUpdated = useRef<(() => void) | null>(null)
  useEffect(() => () => stopChatListUpdated.current?.(), [])
  const [generatingChatId, setGeneratingChatId] = useState<string | null>(null)
  const [prompt, setPrompt] = useState('')
  // 답이 오는 중이면 지금까지 온 글(chat-stream).
  const [streamText, setStreamText] = useState('')
  // 고속 모드로 쓴 답과 넘기지 못한 말의 수는 백엔드가 답에 저장한다(chat_read 의 fast·notHandedOver). 앱을 다시 켜도 남는다.
  // 고속 모드로 보내 봤는데 쓸 수 없다고 온 모델('회사:모델'). 그 뒤로 고속 모드 단추가 눌리지 않는다.
  const [fastRefused, setFastRefused] = useState<Record<string, true>>({})
  // 마지막으로 보낸 곳(chat-error 의 fast_unavailable 이 어느 모델의 것인지 알려고).
  const lastSendRef = useRef<{ chatId: string; provider: AiProvider; model: string | null } | null>(null)
  // 답이 오지 않은 채팅방마다 그 까닭(G18 chat-error 의 error)과 보냈던 글. 그 채팅방에서 다시 보내면 지운다.
  const [chatFailures, setChatFailures] = useState<Record<string, { kind: string; retryAt: number | string | null; actions: string[]; provider: AiProvider; text: string; userMessageId: string | null }>>({})
  // userMessageId: 보내기가 시작되면 chat_send 가 주는 내 말의 id(다시 보내기에 쓴다).
  const lastSentTextRef = useRef<{ chatId: string; text: string; userMessageId?: string | null } | null>(null)
  // 설정을 열 때 처음 보일 쪽(알림 줄의 "AI 연결 열기"면 'ai').
  const [settingsStart, setSettingsStart] = useState<SettingsSection>('profile')
  const [models, setModels] = useState<ProviderModels[] | null>(null)
  // 모델 목록: 읽는 중 / 읽음(명령이 없을 때도) / 읽지 못함. 입력창의 모델·추론 수준 자리에 한 줄로 알린다(2026-11-17).
  const [modelsState, setModelsState] = useState<'loading' | 'ok' | 'failed'>(desktopRuntime ? 'loading' : 'ok')
  const refreshModels = useCallback(() => {
    setModelsState('loading')
    return listAiModels()
      .then((list) => { setModels(list); setModelsState('ok') })
      .catch((error) => { setModels(null); setModelsState(isMissingCommand(error) ? 'ok' : 'failed') })
  }, [])
  // 모델 목록과 보낼 수 있는 곳을 읽었는지(앱을 연 직후 잠깐). 개발용 화면(데스크톱 밖)은 처음부터 됨.
  const [modelsReady, setModelsReady] = useState({ models: !desktopRuntime, sendable: !desktopRuntime })
  // 받은 채팅 사본을 되살리는 동안(chat-archive-restore-progress, available false)은 채팅방을 바꾸는 요청을 보내지 않는다(RESTORE-PROGRESS.md).
  const [chatRestore, setChatRestore] = useState<RestoreProgress | null>(null)
  const chatRestoreRef = useRef<RestoreProgress | null>(null)
  chatRestoreRef.current = chatRestore
  const sendReady = modelsReady.models && modelsReady.sendable && chatRestore === null
  // 대화마다 고른 모델과 추론 수준(새 대화는 마지막에 쓴 것으로 시작한다).
  const [chatModels, setChatModels] = useState(loadChatModels)
  const [permission, setPermission] = useState<PermissionState | null>(null)
  const [chatContext, setChatContext] = useState<ChatContext | null>(null)
  // 뒤로·앞으로. 지나온 곳과 지금 자리.
  const [history, setHistory] = useState<{ places: Place[]; index: number }>({ places: [], index: -1 })
  // 다음에 바뀌는 곳은 쌓지 않고 지금 자리를 바꾼다(앱을 열 때 최근 대화로 갈 때, 새 대화가 저장되어 번호가 생길 때).
  const replacePlace = useRef(false)
  // 대화마다 적다 만 말. 다른 대화로 갔다 와도 남아 있다.
  const drafts = useRef(new Map<string, string>())
  // 지금 보낼 수 있는 곳(chat_send 에 물어 안다). 알기 전에는 Codex·Claude.
  const [probedProviders, setSendableProviders] = useState<AiProvider[]>(FALLBACK_PROVIDERS)
  // AI 연결 상태(provider_auth_status). 저장만 됐거나 틀렸거나 확인하지 못한 API 키는 연결로 치지 않는다(2026-11-17 계약 "AI 연결 확인").
  // 입력창의 회사 고르기·기본 회사와 왼쪽 띠 사용량이 같은 조건(isConnected)을 쓴다. 상태를 읽지 못하면 chat_send 에 물은 것만 쓴다.
  const providerAuth = useProviderAuth()
  const sendableProviders = useMemo(() => {
    const list = providerAuth.list
    if (!Array.isArray(list)) return probedProviders
    return probedProviders.filter((provider) => {
      const item = list.find((entry) => entry.provider === provider)
      return !item || isConnected(item)
    })
  }, [probedProviders, providerAuth.list])
  const generatingRef = useRef<{ chatId: string } | null>(null)
  // 채팅방이 길어져 앞의 내용을 줄여 담은 때(chat-compacted). 그 자리에 한 줄로 남는다(앱을 닫으면 사라짐).
  const [compactions, setCompactions] = useState<Array<{ id: string; chatId: string; at: number }>>([])
  // 몸소 브라우저: 선생님이 직접 해야 할 일(browser-human-required, 채팅방마다 하나)과 브라우저가 열려 있는 채팅방.
  const [browserHuman, setBrowserHuman] = useState<Record<string, { reason: string; at: number }>>({})
  const [browserOpen, setBrowserOpen] = useState<Record<string, boolean>>({})
  // 채팅방 GitHub 사본에서 가린 수(채팅방마다). 0 보다 크면 그 채팅방 끝에 한 줄.
  const [redactions, setRedactions] = useState<Record<string, number>>({})
  // AI 가 허락을 구한 물음들. 대화마다 둔다(다른 대화로 갔다 와도 그대로 있다). 앱을 닫으면 사라진다.
  const [approvals, setApprovals] = useState<ApprovalItem[]>([])
  const [secretRequests, setSecretRequests] = useState<SecretItem[]>([])
  // 지금 보이는 대화의 말 수(물음이 온 자리를 적어 두려고).
  const shownCountRef = useRef(0)

  const [toast, setToast] = useState<string | null>(null)
  // 알림 줄의 단추 하나(원자료 삭제의 되돌리기처럼). 알림이 사라지면 함께 사라진다.
  const [toastAction, setToastAction] = useState<{ label: string; run: () => void } | null>(null)
  const [zetPanelOpen, setZetPanelOpen] = useState(false)
  // 스킬. null = 스킬 명령이 없음(입력 칸의 / 목록과 설정의 "스킬" 쪽을 그리지 않는다).
  const [skills, setSkills] = useState<SkillEntry[] | null>(null)
  // 스킬 명령은 있는데 목록을 읽지 못했다(명령이 없는 것과 다르다): 설정의 "스킬" 쪽에서 알린다.
  const [skillsFailed, setSkillsFailed] = useState(false)
  // 스킬 목록을 한 번이라도 읽었는지(읽는 동안 설정의 "스킬" 칸을 숨기지 않고 "불러오는 중").
  const [skillsLoaded, setSkillsLoaded] = useState(false)
  const [chosenSkill, setChosenSkill] = useState<SkillEntry | null>(null)
  const refreshSkills = useCallback(() => {
    if (!isDesktopRuntime()) return
    listSkills().then((list) => {
      setSkills(list)
      setSkillsFailed(false)
      setSkillsLoaded(true)
      // 입력 칸에 골라 둔 스킬이 설정에서 꺼졌거나 없어졌으면 그 조각도 뺀다(보내면 skill_unavailable 이 된다).
      setChosenSkill((current) => (current && list.some((item) => item.name === current.name && item.enabled) ? current : null))
    }).catch((error) => { setSkills(null); setSkillsFailed(!isMissingCommand(error)); setSkillsLoaded(true) })
  }, [])
  const zetPanelOpenRef = useRef(zetPanelOpen)
  // 패널 단추는 열면 패널 머리로, 닫으면 채팅방 머리로 옮겨 다시 그려진다. 키보드로 누른 뒤 초점이 화면 밖으로 빠지지 않게 그 단추로 옮긴다(2026-11-17).
  useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      const active = document.activeElement
      if (!active || active === document.body) document.querySelector<HTMLElement>('.zet-panel-toggle')?.focus()
    })
    return () => window.cancelAnimationFrame(frame)
  }, [zetPanelOpen])
  zetPanelOpenRef.current = zetPanelOpen
  // 지금 연 채팅방에서 다룬 문서와 미디어(오른쪽 패널). undefined = 명령이 없음(예전처럼 문서 전체), null = 읽는 중.
  const [chatItems, setChatItems] = useState<ChatItems | null | undefined>(undefined)
  // 이 채팅방에서 다룬 것을 읽지 못했다(명령은 있음). 앞 채팅방의 것이 남거나 전체 문서 목록으로 말없이 바뀌지 않게 패널에서 알린다(2026-11-17).
  const [chatItemsFailed, setChatItemsFailed] = useState(false)
  const chatItemsSupported = useRef<boolean | null>(null)
  // 지금 패널에 있는 "이 채팅방" 것이 어느 채팅방의 것인지. 채팅방이 바뀌면 읽는 동안 앞 채팅방 것이나 전체 문서 목록을 보이지 않는다(2026-11-17).
  const itemsChatRef = useRef<string | null>(null)
  // 패널을 닫아 둔 동안 채팅방에 새로 생긴 것이 있으면 패널 단추에 점(채팅방마다).
  const [freshInPanel, setFreshInPanel] = useState<Record<string, boolean>>({})
  // 검색 창(Ctrl+K).
  const [searchOpen, setSearchOpen] = useState(false)
  // 앱 버전과 업데이트. 다시 켜기 전에 물을 일이 있으면 묻는다.
  const update = useAppUpdate()
  const [askRestart, setAskRestart] = useState(false)
  // 왼쪽 목록은 사람이 접을 때만 접힌다(창 제목 표시줄의 단추). 오른쪽 패널을 연다고 접히지 않는다.
  const [listHidden, setListHidden] = useState(false)
  const [listMoving, setListMoving] = useState(false)
  // 오른쪽 패널은 홈에서만 띄운다(설정·미디어에서는 숨기고, 열려 있던 상태는 기억해 두었다가 홈으로 오면 돌아온다).
  // 문서 화면에서는 같은 패널을 화면 가득 넓혀 글 목록과 글을 보인다(같은 목록이 두 번 보이지 않게 하나만 둔다).
  const panelShown = (zetPanelOpen && screen === 'home') || screen === 'documents'
  const zetPanelFrame = useZetPanelFrame(panelShown, listHidden)
  const toastTimer = useRef<number | null>(null)
  const selectedChatRef = useRef<string | null>(null)

  // 그 채팅방에서 다룬 것을 다시 읽는다. 답이 끝났거나 백엔드가 바뀌었다고 알리면(fromEvent) 패널이 닫혀 있을 때 새로 생긴 것을 표시한다.
  const refreshChatItems = useCallback((chatId: string | null, fromEvent = false) => {
    if (!desktopRuntime || chatItemsSupported.current === false) return
    if (!chatId) { itemsChatRef.current = null; if (chatItemsSupported.current) setChatItems(EMPTY_CHAT_ITEMS); return }
    if (!fromEvent && itemsChatRef.current !== chatId) {
      // 읽는 동안: 패널은 "불러오는 중"(null). 명령이 없다고 알게 되면 아래에서 undefined(예전 문서 목록)로 돌린다.
      setChatItems(null)
      setChatItemsFailed(false)
    }
    getChatItems(chatId).then((next) => {
      chatItemsSupported.current = true
      if (chatId !== selectedChatRef.current) return
      itemsChatRef.current = chatId
      setChatItemsFailed(false)
      setChatItems((previous) => {
        if (fromEvent && !zetPanelOpenRef.current && previous && chatItemCount(next) > chatItemCount(previous)) {
          setFreshInPanel((current) => ({ ...current, [chatId]: true }))
        }
        return next
      })
    }).catch((error) => {
      if (isMissingCommand(error)) { chatItemsSupported.current = false; setChatItems(undefined); return }
      if (chatId !== selectedChatRef.current) return
      chatItemsSupported.current = true
      itemsChatRef.current = chatId
      setChatItems(null)
      setChatItemsFailed(true)
    })
  }, [desktopRuntime])
  selectedChatRef.current = selectedChatId

  function showToast(message: string, action?: { label: string; run: () => void }) {
    setToast(message)
    setToastAction(action ?? null)
    if (toastTimer.current) window.clearTimeout(toastTimer.current)
    // 단추가 있으면 누를 틈을 더 둔다.
    toastTimer.current = window.setTimeout(() => { setToast(null); setToastAction(null) }, action ? 8000 : 2600)
  }

  // 채팅방 목록은 쪽으로 받는다(chat_list_page, 80개씩). 다시 읽을 때는 이미 받아 둔 만큼(적어도 한 쪽)을 다시 받는다.
  const chatsNext = useRef<number | null>(null)
  const chatsCount = useRef(0)
  const chatsLoadingMore = useRef(false)
  const refreshChats = useCallback(async () => {
    // 설치된 앱이 아니면(브라우저로 연 개발 화면) 대화 목록을 부르지 않는다.
    if (!isDesktopRuntime()) return null
    try {
      const want = Math.max(1, chatsCount.current)
      let page = await listChatsPage(0)
      const next = [...page.items]
      while (page.hasMore && page.nextOffset !== null && next.length < want) {
        page = await listChatsPage(page.nextOffset)
        const seen = new Set(next.map((chat) => chat.id))
        next.push(...page.items.filter((chat) => !seen.has(chat.id)))
      }
      chatsNext.current = page.hasMore ? page.nextOffset : null
      chatsCount.current = next.length
      setChats(next)
      setChatsError(null)
      setChatsLoaded(true)
      return next
    } catch (error) {
      setChatsError(archiveErrorMessage(error, W.app.text1))
      setChatsLoaded(true)
      return null
    }
  }, [])

  // 왼쪽 목록을 끝까지 내리면 다음 쪽을 받아 붙인다(찾기 창과 같은 방식, 새 단추 없음).
  const loadMoreChats = useCallback(async () => {
    if (chatsNext.current === null || chatsLoadingMore.current || !isDesktopRuntime()) return
    chatsLoadingMore.current = true
    try {
      const page = await listChatsPage(chatsNext.current)
      chatsNext.current = page.hasMore ? page.nextOffset : null
      setChats((current) => {
        const seen = new Set(current.map((chat) => chat.id))
        const merged = [...current, ...page.items.filter((chat) => !seen.has(chat.id))]
        chatsCount.current = merged.length
        return merged
      })
    } catch {
      chatsNext.current = null
    } finally {
      chatsLoadingMore.current = false
    }
  }, [])

  const refreshGroups = useCallback(async () => {
    try {
      setGroups(await listChatGroups())
    } catch {
      setGroups(null)
    }
  }, [])

  // 되살리는 중이면 채팅방을 바꾸지 않고 그 까닭 한 줄만 알린다.
  function restoreBlocks() {
    if (!chatRestoreRef.current) return false
    showToast(W.restoreProgress.chatBusy)
    return true
  }

  // 채팅방 고정·풀기. 실패하면 알리고, 어쨌든 목록을 다시 읽는다.
  async function pinChat(chatId: string, pinned: boolean) {
    if (restoreBlocks()) return
    try {
      await setChatPinned(chatId, pinned)
    } catch (error) {
      showToast(archiveErrorMessage(error, pinned ? W.chatKeep.pinFailed : W.chatKeep.unpinFailed))
    }
    await refreshChats()
  }

  // 채팅방 보관: 왼쪽 목록에서 빠진다(설정의 보관된 채팅방에서 되돌리거나 영구 삭제). 보고 있던 채팅방이면 새 채팅방으로.
  async function keepChatAway(chatId: string) {
    if (restoreBlocks()) return
    try {
      await archiveChat(chatId)
      if (selectedChatRef.current === chatId) startNewChat()
    } catch (error) {
      showToast(archiveErrorMessage(error, W.chatKeep.archiveFailed))
    }
    await refreshChats()
  }

  // 묶음을 바꾼 뒤: 묶음과 채팅방 목록(groupId)을 다시 읽는다. 실패하면 알린다.
  const changeGroups = useCallback(async (work: () => Promise<unknown>, fallback: string) => {
    if (chatRestoreRef.current) { showToast(W.restoreProgress.chatBusy); return }
    try {
      await work()
    } catch (error) {
      showToast(archiveErrorMessage(error, fallback))
    }
    await Promise.all([refreshGroups(), refreshChats()])
  }, [refreshGroups, refreshChats])

  const openChat = useCallback(async (id: string) => {
    selectedChatRef.current = id
    setSelectedChatId(id)
    setPendingMessage(null)
    try {
      const document = await readChat(id)
      if (selectedChatRef.current === id) setChat(document)
    } catch (error) {
      if (selectedChatRef.current === id) setChat(null)
      showToast(archiveErrorMessage(error, W.app.text2))
    }
  }, [])

  const refreshRuntime = useCallback(async () => {
    setSession((current) => ({ ...current, mode: 'starting', error: null }))
    const nextSession = await initializeRuntime()
    setSession(nextSession)
    if (nextSession.mode !== 'ready') return

    // 앱을 열면 가장 최근 대화를 연다. 대화가 없으면 새 대화(빈 칸)로 시작한다.
    const listed = await refreshChats()
    // 이미 그 대화가 열려 있으면(개발용 실행은 이 함수를 두 번 부른다) 자리 바꾸기 표시를 남기지 않는다.
    if (listed && listed.length > 0 && selectedChatRef.current !== listed[0].id) {
      replacePlace.current = true
      void openChat(listed[0].id)
    }
    // 모델 목록과 보낼 수 있는 곳을 다 읽기 전에는 보내기가 눌리지 않는다(고른 것과 다른 모델이 답하지 않게).
    void refreshModels().finally(() => setModelsReady((current) => ({ ...current, models: true })))
    void refreshGroups()
    // 채팅방 고정·보관 명령이 있는지(보관된 목록을 읽어 본다. 아무것도 바뀌지 않는다). 없으면 고정·보관 단추와 설정 쪽을 그리지 않는다.
    void listArchivedChats().then(() => setChatKeep(true)).catch((error) => setChatKeep(!isMissingCommand(error))).finally(() => setChatKeepKnown(true))
    // 고정·보관·되돌리기·삭제가 되면(다른 곳에서 바뀐 것도) 목록을 다시 읽는다.
    if (!stopChatListUpdated.current) {
      stopChatListUpdated.current = () => {}
      void listenChatListUpdated(() => { void refreshChats() }).then((stop) => { stopChatListUpdated.current = stop })
    }
    refreshSkills()
    void listMedia().then(() => setMediaAvailable(true)).catch((error) => setMediaAvailable(!isMissingMediaCommand(error)))
    // 어느 AI 로 보낼 수 있는지 미리 안다(아무것도 저장되지 않는 물음).
    void probeSendableProviders(['codex', 'claude', 'gemini']).then((sendable) => {
      setSendableProviders(sendable ?? FALLBACK_PROVIDERS)
    }).finally(() => setModelsReady((current) => ({ ...current, sendable: true })))
  }, [openChat, refreshChats])

  useEffect(() => {
    void refreshRuntime()
  }, [refreshRuntime])

  useEffect(() => {
    savePreferences(preferences)
  }, [preferences])

  useEffect(() => () => {
    if (toastTimer.current) window.clearTimeout(toastTimer.current)
  }, [])

  const chatPlace: Place = selectedChatId ? `chat:${selectedChatId}` : 'new'
  const place: Place = screen === 'home' ? chatPlace : screen

  // 곳이 바뀔 때마다 지나온 곳에 쌓는다. 뒤로·앞으로로 옮길 때는 자리를 먼저 바꾸므로 쌓이지 않는다.
  useEffect(() => {
    // 표시는 바꾸는 함수 밖에서 읽는다(개발용 실행의 StrictMode 는 바꾸는 함수를 두 번 부른다).
    const replace = replacePlace.current
    replacePlace.current = false
    setHistory((current) => {
      if (current.places[current.index] === place) return current
      if (replace && current.index >= 0) {
        const places = current.places.slice()
        places[current.index] = place
        return { places, index: current.index }
      }
      const places = [...current.places.slice(0, current.index + 1), place]
      return { places, index: places.length - 1 }
    })
  }, [place])

  // 그 곳을 보인다. 대화를 옮길 때 적다 만 말은 대화마다 남겨 둔다.
  function showPlace(next: Place) {
    if (next === 'settings' || next === 'documents' || next === 'media') {
      setScreen(next)
      return
    }
    setScreen('home')
    const nextId = next === 'new' ? null : next.slice(5)
    if (nextId === selectedChatId) return
    drafts.current.set(selectedChatId ?? 'new', prompt)
    setPrompt(drafts.current.get(nextId ?? 'new') ?? '')
    if (nextId) {
      void openChat(nextId)
    } else {
      setSelectedChatId(null)
      setChat(null)
      setPendingMessage(null)
    }
  }

  function startNewChat() {
    if (restoreBlocks()) return
    showPlace('new')
  }

  function selectChat(id: string) {
    showPlace(`chat:${id}`)
  }

  const canGoBack = history.index > 0
  const canGoForward = history.index < history.places.length - 1

  // 설정 닫기: 설정에 오기 전 곳으로 돌아간다(뒤로와 같다).
  // 설정을 떠나면 다음에 열 때는 처음 쪽(나에 대해)부터.
  useEffect(() => { if (screen !== 'settings') setSettingsStart('profile') }, [screen])

  /** 설정을 그 쪽으로 연다(채팅의 알림 줄에서). */
  function openSettingsAt(section: SettingsSection) {
    setSettingsStart(section)
    showPlace('settings')
  }

  function closeSettings() {
    if (canGoBack) moveInHistory(-1)
    else setScreen('home')
  }

  function moveInHistory(step: -1 | 1) {
    const index = history.index + step
    const next = history.places[index]
    if (!next) return
    setHistory((current) => ({ ...current, index }))
    showPlace(next)
  }

  // 클립(또는 끌어 놓기)으로 고른 파일을 지금 채팅방에 첨부한다. 채팅방이 아직 없으면(새 채팅방) 보낼 때처럼 먼저 만든다
  // (뒤쪽 chat_attach_asset 은 있는 채팅방에만 첨부된다). 첨부한 것은 오른쪽 패널의 "이 채팅방에서 다룬 것"에 보인다.
  const attachingRef = useRef(false)
  async function attachFiles(paths: string[]) {
    if (paths.length === 0 || attachingRef.current) return
    attachingRef.current = true
    try {
      let chatId = selectedChatId
      if (!chatId) {
        const created = await createChat()
        chatId = created.id
        selectedChatRef.current = chatId
        replacePlace.current = true
        setSelectedChatId(chatId)
        setChat({ id: created.id, title: created.title, createdAt: created.createdAt, updatedAt: created.updatedAt, messages: [] })
        rememberChatModel(created.id, chosen)
        void refreshChats()
      }
      // 폴더를 놓으면 뒤쪽이 그 경로만 source_folder_not_allowed 로 거절한다(2026-11-17). 폴더는 건너뛰고 파일은 계속 붙인다.
      const names: string[] = []
      let folders = 0
      for (const path of paths) {
        try {
          names.push(await attachFileToChat(chatId, path))
        } catch (error) {
          if (!String(error instanceof Error ? error.message : error).includes('source_folder_not_allowed')) throw error
          folders += 1
        }
      }
      if (names.length > 0) refreshChatItems(chatId, true)
      // 붙인 것과 폴더를 한 줄에 함께(둘 다 알 수 있게).
      const done = names.length === 0 ? null : names.length === 1 ? W.attach.done(names[0]) : W.attach.doneMany(names.length)
      showToast([done, folders > 0 ? W.attach.folderNotAllowed : null].filter(Boolean).join('. '))
    } catch (error) {
      // 고른 지 5분이 지나 뒤쪽이 거절하면(selection_required) 다시 고르라는 한 줄.
      showToast(isSelectionExpired(error) ? W.selectAgain : archiveErrorMessage(error, W.attach.failed))
    } finally {
      attachingRef.current = false
    }
  }

  // 클립: 고르는 창은 뒤쪽이 연다(select_local_paths source, 여럿). 취소하면 빈 배열이라 아무 일도 없다.
  async function pickAndAttach() {
    const paths = await selectLocalPaths('source').catch(() => [] as string[])
    await attachFiles(paths)
  }

  // 입력 칸 아래에서 고른 모델. 고른 적이 없으면 설정의 AI 곳의 기본 모델.
  // 고른 모델이 목록에서 사라졌으면 그 곳의 기본 모델로, 목록을 읽지 못했으면 설정의 두 곳 안에서 고른다.
  const chosen: ChosenModel = (() => {
    const saved = chatModels[selectedChatId ?? 'new'] ?? preferences.chatModel ?? { provider: preferences.assistantProvider, model: null }
    // 보낼 수 없는 곳이 남아 있으면 설정의 AI 곳으로.
    const fallbackProvider = sendableProviders.includes(preferences.assistantProvider) ? preferences.assistantProvider : sendableProviders[0] ?? preferences.assistantProvider
    const wanted = sendableProviders.includes(saved.provider) ? saved : { provider: fallbackProvider, model: null }
    if (!models) {
      return FALLBACK_PROVIDERS.includes(wanted.provider)
        ? { provider: wanted.provider, model: null }
        : { provider: preferences.assistantProvider, model: null }
    }
    const list = models.find((item) => item.provider === wanted.provider)
    if (wanted.model && list?.state === 'ok' && list.models.some((model) => model.id === wanted.model)) return wanted
    if (list?.state === 'ok' && list.models.length > 0) return { provider: wanted.provider, model: defaultModel(list)?.id ?? null, effort: wanted.effort ?? null }
    // 그 곳의 모델 목록을 읽지 못했으면, 보낼 수 있고 목록이 있는 곳으로(설정의 AI 곳을 먼저).
    const usable = [fallbackProvider, ...sendableProviders]
      .map((provider) => models.find((item) => item.provider === provider && item.state === 'ok' && item.models.length > 0))
      .find(Boolean)
    return usable
      ? { provider: usable.provider, model: defaultModel(usable)?.id ?? null }
      : { provider: wanted.provider, model: null }
  })()

  // 모델마다 마지막에 고른 추론 수준(그 모델로 돌아오면 그 값).
  const modelEfforts = useRef<Record<string, string>>((() => {
    try { return JSON.parse(window.localStorage.getItem('momso.desktop.model-efforts.v1') ?? '{}') as Record<string, string> } catch { return {} }
  })())

  function chooseProvider(provider: AiProvider) {
    // 회사를 바꾸면 그 회사의 기본 모델, 그 모델의 기본 추론 수준(전에 고른 값이 있으면 그것). 권한은 회사에서 기억해 둔 단계(백엔드).
    const list = models?.find((item) => item.provider === provider && item.state === 'ok')
    const model = defaultModel(list)?.id ?? null
    chooseModel({ provider, model, effort: model ? modelEfforts.current[model] ?? null : null })
  }

  function chooseFamily(modelId: string) {
    chooseModel({ provider: chosen.provider, model: modelId, effort: modelEfforts.current[modelId] ?? null })
  }

  function chooseEffort(effort: string) {
    if (chosen.model) {
      modelEfforts.current = { ...modelEfforts.current, [chosen.model]: effort }
      try { window.localStorage.setItem('momso.desktop.model-efforts.v1', JSON.stringify(modelEfforts.current)) } catch { /* 저장하지 못해도 지금 값으로 보낸다 */ }
    }
    chooseModel({ ...chosen, effort })
  }

  function toggleFast(on: boolean) {
    chooseModel({ ...chosen, fast: on })
  }

  function rememberChatModel(key: string, next: ChosenModel) {
    setChatModels((current) => {
      // 새 대화는 마지막에 쓴 것으로 시작하므로 'new' 도 같이 바꾼다.
      const updated = { ...current, [key]: next, new: next }
      saveChatModels(updated)
      return updated
    })
  }

  function chooseModel(next: ChosenModel) {
    // 이 대화에서 쓸 것으로 기억하고, 새 대화가 시작할 값(마지막에 쓴 것)으로도 둔다.
    rememberChatModel(selectedChatId ?? 'new', next)
    setPreferences((current) => ({
      ...current,
      chatModel: next,
      // 설정의 AI 연결과 같은 곳을 가리키게 둔다(설정에는 Codex, Claude 두 곳만 있다).
      ...(next.provider === 'codex' || next.provider === 'claude' ? { assistantProvider: next.provider } : {}),
    }))
  }

  // 권한 단계: 고른 곳에서 어느 단계를 쓸 수 있는지 함께 읽는다. 읽지 못하면 입력 칸 아래에 보이지 않는다.
  useEffect(() => {
    if (session.mode !== 'ready') return
    let current = true
    const provider = chosen.provider
    Promise.all([getPermissionMode(provider), listPermissionModes(provider)])
      .then(([mode, modes]) => {
        if (!current) return
        // 저장된 값이 이 회사 목록에 없으면 그 회사의 기본 단계로 보인다.
        const known = modes.some((item) => item.id === mode)
        const fallback = modes.find((item) => item.isDefault)?.id ?? modes[0].id
        setPermission({ provider, mode: known ? mode : fallback, modes })
      })
      .catch(() => { if (current) setPermission(null) })
    return () => { current = false }
  }, [session.mode, chosen.provider])

  async function changePermission(mode: string) {
    const provider = permission?.provider
    if (!provider) return
    try {
      await setPermissionMode(provider, mode)
      setPermission((current) => (current && current.provider === provider ? { ...current, mode } : current))
    } catch (error) {
      showToast(archiveErrorMessage(error, W.app.text6))
    }
  }

  // 대화가 찬 정도. 숫자가 올 때만 입력 칸 아래에 보인다.
  const refreshContext = useCallback((id: string | null) => {
    if (!id) {
      setChatContext(null)
      return
    }
    readChatContext(id)
      .then((next) => { if (selectedChatRef.current === id) setChatContext(next) })
      .catch(() => { if (selectedChatRef.current === id) setChatContext(null) })
  }, [])

  useEffect(() => {
    if (session.mode === 'ready') refreshContext(selectedChatId)
  }, [session.mode, selectedChatId, refreshContext])

  const finishGenerating = useCallback((chatId: string) => {
    if (generatingRef.current?.chatId !== chatId) return
    generatingRef.current = null
    setGeneratingChatId(null)
    setStreamText('')
    void refreshChats()
    void readChat(chatId).then((document) => {
      if (selectedChatRef.current !== chatId) return
      setChat(document)
      setPendingMessage(null)
    }).catch(() => {})
    refreshContext(chatId)
  }, [refreshChats, refreshContext])

  // 백엔드가 보내는 답(chat-stream, chat-done, chat-error).
  useEffect(() => {
    if (!desktopRuntime) return
    let stop: (() => void) | undefined
    let disposed = false
    void listenChatEvents({
      onStream: ({ chatId, delta }) => {
        if (generatingRef.current?.chatId === chatId) setStreamText((current) => current + delta)
      },
      // 고속 모드 여부와 넘기지 못한 말의 수는 이 이벤트 전에 답에 저장된다. 다시 읽은 말(chat_read)로 보인다.
      onDone: ({ chatId }) => {
        finishGenerating(chatId)
        refreshChatItems(chatId, true)
      },
      // 찬 정도는 이벤트에 값이 함께 온다(회사를 바꾸면 unavailable). 값이 없으면 다시 읽는다.
      onContext: ({ chatId, context }) => {
        if (!context) { refreshContext(chatId); return }
        if (selectedChatRef.current === chatId) setChatContext(context)
      },
      // 백엔드가 지은 짧은 이름: 목록과 머리의 이름만 바꾼다(머리 이름은 부드럽게 바뀐다).
      onRenamed: ({ chatId, title }) => {
        setChats((current) => current.map((item) => (item.id === chatId ? { ...item, title } : item)))
        setChat((current) => (current && current.id === chatId ? { ...current, title } : current))
      },
      onCompacted: ({ chatId }) => {
        // 앞의 내용을 줄여 담았다: 그 자리에 한 줄로 알린다(두 앱처럼).
        const at = chatId === selectedChatRef.current ? shownCountRef.current : Number.MAX_SAFE_INTEGER
        setCompactions((current) => [...current, { id: `${chatId}-${Date.now()}`, chatId, at }])
        refreshContext(chatId)
      },
      onError: ({ chatId, reason, message, status, error, userMessageId }) => {
        if (generatingRef.current?.chatId !== chatId) return
        if (status === 'fast_unavailable') {
          // 보낸 뒤 고속 모드를 쓸 수 없다고 왔다(보낸 말은 채팅방에 남고, 같은 글을 다시 보내면 백엔드가 그 말을 다시 쓴다).
          const sent = lastSendRef.current
          refuseFast(sent?.chatId === chatId ? sent : null)
          finishGenerating(chatId)
          return
        }
        const why = reason ?? message ?? ''
        if (String(why).includes('중단') && !error) { finishGenerating(chatId); return }
        // 까닭을 가른 값(G18)으로 채팅방 끝에 한 줄을 남긴다. 회사가 준 원문은 화면에 두지 않는다.
        // 로그인이 풀린 것(status auth_failed)은 error 가 없던 예전 계약에서도 authentication 으로 본다. 그 밖에 값이 없으면 unknown.
        const sent = lastSendRef.current
        const sentText = lastSentTextRef.current?.chatId === chatId ? lastSentTextRef.current.text : ''
        const kind = error?.kind ?? (status === 'auth_failed' ? 'authentication' : 'unknown')
        setChatFailures((current) => ({
          ...current,
          [chatId]: {
            kind,
            retryAt: error?.retryAt ?? null,
            actions: Array.isArray(error?.actions) ? error.actions : status === 'auth_failed' ? ['signIn', 'switchProvider'] : [],
            provider: (sent?.chatId === chatId ? sent.provider : null) ?? 'codex',
            text: sentText,
            userMessageId: userMessageId ?? (lastSentTextRef.current?.chatId === chatId ? lastSentTextRef.current.userMessageId ?? null : null),
          },
        }))
        // 오다가 끊긴 답은 뒤쪽이 저장해 두므로(messageId) finishGenerating 이 채팅방을 다시 읽어 그때까지 온 글을 보인다.
        finishGenerating(chatId)
      },
    }).then((unlisten) => {
      if (disposed) unlisten()
      else stop = unlisten
    }).catch(() => {})
    return () => {
      disposed = true
      stop?.()
    }
  }, [desktopRuntime, finishGenerating])

  // 허락을 구하는 물음(agent-approval-request)과 거두기(agent-approval-closed).
  // 물음에는 대화 번호가 없어서(계약), 답을 쓰는 중인 대화에 둔다. 없으면 보고 있는 대화.
  useEffect(() => {
    if (!desktopRuntime) return
    let stop: (() => void) | undefined
    let disposed = false
    void listenApprovals({
      onRequest: (request) => {
        const chatId = request.chatId ?? generatingRef.current?.chatId ?? selectedChatRef.current
        const at = chatId && chatId === selectedChatRef.current ? shownCountRef.current : Number.MAX_SAFE_INTEGER
        setApprovals((current) => (
          current.some((item) => item.request.requestId === request.requestId)
            ? current
            : [...current, { request, chatId, at, status: 'pending', error: null }]
        ))
      },
      onClosed: (requestId) => {
        // 답하지 않은 물음만 거둔다. 답한 것은 한 줄로 남는다.
        setApprovals((current) => current.filter((item) => item.request.requestId !== requestId || item.status === 'answered'))
      },
    }).then((unlisten) => {
      if (disposed) unlisten()
      else stop = unlisten
    }).catch(() => {})
    return () => {
      disposed = true
      stop?.()
    }
  }, [desktopRuntime])

  // 몸소 브라우저 알림(browser.ts). 사람이 해야 할 일은 그 채팅방 끝에 한 줄로 두고, 그 채팅방에서 말을 보내거나 브라우저가 닫히면 거둔다.
  useEffect(() => {
    if (!desktopRuntime) return
    let stop: (() => void) | undefined
    let disposed = false
    void listenBrowser({
      onHumanRequired: (chatId, reason) => {
        const at = chatId === selectedChatRef.current ? shownCountRef.current : Number.MAX_SAFE_INTEGER
        setBrowserHuman((current) => ({ ...current, [chatId]: { reason, at } }))
      },
      onSession: (chatId, open) => {
        setBrowserOpen((current) => ({ ...current, [chatId]: open }))
        if (!open) setBrowserHuman((current) => { if (!current[chatId]) return current; const next = { ...current }; delete next[chatId]; return next })
      },
    }).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [desktopRuntime])

  // 채팅방 사본에서 가린 수: 채팅방을 고를 때와 사본을 새로 쓸 때(chat-archive-changed saved) 읽는다.
  useEffect(() => {
    if (!desktopRuntime) return
    const read = (chatId: string) => {
      chatRedactionCount(chatId)
        .then((count) => setRedactions((current) => (current[chatId] === (count ?? 0) ? current : { ...current, [chatId]: count ?? 0 })))
        .catch(() => {})
    }
    if (selectedChatId) read(selectedChatId)
    let stop: (() => void) | undefined
    let disposed = false
    void listenChatArchiveChanged((chatId, state) => { if (chatId && state === 'saved' && chatId === selectedChatRef.current) read(chatId) })
      .then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [desktopRuntime, selectedChatId])

  // 앱 시작 진행(runtime_startup_status / runtime-startup-progress). 끝나기 전에는 왼쪽 목록의 "불러오는 중" 줄에 단계를 얹는다.
  const [startup, setStartup] = useState<StartupProgress | null>(null)
  useEffect(() => {
    if (!desktopRuntime) return
    return watchStartup(setStartup)
  }, [desktopRuntime])
  const startupDetail = startup && !startup.available && W.app.startup[startup.state]
    ? (startup.total > 0 ? `${W.app.startup[startup.state]} ${startup.completed}/${startup.total}` : W.app.startup[startup.state])
    : null

  // 사본 쓰기 실패와 처음 받아 오기를 앱을 켤 때부터 듣는다(설정 화면을 열기 전에 온 것도 알리고, 받는 동안 문서 고치기를 막는다).
  useEffect(() => {
    if (!desktopRuntime) return
    watchChatArchive()
    watchFirstReceive()
  }, [desktopRuntime])

  // 받은 채팅 사본을 되살리는 진행. 끝나면 목록을 다시 읽고(새로 받은 채팅방이 생김), 실패하면 이 PC 에 있던 채팅방은 그대로라고 알린다.
  useEffect(() => {
    if (!desktopRuntime) return
    let stop: (() => void) | undefined
    let disposed = false
    void listenChatRestoreProgress((value) => {
      if (value.state === 'restoring' && !value.available) { setChatRestore(value); return }
      setChatRestore(null)
      if (value.state === 'complete') void refreshChats()
      else if (value.state === 'failed') showToast(W.restoreProgress.chatFailed)
    }).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [desktopRuntime, refreshChats])

  // 채팅방을 고를 때 브라우저가 열려 있는지 다시 확인한다(사람이 창을 직접 닫았을 수 있다).
  useEffect(() => {
    if (!desktopRuntime || !selectedChatId) return
    let current = true
    const chatId = selectedChatId
    browserSessionStatus(chatId).then((open) => { if (current) setBrowserOpen((value) => (value[chatId] === open ? value : { ...value, [chatId]: open })) }).catch(() => {})
    return () => { current = false }
  }, [desktopRuntime, selectedChatId])

  // 스킬로 하는 연결에서 키를 넣어 달라는 물음(agent-setup-secret-request)과 거두기. 채팅방 번호가 없으면 답을 쓰는 중인 대화에 둔다.
  useEffect(() => {
    if (!desktopRuntime) return
    let stop: (() => void) | undefined
    let disposed = false
    void listenSecretRequests({
      onRequest: (request) => {
        const chatId = request.chatId ?? generatingRef.current?.chatId ?? selectedChatRef.current
        const at = chatId && chatId === selectedChatRef.current ? shownCountRef.current : Number.MAX_SAFE_INTEGER
        setSecretRequests((current) => (
          current.some((item) => item.request.requestId === request.requestId)
            ? current
            : [...current, { request: { ...request, chatId }, at, status: 'pending', error: null }]
        ))
      },
      onClosed: (requestId) => {
        setSecretRequests((current) => current.filter((item) => item.request.requestId !== requestId || item.status !== 'pending'))
      },
    }).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [desktopRuntime])

  /** values 가 null 이면 "그만두기"(agent_setup_secret_cancel). */
  async function answerSecret(requestId: string, values: Record<string, string> | null) {
    const update = (patch: Partial<SecretItem>) => setSecretRequests((current) => current.map((item) => (
      item.request.requestId === requestId ? { ...item, ...patch } : item
    )))
    const item = secretRequests.find((entry) => entry.request.requestId === requestId)
    if (!item) return
    update({ status: 'sending', error: null })
    try {
      // 값은 여기서 바로 넘기고 어디에도 담지 않는다. 계약의 chatId 는 이벤트가 준 그대로(없으면 null).
      if (values) await submitSecret({ requestId, chatId: item.request.chatId }, values)
      else await cancelSecret({ requestId, chatId: item.request.chatId })
      update({ status: values ? 'saved' : 'cancelled' })
    } catch {
      update({ status: 'pending', error: values ? W.secretRequest.failed : W.secretRequest.cancelFailed })
    }
  }

  async function answerApproval(requestId: string, choice: ApprovalChoice) {
    const update = (patch: Partial<ApprovalItem>) => setApprovals((current) => current.map((item) => (
      item.request.requestId === requestId ? { ...item, ...patch } : item
    )))
    update({ status: 'sending', error: null })
    try {
      await respondApproval(requestId, choice)
      update({ status: 'answered', choice })
    } catch (error) {
      update({ status: 'pending', error: archiveErrorMessage(error, W.app.text8) })
    }
  }

  // 고속 모드를 쓸 수 없다고 왔을 때(chat_send 의 즉시 반환과 chat-error 가 같은 이름 fast_unavailable): 그 모델의 고속 모드 단추를 끄고 한 줄로 알린다.
  function refuseFast(sent: { provider: AiProvider; model: string | null } | null) {
    if (sent) setFastRefused((current) => ({ ...current, [`${sent.provider}:${sent.model ?? ''}`]: true }))
    const provider = sent?.provider ?? 'claude'
    const name = W.fastName[provider] ?? models?.find((list) => list.provider === provider)?.models.find((item) => item.id === sent?.model)?.fast?.label?.trim() ?? W.fastName.claude ?? ''
    showToast(W.fastUnavailable(name))
  }

  // 말 보내기: 대화가 없으면 만들고 chat_send 로 보낸다(답은 이벤트로 온다).
  async function sendPrompt(again?: string, retryMessageId?: string) {
    // again: 알림 줄의 "다시 보내기"로 보낸 글(입력창에 적던 글은 건드리지 않는다).
    // retryMessageId: 답을 받지 못한 그 말의 id. 뒤쪽이 저장된 그 말로 다시 보내고 같은 말을 새로 남기지 않는다.
    const text = again ?? prompt.trim()
    const skill = again ? null : chosenSkill
    if ((!text && !skill) || generatingChatId || !sendReady) return
    const target = chosen

    if (!again) {
      setPrompt('')
      setChosenSkill(null)
    }
    // 답을 받지 못한 마지막 말과 같은 글을 다시 보내면 백엔드가 그 말을 다시 쓸 수 있다(고속 모드 실패 뒤). 그때는 같은 말을 한 번 더 그리지 않는다.
    const last = chat && chat.id === selectedChatId ? chat.messages[chat.messages.length - 1] : undefined
    const resending = Boolean(retryMessageId) || (last?.role === 'user' && last.text === text)
    setPendingMessage(resending ? null : { id: `pending-${Date.now()}`, role: 'user', text, skill: skill?.title ?? null, createdAt: new Date().toISOString() })
    const giveBack = (error: unknown, fallback: string) => {
      setPendingMessage(null)
      setPrompt(text)
      setChosenSkill(skill)
      showToast(error ? archiveErrorMessage(error, fallback) : fallback)
    }

    let chatId = selectedChatId
    try {
      if (!chatId) {
        const created = await createChat()
        chatId = created.id
        selectedChatRef.current = chatId
        replacePlace.current = true
        setSelectedChatId(chatId)
        setChat({ id: created.id, title: created.title, createdAt: created.createdAt, updatedAt: created.updatedAt, messages: [] })
        // 새 대화에서 고른 모델은 이제 이 대화의 것이 된다.
        rememberChatModel(created.id, target)
      }
    } catch (error) {
      giveBack(error, W.app.text9)
      return
    }
    const targetId = chatId
    lastSentTextRef.current = { chatId: targetId, text }
    // 브라우저에서 직접 한 뒤 채팅으로 알린 것이므로 "직접 해 주세요" 줄을 거둔다.
    setBrowserHuman((current) => {
      if (!current[targetId]) return current
      const next = { ...current }
      delete next[targetId]
      return next
    })
    setChatFailures((current) => {
      if (!current[targetId]) return current
      const next = { ...current }
      delete next[targetId]
      return next
    })

    // 보내기는 chat_send 하나로 한다(예전 길: chat_append + run_ai_assistant 는 2026-11-16 없앰). 답은 이벤트로 온다.
    generatingRef.current = { chatId: targetId }
    setGeneratingChatId(targetId)
    setStreamText('')
    try {
      const targetModel = models?.find((list) => list.provider === target.provider)?.models.find((model) => model.id === target.model)
      const refused = Boolean(fastRefused[`${target.provider}:${target.model ?? ''}`])
      lastSendRef.current = { chatId: targetId, provider: target.provider, model: target.model }
      const result = await sendChatMessage(targetId, text, target.provider, target.model, resolveEffort(targetModel, target.effort), resolveFast(target.provider, targetModel, target.fast, refused), skill?.name ?? null, retryMessageId ?? null)
      if (result?.status === 'started' && lastSentTextRef.current?.chatId === targetId) {
        lastSentTextRef.current = { ...lastSentTextRef.current, userMessageId: result.userMessageId ?? null }
      }
      if (result?.status === 'retry_unavailable') {
        // 그 말은 다시 보낼 수 없다(이미 답을 받았거나 예전 실패). 아무것도 보내지 않았다. 글을 입력창에 넣어 새로 보낼 수 있게 한다.
        generatingRef.current = null
        setGeneratingChatId(null)
        setPendingMessage(null)
        setPrompt(text)
        showToast(W.app.retryUnavailable)
        return
      }
      if (result?.status === 'skill_unavailable') {
        // 고른 스킬이 그사이 꺼졌거나 없어졌다(말은 저장되지 않았다). 적은 말은 돌려놓고, 쓸 수 없는 스킬 조각은 빼고, 목록을 새로 읽는다.
        generatingRef.current = null
        setGeneratingChatId(null)
        giveBack(null, W.skills.unavailable)
        setChosenSkill(null)
        refreshSkills()
        return
      }
      if (result?.status === 'auth_failed') {
        // 보내기 전에 그 회사의 로그인이 받아들여지지 않았다(말은 저장되지 않았다). 적은 말을 돌려놓고 알린다.
        generatingRef.current = null
        setGeneratingChatId(null)
        setPendingMessage(null)
        setPrompt(text)
        setChosenSkill(skill)
        showToast(W.app.authFailed(PROVIDER_NAME[target.provider]))
        return
      }
      if (result?.status === 'fast_unavailable') {
        // 보내기 전에 고속 모드를 쓸 수 없다고 확인됐다(말은 저장되지 않았다). 오류 이벤트와 같게 알리고, 적은 말을 돌려놓는다.
        generatingRef.current = null
        setGeneratingChatId(null)
        setPendingMessage(null)
        setPrompt(text)
        setChosenSkill(skill)
        refuseFast(lastSendRef.current)
        return
      }
      if (result?.status === 'unavailable' || result?.status === 'not_connected') {
        // 이 AI 로는 보낼 수 없다(클로드가 연결되지 않은 not_connected 도 같다. 말은 저장되지 않았다). 고를 수 없게 하고 적은 말을 돌려놓는다(글 없음).
        generatingRef.current = null
        setGeneratingChatId(null)
        setPendingMessage(null)
        setPrompt(text)
        setSendableProviders((current) => current.filter((provider) => provider !== target.provider))
        return
      }
      // 보낸 말은 백엔드가 대화에 붙였다. 목록과 맨 위 줄을 새로 읽는다.
      void refreshChats()
      void readChat(targetId).then((document) => {
        if (selectedChatRef.current !== targetId) return
        setChat(document)
        setPendingMessage(null)
      }).catch(() => {})
    } catch (error) {
      generatingRef.current = null
      setGeneratingChatId(null)
      giveBack(error, W.app.text10)
    }
  }

  function stopResponse() {
    const running = generatingRef.current
    if (!running) return
    // 멈추면 백엔드가 chat-error(또는 chat-done)를 보낸다. 오지 않아도 화면은 바로 멈춘다.
    void cancelChatMessage(running.chatId).catch(() => {})
    finishGenerating(running.chatId)
  }

  async function renameCurrentChat(title: string) {
    if (restoreBlocks()) return
    const id = selectedChatId
    if (!id) return
    try {
      await renameChat(id, title)
      setChat((current) => (current && current.id === id ? { ...current, title } : current))
      void refreshChats()
    } catch (error) {
      showToast(archiveErrorMessage(error, W.app.text11))
    }
  }

  async function openDataRoot() {
    try {
      if (session.info && session.mode === 'ready') {
        await openRuntimeDataRoot(session.info.dataRoot)
        return
      }
      const picker = (window as DirectoryPickerWindow).showDirectoryPicker
      if (!picker) throw new Error(W.app.text12)
      await picker({ mode: 'readwrite' })
    } catch (error) {
      showToast(error instanceof Error ? error.message : String(error))
    }
  }

  function toggleList() {
    setListHidden((current) => !current)
    setListMoving(true)
    window.setTimeout(() => setListMoving(false), 300)
  }

  // 창에 파일을 끌어 놓으면 클립과 같게 지금 채팅방에 첨부한다(채팅방 화면일 때만).
  const attachDropRef = useRef<(paths: string[]) => void>(() => {})
  attachDropRef.current = (paths) => { if (screen === 'home' && !settingsOpen) void attachFiles(paths) }
  useEffect(() => {
    if (!desktopRuntime) return
    let stop: (() => void) | undefined
    let disposed = false
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === 'drop' && event.payload.paths.length > 0) attachDropRef.current(event.payload.paths)
      })
      .then((unlisten) => { if (disposed) unlisten(); else stop = unlisten })
      .catch(() => { /* 끌어 놓기를 못 받아도 클립으로 같은 일을 한다 */ })
    return () => { disposed = true; stop?.() }
  }, [desktopRuntime])

  // 오른쪽 패널을 여닫는 단추는 화면에 하나만 둔다. 패널이 닫혀 있으면 대화 칸(또는 설정) 맨 위 줄의 오른쪽 끝에,
  // 열려 있으면 패널의 탭 줄 끝에 있다.
  useEffect(() => {
    if (chatItemsSupported.current !== false) setChatItems((current) => (current === undefined && chatItemsSupported.current !== true ? undefined : null))
    refreshChatItems(selectedChatId)
  }, [selectedChatId, refreshChatItems])

  useEffect(() => {
    let stop: (() => void) | undefined
    let disposed = false
    void listenChatItems((chatId) => refreshChatItems(chatId, true)).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [refreshChatItems])

  useEffect(() => {
    if (zetPanelOpen && selectedChatId && freshInPanel[selectedChatId]) setFreshInPanel((current) => ({ ...current, [selectedChatId]: false }))
  }, [zetPanelOpen, selectedChatId, freshInPanel])

  const toggleZetPanel = () => setZetPanelOpen((current) => !current)
  const zetPanelToggle = desktopRuntime && !zetPanelOpen && screen === 'home'
    ? <ZetPanelToggle open={false} onToggle={toggleZetPanel} fresh={Boolean(selectedChatId && freshInPanel[selectedChatId])} />
    : null

  // 단축키. 대화 상자가 떠 있으면 받지 않는다.
  const shortcutActions = useRef<Record<ShortcutAction, () => void>>({
    back: () => {}, forward: () => {}, toggleList: () => {}, togglePanel: () => {}, search: () => {},
  })
  shortcutActions.current = {
    back: () => moveInHistory(-1),
    forward: () => moveInHistory(1),
    toggleList,
    // 화면에 하나뿐인 패널 단추를 누른다(패널을 닫을 때 저장하지 않은 글을 묻는 길을 그대로 탄다).
    togglePanel: () => document.querySelector<HTMLButtonElement>('.zet-panel-toggle')?.click(),
    search: () => setSearchOpen((open) => !open),
  }

  useEffect(() => {
    if (!desktopRuntime) return
    const dialogOpen = () => Boolean(document.querySelector('[aria-modal="true"]'))
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing) return
      const action = shortcutFor(event)
      if (!action) return
      event.preventDefault()
      if (dialogOpen()) return
      shortcutActions.current[action]()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [desktopRuntime])

  // 문서를 연다: 문서 화면이면 그 자리에서, 아니면 대화하기 화면의 오른쪽 패널에서.
  function openDocumentAnywhere(id: string) {
    if (screen !== 'documents') {
      if (screen !== 'home') showPlace(chatPlace)
      setZetPanelOpen(true)
    }
    window.setTimeout(() => window.dispatchEvent(new CustomEvent('momso:open-document', { detail: { id } })), 0)
  }

  // 업데이트를 마치려고 다시 켜기: 저장하지 않은 고친 문서가 있거나 AI 가 답하는 중이면 먼저 묻는다.
  function requestRestart() {
    const unsaved = Boolean(document.querySelector('.zet-tab-dirty'))
    if (unsaved || generatingChatId) setAskRestart(true)
    else void restartApp().catch(() => showToast(W.app.text13))
  }

  // 검색 창의 할 일: 지금 앱이 할 수 있는 것만.
  const searchActions: SearchAction[] = [
    { id: 'new-chat', label: W.app.text14(W.chatRoom), icon: 'new', run: startNewChat },
    { id: 'documents', label: W.app.text15(W.document), icon: 'documents', run: () => showPlace('documents') },
    { id: 'settings', label: W.app.text16, icon: 'settings', run: () => showPlace('settings') },
  ]

  // 답이 빠른 모드였을 때 붙일 이름(회사 앱의 한국어 이름, 없으면 그 모델의 빠른 모드 이름, 백엔드가 준 것).
  function fastLabelOf(provider: AiProvider | null | undefined, model: string | null | undefined) {
    if (provider && W.fastName[provider]) return W.fastName[provider] ?? null
    const found = models?.find((list) => list.provider === provider)?.models.find((item) => item.id === model)
    return found?.fast?.label?.trim() || null
  }

  // 답이 오지 않았을 때의 한 줄(G18): 까닭마다 쉬운 말, 회사가 다시 쓸 수 있는 때를 주면 그 때, 지금 할 수 있는 일을 단추로.
  const failureView = (() => {
    const failure = selectedChatId ? chatFailures[selectedChatId] : undefined
    if (!failure) return null
    const name = PROVIDER_NAME[failure.provider]
    const base = failure.kind === 'authentication' ? W.app.authFailed(name) : W.app.failure[failure.kind]?.(name) ?? W.app.text7
    const when = retryTimeText(failure.retryAt)
    const text = when && (failure.kind === 'usageLimit' || failure.kind === 'rateLimited') ? `${base} ${W.app.retryAt(when)}` : base
    const actions: Array<{ id: string; label: string; run: () => void }> = []
    const has = (...ids: string[]) => ids.some((id) => failure.actions.includes(id))
    // 다시 보내기는 그 말의 id 를 알 때만(같은 말을 새로 남기지 않는다).
    if (has('retry') && failure.userMessageId) actions.push({ id: 'retry', label: W.app.failureAction.retry, run: () => void sendPrompt(failure.text, failure.userMessageId ?? undefined) })
    if (has('signIn', 'checkApiKey', 'checkBilling', 'install')) actions.push({ id: 'settings', label: W.app.failureAction.settings, run: () => openSettingsAt('ai') })
    if (has('switchModel')) actions.push({ id: 'switchModel', label: W.app.failureAction.switchModel, run: () => window.dispatchEvent(new CustomEvent('momso:open-picker', { detail: 'model' })) })
    if (has('shorten')) actions.push({ id: 'shorten', label: W.app.failureAction.shorten, run: () => startNewChat() })
    if (has('switchProvider')) actions.push({ id: 'switchProvider', label: W.app.failureAction.switchProvider, run: () => window.dispatchEvent(new CustomEvent('momso:open-picker', { detail: 'company' })) })
    return { text, actions }
  })()

  const shownChat = chat && chat.id === selectedChatId ? chat : null
  const messages: ChatViewMessage[] = [
    ...(shownChat?.messages ?? []).map((message) => ({
      id: message.id,
      role: message.role,
      text: message.text,
      provider: message.role === 'assistant' ? message.provider ?? null : null,
      // 고속 모드는 확인된 true 일 때만, 넘기지 못한 말은 1 이상일 때만 적는다(null 이면 아무것도 적지 않는다).
      // 실제로 쓰인 추론 수준(actualEffort)은 확인된 값이 올 때만 적는다(T43, 제미나이는 오지 않는다).
      modelLabel: [
        findModelName(models, message.provider, message.model),
        message.actualEffort && message.provider ? effortKorean(message.provider, message.actualEffort, message.actualEffort)?.label ?? message.actualEffort : null,
        // 실제 고속 상태(fastState, 세 회사 계약): on 일 때만 적는다. 옛 답은 fastState 가 없어 예전 값(fast === true)을 본다.
        (message.fastState === 'on' || (message.fastState == null && message.fast === true)) ? fastLabelOf(message.provider, message.model) : null,
      ].filter(Boolean).join(' · ') || null,
      endReason: message.role === 'assistant' && message.finishReason && message.finishReason !== 'completed' ? message.finishReason : null,
      notHandedOver: typeof message.notHandedOver === 'number' && message.notHandedOver > 0 ? message.notHandedOver : 0,
      skill: message.skill ? skills?.find((item) => item.name === message.skill)?.title ?? message.skill : null,
      createdAt: message.createdAt,
    })),
    ...(pendingMessage ? [pendingMessage] : []),
  ]
  shownCountRef.current = messages.length
  // 물음이 온 대화(아직 저장 전인 새 대화면 null)와 지금 보는 대화가 같으면 보인다.
  const chatApprovals = approvals.filter((item) => item.chatId === selectedChatId)
  const chatSecrets = secretRequests.filter((item) => item.request.chatId === selectedChatId)
  const waitingChatIds = [...new Set([
    ...approvals.filter((item) => item.status !== 'answered' && item.chatId).map((item) => item.chatId as string),
    ...secretRequests.filter((item) => (item.status === 'pending' || item.status === 'sending') && item.request.chatId).map((item) => item.request.chatId as string),
  ])]

  // 첫 실행: 몸소 첫 화면 → 영어 사용자 이름 → AI 하나 이상 연결(건너뛸 수 없음). 이미 쓰던 사람에게는 뜨지 않는다(first-run.ts).
  if (firstRun) {
    return (
      <div className="desktop-shell has-window-titlebar">
        <WindowTitlebar appearance="welcome" />
        {firstRun === 'checking'
          ? <main className="first-run" aria-busy="true" />
          : <FirstRun username={firstRun.username} nameStep={firstRun.nameStep} onDone={() => setFirstRun(null)} />}
      </div>
    )
  }

  return (
    <div className={desktopRuntime ? 'desktop-shell has-window-titlebar' : 'desktop-shell'}>
      {desktopRuntime && (
        <WindowTitlebar
          listHidden={listHidden}
          onToggleList={toggleList}
          onOpenSearch={() => setSearchOpen(true)}
          canGoBack={canGoBack}
          canGoForward={canGoForward}
          onBack={() => moveInHistory(-1)}
          onForward={() => moveInHistory(1)}
        />
      )}
      <div
        ref={zetPanelFrame.bodyRef}
        className="workbench-body"
        data-zet-panel={panelShown ? 'open' : 'closed'}
        data-zet-moving={zetPanelFrame.moving}
        data-zet-resizing={zetPanelFrame.resizing}
        data-list-hidden={listHidden}
        data-list-moving={listMoving}
        data-screen={screen}
        style={{ '--zet-panel-width': `${zetPanelFrame.width}px` } as CSSProperties}
      >
      <div className="workbench-grid">
        <ActivityRail
          screen={screen}
          mediaAvailable={mediaAvailable}
          onNavigate={(next) => showPlace(next === 'home' ? chatPlace : next)}
          update={update}
          onRestart={requestRestart}
          providerAuth={providerAuth.list}
        />

        {screen === 'documents' ? null : screen === 'media' ? (
          <MediaView onOpenChat={(id) => { void openChat(id); showPlace(`chat:${id}`) }} onNotice={showToast} />
        ) : settingsOpen ? (
          <SettingsView
            key={settingsStart}
            initialSection={settingsStart}
            headerActions={zetPanelToggle}
            onClose={closeSettings}
            onOpenDataRoot={() => void openDataRoot()}
            update={update}
            skills={skills}
            onSkillsChanged={refreshSkills}
            skillsFailed={skillsFailed}
            skillsLoading={desktopRuntime && !skillsLoaded}
            archivedChats={chatKeep || (desktopRuntime && !chatKeepKnown)}
            onChatsChanged={() => void refreshChats()}
          />
        ) : (
          <>
            <ChatSidebar
              chats={chats}
              waitingChatIds={waitingChatIds}
              selectedChatId={selectedChatId}
              error={chatsError}
              loading={desktopRuntime && !chatsLoaded && session.mode !== 'error'}
              loadingDetail={startupDetail}
              onLoadMore={() => void loadMoreChats()}
              onRetry={() => void refreshChats()}
              restoring={chatRestore}
              onNewChat={startNewChat}
              onSelectChat={selectChat}
              groups={groups}
              onCreateGroup={async () => {
                try {
                  const created = await createChatGroup(W.app.text17(GROUP_WORD))
                  await refreshGroups()
                  return created.id
                } catch (error) {
                  showToast(archiveErrorMessage(error, W.app.text18(GROUP_WORD)))
                  return null
                }
              }}
              onRenameGroup={(id, name) => void changeGroups(() => renameChatGroup(id, name), W.app.text11)}
              onMoveGroup={(id, step) => {
                const ids = [...(groups ?? [])].sort((a, b) => a.order - b.order).map((group) => group.id)
                const at = ids.indexOf(id)
                const to = at + step
                if (at < 0 || to < 0 || to >= ids.length) return
                ;[ids[at], ids[to]] = [ids[to], ids[at]]
                void changeGroups(() => reorderChatGroups(ids), W.app.text19)
              }}
              onRemoveGroup={(id) => void changeGroups(() => removeChatGroup(id), W.app.text20(GROUP_WORD))}
              onSetGroup={(chatId, groupId) => void changeGroups(() => setChatGroup(chatId, groupId), W.app.text21(GROUP_WORD))}
              onPinChat={chatKeep ? (chatId, pinned) => void pinChat(chatId, pinned) : undefined}
              onArchiveChat={chatKeep ? (chatId) => void keepChatAway(chatId) : undefined}
              busyChatId={generatingChatId}
            />
            <AssistantPanel
              failure={failureView}
              headerActions={zetPanelToggle}
              title={shownChat?.title ?? ''}
              canRename={Boolean(shownChat)}
              messages={messages}
              isGenerating={generatingChatId !== null && generatingChatId === selectedChatId}
              streamText={streamText}
              onRename={(title) => void renameCurrentChat(title)}
              approvals={chatApprovals}
              notices={[
                ...compactions.filter((item) => item.chatId === selectedChatId).map((item) => ({ id: item.id, at: item.at, text: W.app.text22 })),
                ...(selectedChatId && (redactions[selectedChatId] ?? 0) > 0
                  ? [{ id: `redacted-${selectedChatId}`, at: Number.MAX_SAFE_INTEGER, text: W.app.chatRedacted(redactions[selectedChatId]) }]
                  : []),
                ...(selectedChatId && browserHuman[selectedChatId]
                  ? [{ id: `browser-${selectedChatId}`, at: browserHuman[selectedChatId].at, text: `${W.app.browserHuman[browserHuman[selectedChatId].reason] ?? W.app.browserHuman.verification} ${W.app.browserHumanAfter}` }]
                  : []),
              ]}
              browserOpen={Boolean(selectedChatId && browserOpen[selectedChatId])}
              onAnswerApproval={(requestId, choice) => void answerApproval(requestId, choice)}
              secretRequests={chatSecrets}
              onSubmitSecret={(requestId, values) => void answerSecret(requestId, values)}
              onCancelSecret={(requestId) => void answerSecret(requestId, null)}
              onOpenDocument={(id) => {
                // 오른쪽 패널을 열고 그 문서를 탭으로 연다.
                setZetPanelOpen(true)
                window.setTimeout(() => window.dispatchEvent(new CustomEvent('momso:open-document', { detail: { id } })), 0)
              }}
              composer={(
                <Composer
                  prompt={prompt}
                  isGenerating={generatingChatId !== null && generatingChatId === selectedChatId}
                  onPromptChange={setPrompt}
                  onSend={() => void sendPrompt()}
                  skills={skills ? skills.filter((item) => item.enabled) : null}
                  skill={chosenSkill}
                  onSkillChange={setChosenSkill}
                  sendReady={sendReady}
                  onStop={stopResponse}
                  onAttach={() => void pickAndAttach()}
                  models={models}
                  modelsState={modelsState}
                  onRetryModels={() => void refreshModels()}
                  fallbackProviders={FALLBACK_PROVIDERS}
                  sendableProviders={sendableProviders}
                  chosen={chosen}
                  onChooseProvider={chooseProvider}
                  onChooseModel={chooseFamily}
                  onChooseEffort={chooseEffort}
                  onToggleFast={toggleFast}
                  fastRefused={Boolean(fastRefused[`${chosen.provider}:${chosen.model ?? ''}`])}
                  permission={permission}
                  onPermissionChange={(mode) => void changePermission(mode)}
                  onVoiceProblem={showToast}
                  context={selectedChatId ? chatContext : null}
                />
              )}
            />
          </>
        )}
      </div>
      {desktopRuntime && (
        <ZetPanel
          open={panelShown}
          variant={screen === 'documents' ? 'page' : 'side'}
          width={zetPanelFrame.width}
          maxWidth={zetPanelFrame.maxWidth}
          onWidthChange={zetPanelFrame.changeWidth}
          onResizingChange={zetPanelFrame.setResizing}
          onClose={() => setZetPanelOpen(false)}
          onToggle={toggleZetPanel}
          onNotice={showToast}
          chatId={selectedChatId}
          chatItems={chatItems}
          chatItemsFailed={chatItemsFailed}
          onRetryChatItems={() => refreshChatItems(selectedChatRef.current)}
          onOpenChat={(id) => { void openChat(id); showPlace(`chat:${id}`) }}
          onOpenMedia={() => showPlace('media')}
        />
      )}
      </div>

      {askRestart && (
        <QuestionDialog
          title={W.app.title1}
          text={W.app.text23}
          actions={[
            { label: W.app.text24, onSelect: () => setAskRestart(false), autoFocus: true },
            { label: W.app.text25, primary: true, onSelect: () => { setAskRestart(false); void restartApp().catch(() => showToast(W.app.text13)) } },
          ]}
          onDismiss={() => setAskRestart(false)}
        />
      )}

      {searchOpen && (
        <SearchDialog
          chats={chats}
          groups={groups}
          actions={searchActions}
          onClose={() => setSearchOpen(false)}
          onOpenChat={(id) => showPlace(`chat:${id}`)}
          onOpenDocument={openDocumentAnywhere}
          onOpenMedia={() => showPlace('media')}
        />
      )}

      {(toast || session.error) && (
        <div className="workbench-toast" role="status">
          {/* 앱 시작(runtime_info)이 실패하면 뒤쪽 글을 그대로 보이지 않는다: 한글 글이면 그대로, 영어·코드 값이면 일반 글과 다시 시도(2026-11-17). */}
          {toast ?? archiveErrorMessage(session.error, W.app.runtimeFailed)}
          {toast && toastAction && (
            <button className="secondary-button workbench-toast-action" type="button" onClick={() => { const run = toastAction.run; setToast(null); setToastAction(null); run() }}>{toastAction.label}</button>
          )}
          {!toast && session.error && (
            <button className="secondary-button workbench-toast-action" type="button" onClick={() => void refreshRuntime()}>{W.firstRun.retry}</button>
          )}
        </div>
      )}
    </div>
  )
}

export default App
