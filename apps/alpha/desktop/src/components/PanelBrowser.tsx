import { useCallback, useEffect, useRef, useState, useLayoutEffect } from 'react'
import { createPortal } from 'react-dom'
import { ArrowLeft, ArrowRight, ExternalLink, KeyRound, LoaderCircle, Lock, LockOpen, Puzzle, RefreshCw, RotateCw, X } from 'lucide-react'
import { openWebAddress } from '../archive'
import { SavedLoginsCard } from './SavedLoginsCard'
import { openAndFillSavedLogin, type SavedLogin } from '../saved-logins'
import { isMissingCommand } from '../ai-models'
import { closePanelOverlay, LOGIN_CARD_MARGIN, listenOverlayFilled, listenPanelOverlayClosed, openPanelOverlay, overlayKind } from '../panel-overlay'
import { extensionErrorCode, listenExtensionsChanged, listenExtensionsPage, listExtensions, openExtensionPopup, pinnedExtensions, type ExtensionsSection, type PanelExtension } from '../panel-extensions'
import { ExtensionsManager } from './ExtensionsManager'
import { ExtensionIcon, ExtensionsMenu } from './ExtensionsMenu'
import {
  closePanelBrowser,
  listenPanelAgent,
  listenPanelBrowser,
  openPanelAgent,
  panelAgentStatus,
  resumePanelAgent,
  takeOverPanel,
  navigatePanelBrowser,
  openExternalLogin,
  openHumanBrowser,
  openPanelBrowser,
  panelBrowserAction,
  panelBrowserError,
  setPanelBrowserBounds,
  setPanelBrowserShown,
  setPanelBrowserVisible,
  selectDownloadForAttach,
  revealDownload,
  listenPanelPopup,
  type PanelPopup,
  typedAddress,
  useAvoidPanelBrowser,
  usePanelBrowserHeld,
  usePanelChat,
  type PanelBounds,
  type PanelBrowserError,
  type PanelPage,
} from '../panel-browser'
import { W } from '../words'
import type { SourceNotice } from './SourceDelete'

const B = W.panelBrowser

/**
 * 오른쪽 패널의 브라우저 탭 안(2026-11-17 본부 결정, ui-panel-browser-plan-20261117.md).
 * 맨 위 한 줄은 문서 탭의 zet-viewbar 줄 꼴 그대로: 뒤로·앞으로·새로 고침(불러오는 중에는 멈추기), 자물쇠와 호스트(누르면 전체 주소를 고치는 칸),
 * 저장된 로그인(열쇠, 2026-11-21: 어사이드처럼 주소 칸 오른쪽. 누르면 단추 아래에 떠 있는 카드), 기본 브라우저로 열기.
 * 그 아래 빈 자리를 재서 뒤쪽 웹뷰를 그 위에 띄운다. 호스트·자물쇠는 뒤쪽 이벤트의 값만 쓴다(사이트 화면 안의 글은 쓰지 않는다).
 *
 * 채팅방마다 하나(2026-11-21, panel-browser.ts "채팅방마다 따로"): 이 부품은 chatId 방의 브라우저다. 뒤쪽이 그 방을 고른 뒤에만
 * 열고 움직이며, 다른 방의 이벤트·응답은 그리지 않는다. 방을 옮겨도 부품은 남아(숨김) 그 방의 주소·제목을 기억한다.
 */
export function PanelBrowserView({
  url,
  seq,
  agentOpen = false,
  chatId = null,
  active,
  onTitle,
  onOpenFailed,
  onNotice,
}: {
  /** 열어 달라는 주소. seq 가 바뀌면 같은 주소라도 다시 연다. null 이면 빈 브라우저(주소 칸만). */
  url: string | null
  seq: number
  /** url 이 null 일 때 AI(또는 로그인)가 열어 달라고 했으면 참: 빈 패널을 곧바로 연다. 거짓이면 주소를 칠 때까지 열지 않는다. */
  agentOpen?: boolean
  /** 이 브라우저의 채팅방. 새 채팅방이면 뒤쪽이 준 임시 방 번호(첫 말로 방이 생기면 그 방 번호로 바뀌고 웹뷰는 그대로). */
  chatId?: string | null
  /** 이 탭이 지금 보이는가(패널이 열려 있고 이 방의 브라우저가 고른 탭). */
  active: boolean
  onTitle: (title: string | null, host: string | null) => void
  /** 처음 열기가 페이지 없이 실패했을 때(탭을 거둔다). */
  onOpenFailed: (code: PanelBrowserError) => void
  onNotice: SourceNotice
}) {
  const spot = useRef<HTMLDivElement>(null)
  const [opened, setOpened] = useState(false)
  const [failed, setFailed] = useState(false)
  const [page, setPage] = useState<PanelPage | null>(null)
  const [history, setHistory] = useState<{ canGoBack: boolean | null; canGoForward: boolean | null }>({ canGoBack: null, canGoForward: null })
  const [editing, setEditing] = useState<string | null>(null)
  // 저장된 로그인 카드가 떠 있는가(열쇠 단추). 카드 자리는 열쇠 단추 바로 아래, 단추 오른쪽 끝에 맞춘다(어사이드 꼴).
  const [logins, setLogins] = useState(false)
  const [cardAt, setCardAt] = useState<{ top: number; right: number; width: number; height: number } | null>(null)
  const keyButton = useRef<HTMLButtonElement>(null)
  const loginCard = useRef<HTMLElement>(null)
  const browserBox = useRef<HTMLDivElement>(null)
  // 이번 누름이 카드(또는 카드에서 연 묻는 창, 열쇠 단추) 안에서 시작됐는가. React 이벤트는 포털을 거쳐서도 카드로 올라오고,
  // document 의 듣기보다 먼저 돈다. 그래서 바깥 누름만 카드를 닫는다.
  const pressInside = useRef(false)
  // 아래 logins 카드는 떠 있는 창이 없는 뒤쪽(가짜 화면·옛 앱)일 때만 쓴다: 웹뷰가 따로 뜨는 층이라 떠 있는 동안 웹뷰를 숨긴다.
  useAvoidPanelBrowser(loginCard, logins)
  // 떠 있는 창(S13, panel-overlay.ts)으로 연 카드: 페이지를 숨기지 않고 그 위에 뜬다(어사이드 꼴). 창 id 와, 열쇠를 누르는 사이 바깥 누름으로 닫힌 때.
  const [overlayId, setOverlayId] = useState<string | null>(null)
  const overlayIdRef = useRef<string | null>(null)
  const overlayMissing = useRef(false)
  const overlayClosedAt = useRef(0)
  const held = usePanelBrowserHeld()
  const lastBounds = useRef<string>('')
  const openedRef = useRef(false)
  const pageRef = useRef<PanelPage | null>(null)
  pageRef.current = page
  const titleRef = useRef<string | null>(null)
  const callbacks = useRef({ onTitle, onOpenFailed, onNotice })
  callbacks.current = { onTitle, onOpenFailed, onNotice }
  // 뒤쪽이 이 방을 골랐는가(panel_browser_select_chat 이 끝남). 고르기 전에는 명령을 보내지 않는다.
  const panelChat = usePanelChat()
  const selected = !!chatId && panelChat.chatId === chatId
  const hasPanel = selected && panelChat.hasPanel
  const chatRef = useRef(chatId)
  chatRef.current = chatId
  const selectedRef = useRef(selected)
  selectedRef.current = selected
  // 마지막으로 처리한 열기 요청(seq|url|agent). 방을 오갈 때 같은 요청을 다시 처리하지 않게.
  const handledRef = useRef('')

  function measure(): PanelBounds | null {
    const rect = spot.current?.getBoundingClientRect()
    if (!rect || rect.width < 1 || rect.height < 1) return null
    return { x: Math.round(rect.left), y: Math.round(rect.top), width: Math.round(rect.width), height: Math.round(rect.height) }
  }

  // 뒤쪽 이벤트. 페이지·제목·앞뒤는 이 방(chatId) 것만. 알림 줄이 뜨는 것(다운로드·Google 로그인)은 이 방 것이고 뒤쪽이 이 방을 골랐을 때만
  // (다른 방에서 받은 파일의 [채팅방에 첨부]가 지금 방에 붙지 않게). 방 번호가 없는 옛 뒤쪽이면 고른 방인지만 본다.
  useEffect(() => {
    let stop: (() => void) | undefined
    let disposed = false
    const mine = (owner: string | null) => !!owner && owner === chatRef.current
    const here = (owner: string | null) => selectedRef.current && (owner === null || owner === chatRef.current)
    void listenPanelBrowser({
      onPage: (next, owner) => {
        if (!mine(owner)) return
        setPage(next)
        if (!titleRef.current) callbacks.current.onTitle(null, next.host)
      },
      onTitle: (title, owner) => {
        if (!mine(owner)) return
        titleRef.current = title.trim() || null
        callbacks.current.onTitle(titleRef.current, pageRef.current?.host ?? null)
      },
      onHistory: (next, owner) => { if (mine(owner)) setHistory(next) },
      // 다운로드(2026-11-18 새 꼴): 받는 중 · 다 받음(채팅방에 첨부할 수 있음) · 막음(실행 파일) · 실패. 알림 줄 한 줄씩.
      onDownload: (download, owner) => {
        if (!here(owner)) return
        const name = download.name
        if (download.state === 'started') { callbacks.current.onNotice(name ? B.downloading(name) : B.downloadingFile); return }
        if (download.state === 'blocked') { callbacks.current.onNotice(name ? B.downloadBlocked(name) : B.downloadBlockedFile); return }
        if (download.state === 'finished') {
          const path = download.path
          callbacks.current.onNotice(name ? B.downloaded(name) : B.downloadedFile, path ? [{
            label: B.attachDownload,
            run: () => {
              // 다 받은 파일을 첨부 선택으로 기록한 뒤 첨부한다(끌어 놓기·클립과 같은 길).
              void selectDownloadForAttach(path)
                .then(() => window.dispatchEvent(new CustomEvent('momso:attach-paths', { detail: { paths: [path] } })))
                .catch(() => callbacks.current.onNotice(W.attach.failed))
            },
          }, {
            label: B.revealDownload,
            run: () => { void revealDownload(path).catch(() => callbacks.current.onNotice(B.revealFailed)) },
          }] : undefined)
          return
        }
        callbacks.current.onNotice(name ? B.downloadFailed(name) : B.downloadFailedFile)
      },
      onGoogleLogin: (siteUrl, owner) => {
        if (!here(owner)) return
        callbacks.current.onNotice(siteUrl ? B.google : B.googleNoSite, siteUrl ? {
          label: B.openHuman,
          run: () => { void openHumanBrowser(siteUrl).catch((error) => callbacks.current.onNotice(panelBrowserError(error) === 'browser_missing' ? B.browserMissing : B.humanFailed)) },
        } : undefined)
      },
    }).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [])

  // 이 방이 다시 골라졌는데 뒤쪽에 그 방의 패널이 없으면(hasPanel 거짓) 처음부터 다시 연다. 다른 방에 있는 동안은 자리를 다시 재게 둔다.
  useEffect(() => {
    if (!selected) { lastBounds.current = ''; return }
    if (hasPanel || !openedRef.current) return
    openedRef.current = false
    handledRef.current = ''
    titleRef.current = null
    setOpened(false)
    setPage(null)
    setHistory({ canGoBack: null, canGoForward: null })
    callbacks.current.onTitle(null, null)
  }, [selected, hasPanel])

  // 웹뷰를 이 방에 연다(이미 있으면 그것을 쓴다). target 이 null 이면 빈 패널. 끝나면 참.
  async function openHere(target: string | null, want: string, isCancelled: () => boolean) {
    const room = chatRef.current
    // 자리가 그려지고 멈출 때까지 기다린다. 패널이 열리는 중이면 폭이 0 이거나 오른쪽에서 밀려 들어오는 중이다.
    // 움직이는 중에 재면 웹뷰가 엉뚱한 자리(오른쪽)에 놓였다(2026-11-18 실제 앱 B72: 웹뷰 x 1181, 자리 x 780.6). 50ms 간격 두 번이 같을 때까지(최대 2초).
    let bounds = measure()
    for (let i = 0; i < 40 && !isCancelled(); i += 1) {
      await new Promise((r) => setTimeout(r, 50))
      const next = measure()
      const still = Boolean(bounds && next && JSON.stringify(bounds) === JSON.stringify(next))
      bounds = next
      if (still) break
    }
    if (isCancelled() || !bounds) return false
    try {
      // AI 의 열기 요청이나 이 방에 이미 있는 패널이면 빈 패널로 연다(주소는 AI 가 승인 뒤 연다).
      if (target === null) await openPanelAgent(room, bounds)
      else await openPanelBrowser(room, target, bounds)
    } catch (error) {
      const code = panelBrowserError(error)
      // 그새 다른 방을 골랐다. 이 방을 다시 고르면 다시 연다(탭을 거두지 않는다).
      if (code === 'panel_not_active') return false
      if (code !== 'already_open') {
        if (code === 'unavailable') { setFailed(true); return false }
        callbacks.current.onOpenFailed(code)
        return false
      }
      // 이 방에 앞서 연 웹뷰가 남아 있으면 그 웹뷰를 쓴다.
      if (target !== null) { try { await navigatePanelBrowser(room, target) } catch (navError) { report(panelBrowserError(navError)) } }
      await setPanelBrowserBounds(room, bounds).catch(() => {})
    }
    if (isCancelled() || room !== chatRef.current) return false
    openedRef.current = true
    handledRef.current = want
    lastBounds.current = JSON.stringify(bounds)
    setFailed(false)
    setOpened(true)
    return true
  }

  // 처음 열기와 다시 열기(주소·seq 가 바뀌면). 뒤쪽이 이 방을 고른 뒤에만.
  const want = `${seq}|${url ?? ''}|${agentOpen ? 'agent' : ''}`
  useEffect(() => {
    let cancelled = false
    async function go() {
      if (!selected || !chatRef.current) return
      if (openedRef.current) {
        if (handledRef.current === want) return
        handledRef.current = want
        if (url === null) return
        try { await navigatePanelBrowser(chatRef.current, url) } catch (error) { report(panelBrowserError(error)) }
        return
      }
      // 빈 브라우저(주소 칸만)는 주소를 칠 때까지 열지 않는다. 이 방에 패널이 이미 있으면(hasPanel) 그것을 붙인다.
      if (url === null && !agentOpen && !hasPanel) return
      await openHere(url, want, () => cancelled)
    }
    void go()
    return () => { cancelled = true }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [want, selected, hasPanel])

  // 탭을 닫으면 그 방의 웹뷰도 닫는다.
  useEffect(() => () => {
    if (openedRef.current) void closePanelBrowser(chatRef.current).catch(() => {})
    openedRef.current = false
  }, [])

  // 보임: 이 탭이 보이고, 뒤쪽이 이 방을 골랐고, 몸소 것이 덮고 있지 않을 때만.
  // 다른 방을 고르면 뒤쪽이 이 방의 웹뷰를 숨긴다. 돌아오면 뒤쪽이 웹뷰를 보이므로 지금 보여야 하는지 다시 알린다.
  // 확장 프로그램 관리 화면(어사이드처럼 탭 자리를 쓰는 화면): 열려 있는 동안 페이지(웹뷰)는 숨기고 그 자리에 관리 화면.
  const [manage, setManage] = useState<ExtensionsSection | null>(null)
  const visible = opened && active && !held && selected && !manage
  useEffect(() => {
    if (!opened || !selected) return
    void setPanelBrowserVisible(chatRef.current, visible).catch(() => {})
  }, [opened, visible, selected])
  useEffect(() => {
    if (!(opened && active && selected)) return
    setPanelBrowserShown(true)
    return () => setPanelBrowserShown(false)
  }, [opened, active, selected])

  // 자리: 보이는 동안 화면 그림마다 재서 바뀌었을 때만 알린다(창·최대화·배율·패널 폭·패널이 열리는 움직임 모두).
  // 보내기는 한 번에 하나씩. 보내는 중에 자리가 또 바뀌면 끝난 뒤 마지막 자리를 보낸다(두 요청이 거꾸로 끝나 옛 자리가 남지 않게).
  const follow = useRef(() => {})
  const sending = useRef(false)
  follow.current = () => {
    if (!visible || sending.current) return
    const bounds = measure()
    const key = bounds ? JSON.stringify(bounds) : ''
    if (bounds && key !== lastBounds.current) {
      sending.current = true
      // 못 옮기면 곧바로 다시 하지 않고 다음 그림·시계 때 다시 한다.
      void setPanelBrowserBounds(chatRef.current, bounds)
        .then(() => { lastBounds.current = key; sending.current = false; follow.current() })
        .catch(() => { sending.current = false })
    }
  }
  // 자리가 바뀔 수 있을 때만 잰다(2026-11-18 G71: 매 그림·250ms 시계가 빈 패널에서 CPU 를 썼다).
  //   자리 칸의 크기 바뀜(ResizeObserver: 창 크기·최대화·패널 폭·위 줄 생김), 창 크기·배율 바뀜, 패널이 밀려 들어오는 움직임이 끝남,
  //   그리고 이 부품이 다시 그려질 때(아래 useLayoutEffect, 그림이 멈춘 창에서도 돈다).
  useEffect(() => {
    if (!visible || !spot.current) return
    const run = () => follow.current()
    const observer = new ResizeObserver(run)
    observer.observe(spot.current)
    window.addEventListener('resize', run)
    document.addEventListener('transitionend', run, true)
    document.addEventListener('animationend', run, true)
    run()
    return () => {
      observer.disconnect()
      window.removeEventListener('resize', run)
      document.removeEventListener('transitionend', run, true)
      document.removeEventListener('animationend', run, true)
    }
  }, [visible])
  // 패널 위 줄(AI 가 쓰는 중·로그인 안내·팝업 출처)이 생기거나 사라지면 그 그림 전에 곧바로 웹뷰를 내리거나 올린다(2026-11-18 실제 앱 B71).
  // 화면 그림마다 도는 것만으로는, 창이 가려졌거나 숨겨져 그림이 멈추면 웹뷰가 옛 자리에 남아 줄을 덮었다.
  useLayoutEffect(() => { follow.current() })

  function report(code: PanelBrowserError) {
    // Google 로그인은 뒤쪽 이벤트가 따로 알린다(두 번 알리지 않는다). 다른 방을 골라 거절된 것은 알리지 않는다.
    if (code === 'google_login_required' || code === 'panel_not_active') return
    callbacks.current.onNotice(code === 'invalid_url' ? W.zetPanel.text18 : B.openFailed)
  }

  // 팝업(로그인 창 등): 열려 있는 동안 위쪽에 한 줄로 어느 사이트에서 연 창인지(호스트는 뒤쪽 값만). 닫히면 거둔다.
  const [popup, setPopup] = useState<PanelPopup | null>(null)
  useEffect(() => {
    let stop: (() => void) | undefined
    let disposed = false
    void listenPanelPopup((next, owner) => {
      // 이 방의 팝업만(뒤쪽은 고른 방의 팝업만 보인다). 방 번호가 없는 옛 뒤쪽이면 고른 방인지만 본다.
      if (!selectedRef.current || (owner !== null && owner !== chatRef.current)) return
      if (next.state === 'opened') setPopup(next)
      else if (next.state === 'closed') setPopup((current) => (!current || !next.host || current.host === next.host ? null : current))
    }).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [])

  // AI 가 이 패널을 쓰는지(panel-agent-control-changed). started → AI 가 쓰는 중(직접 쓰기 단추), human → 내가 쓰는 중(AI에게 돌려주기 단추),
  // ended·available → 줄 없음. 처음에는 panel_agent_status 로 내가 가져가 있는지만 본다(AI 가 지금 움직이는지는 이벤트로만 안다).
  // login(2026-11-18 G57b): 로그인·인증을 사람이 해야 함. 끝나면 [AI에게 돌려주기]. 구글 로그인은 바깥 창(몸소 브라우저 창)에서.
  // 채팅방 이름(title)은 지금 보는 채팅방이 아닐 때 "○○ 채팅방의 AI가…"로만 쓴다. 회사 이름은 줄을 짧게 두려고 적지 않는다.
  // external: 로그인 뒤 AI 가 몸소 브라우저 창(바깥)에서 이어 하는 중(G75). login 의 reason external_opened 는 그 창을 연 뒤.
  type AgentLine = { chatId: string; state: 'ai' | 'human' | 'login' | 'external'; reason: string | null; title: string | null; loginUrl?: string | null }
  const [agent, setAgent] = useState<AgentLine | null>(null)
  const [agentWorking, setAgentWorking] = useState(false)
  useEffect(() => {
    let stop: (() => void) | undefined
    let disposed = false
    // 이 방의 AI 것만 듣는다(다른 방의 AI 는 이 방의 브라우저를 쓰지 못한다. 뒤쪽은 panel_not_active 로 멈춘다).
    const mine = (owner: string) => owner === chatRef.current
    void listenPanelAgent({
      onControl: ({ chatId: owner, state, reason, chatTitle, loginUrl }) => {
        if (!mine(owner)) return
        if (state === 'started') setAgent({ chatId: owner, state: 'ai', reason: null, title: chatTitle })
        else if (state === 'human') setAgent({ chatId: owner, state: 'human', reason: null, title: chatTitle })
        else if (state === 'login') setAgent({ chatId: owner, state: 'login', reason, title: chatTitle, loginUrl })
        // AI 의 턴이 끝나도(ended, 뒤쪽 close_panel_turn) 사람이 로그인해야 하는 줄은 남긴다(2026-11-18 실제 앱: 코덱스는 답이 끝나며 줄이 사라졌다).
        // 사람이 [AI에게 돌려주기](→ available), 다른 턴 시작(started), 직접 쓰기(human), 패널 닫기 때 거둔다.
        else if (state === 'ended') setAgent((current) => (current && current.state === 'login' && current.chatId === owner && reason !== 'panel_not_opened' ? current : null))
        else if (state === 'available') setAgent(null)
      },
      // 바깥 로그인(G75): 몸소 브라우저 창이 열리면 "그 창에서 로그인한 뒤 돌려주세요", AI 가 그 창에서 이어 하면 "…이어서 하고 있습니다".
      onExternalOpened: (owner) => mine(owner) && setAgent((current) => (current && current.chatId === owner ? { ...current, state: 'login', reason: 'external_opened' } : { chatId: owner, state: 'login', reason: 'external_opened', title: null })),
      onExternalContinued: (owner, state) => {
        if (mine(owner) && state === 'started') setAgent((current) => ({ chatId: owner, state: 'external', reason: null, title: current?.chatId === owner ? current.title : null }))
      },
      // [AI에게 돌려주기] 뒤 이어 하기 턴이 시작됨(G68): 줄을 "AI가 이 패널을 쓰고 있습니다"로.
      onResumeStarted: ({ chatId: owner }) => mine(owner) && setAgent((current) => ({ chatId: owner, state: 'ai', reason: null, title: current?.chatId === owner ? current.title : null })),
      // 몸소 브라우저의 "사람이 해야 함"이 패널 AI 의 채팅방에서 오면 같은 로그인 줄로(까닭 이름은 panel-agent-control-changed 의 것으로 맞춤).
      onHumanRequired: (owner, reason, loginUrl) => {
        if (!mine(owner)) return
        const mapped = reason === 'external_login' ? 'google_login_external'
          : reason === 'login' || reason === 'password' ? 'login_page'
            : reason === 'one_time_code' || reason === 'captcha' ? reason : 'verification'
        setAgent((current) => (current && current.chatId === owner && current.state !== 'human'
          ? { ...current, state: 'login', reason: mapped, loginUrl: loginUrl ?? current.loginUrl }
          : current))
      },
    }).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [])
  useEffect(() => {
    if (!chatId) return
    let alive = true
    panelAgentStatus(chatId).then((value) => {
      if (alive && value.humanControls && value.ownerChatId) setAgent((current) => current ?? { chatId: value.ownerChatId as string, state: 'human', reason: null, title: null })
    }).catch(() => {})
    return () => { alive = false }
  }, [chatId])
  function control(kind: 'take' | 'resume') {
    if (!agent) return
    setAgentWorking(true)
    const run = kind === 'take' ? takeOverPanel(agent.chatId) : resumePanelAgent(agent.chatId)
    void run
      .catch((error) => {
        const code = String(error)
        // 로그인 뒤 돌려줬는데 뒤쪽이 아직 로그인 전으로 보면 human_required.
        // 이어 하기 오류(G68): login_incomplete 는 아직 로그인 화면, 그 밖은 까닭별 말.
        const known = Object.keys(B.resumeFailed).find((key) => code.includes(key))
        callbacks.current.onNotice(code.includes('panel_busy') ? B.agentBusy
          : code.includes('human_required') || code.includes('login_incomplete') ? B.agentLoginNotYet
            : known ? B.resumeFailed[known] : B.agentControlFailed)
      })
      .finally(() => setAgentWorking(false))
  }
  // 구글 로그인: 뒤쪽이 준 로그인 주소(loginUrl, 2026-11-18 G58b)를 몸소 브라우저 창으로 연다. 없으면(옛 뒤쪽) 지금 패널의 사이트, 그것도 없으면 구글 로그인 첫 화면.
  function openOutside() {
    // 새 뒤쪽(G75): 그 채팅방의 몸소 전용 창으로 원래 사이트를 연다(뒤쪽이 opened 이벤트를 보냄). 명령이 없는 뒤쪽이면 예전처럼 주소로 연다.
    const fallback = () => {
      const page = pageRef.current?.url && /^https:/i.test(pageRef.current.url) ? pageRef.current.url : null
      const site = agent?.loginUrl ?? page ?? 'https://accounts.google.com/'
      void openHumanBrowser(site).catch((error) => callbacks.current.onNotice(panelBrowserError(error) === 'browser_missing' ? B.browserMissing : B.humanFailed))
    }
    if (!agent) { fallback(); return }
    openExternalLogin(agent.chatId).catch((error) => {
      if (String(error).includes('not found')) fallback()
      else callbacks.current.onNotice(panelBrowserError(error) === 'browser_missing' ? B.browserMissing : B.humanFailed)
    })
  }
  const agentOther = !!agent && !!chatId && agent.chatId !== chatId
  const agentText = !agent ? null
    : agent.state === 'human' ? B.agentHuman
      : agent.state === 'external' ? B.agentExternalWorking
      : agent.state === 'login'
        ? (agent.reason === 'external_opened' ? B.agentExternalOpened
          : agent.reason === 'google_login_external' ? B.agentLoginOutside
          : agent.reason === 'one_time_code' ? B.agentCode
            : agent.reason === 'captcha' || agent.reason === 'verification' ? B.agentCheck
              : B.agentLogin)
        : agentOther ? (agent.title ? B.agentOther(agent.title) : B.agentBusy) : B.agentUsing

  function act(action: 'back' | 'forward' | 'reload' | 'stop') {
    void panelBrowserAction(chatRef.current, action).catch(() => {})
  }

  // 빈 브라우저(아직 열지 않음)에서도 이 방을 뒤쪽이 골랐으면 주소를 칠 수 있다. 치면 그 주소로 연다.
  const canType = opened || (selected && !failed)
  function submitAddress() {
    const next = editing === null ? null : typedAddress(editing)
    setEditing(null)
    if (!next) return
    if (!openedRef.current) {
      if (!selectedRef.current) { callbacks.current.onNotice(B.openFailed); return }
      setTyping(true)
      void openHere(next, want, () => false).finally(() => setTyping(false))
      return
    }
    void navigatePanelBrowser(chatRef.current, next).catch((error) => report(panelBrowserError(error)))
  }
  const [typing, setTyping] = useState(false)

  const host = page?.host ?? null
  // 저장된 로그인 칸의 사이트: 지금 페이지의 https 사이트(https://호스트[:포트], 뒤쪽이 받는 꼴). 페이지가 없거나 https 가 아니면 null(모든 로그인).
  const loginSite = (() => {
    try {
      const parsed = new URL(page?.url ?? '')
      return parsed.protocol === 'https:' ? parsed.origin : null
    } catch {
      return null
    }
  })()
  function closeLogins() {
    setLogins(false)
    requestAnimationFrame(() => keyButton.current?.focus())
  }
  // 새 로그인의 사이트 주소 칸에 채울 지금 페이지 주소(https 만).
  const loginPageUrl = page?.url && /^https:/i.test(page.url) ? page.url : null
  /**
   * 열고 채우기(S14 saved_login_open_and_fill): 이 방의 패널 자리(x·y·너비·높이)를 넘기면 뒤쪽이 그 사이트를 열고 그 로그인만 채운다(제출은 안 함).
   * 채웠으면 카드를 닫아 페이지를 보이고 알린다. 아직 연 적 없는 빈 브라우저였으면 뒤쪽이 연 페이지를 이 탭이 보이도록 연 것으로 둔다.
   */
  // ---- 확장 프로그램(퍼즐 단추·고정한 확장 단추·관리 화면). 떠 있는 창은 열쇠 창과 같은 길(동시에 하나: overlayWhat 으로 무엇이 떠 있는지). ----
  const [extensions, setExtensions] = useState<PanelExtension[]>([])
  const [pins, setPins] = useState<string[]>([])
  const [extMenu, setExtMenu] = useState<{ top: number; right: number; width: number } | null>(null)
  const puzzleButton = useRef<HTMLButtonElement>(null)
  const overlayWhat = useRef<string | null>(null)
  const reloadExtensions = useCallback(() => {
    listExtensions().then(setExtensions).catch(() => setExtensions([]))
    pinnedExtensions().then(setPins).catch(() => setPins([]))
  }, [])
  useEffect(() => { reloadExtensions() }, [reloadExtensions])
  useEffect(() => {
    let disposed = false
    const stops: Array<() => void> = []
    const keep = (stop: () => void) => { if (disposed) stop(); else stops.push(stop) }
    void listenExtensionsChanged(reloadExtensions).then(keep).catch(() => {})
    // 떠 있는 메뉴의 "확장 프로그램 관리"·"가져오기": 이 방이면 관리 화면.
    void listenExtensionsPage((owner, section) => { if (owner === chatRef.current) { setExtMenu(null); setManage(section) } }).then(keep).catch(() => {})
    return () => { disposed = true; stops.forEach((stop) => stop()) }
  }, [reloadExtensions])
  // 다른 탭·방으로 가면 관리 화면은 닫는다(페이지로 돌아옴).
  useEffect(() => { if (!active) setManage(null) }, [active])
  /** 퍼즐: 떠 있는 창(열쇠 창과 같은 길)으로 메뉴. 창이 없는 뒤쪽이면 화면 안 메뉴(웹뷰를 피해). */
  async function toggleExtensions() {
    if (overlayIdRef.current && overlayWhat.current === 'extensions') { void closePanelOverlay(overlayIdRef.current).catch(() => {}); return }
    if (Date.now() - overlayClosedAt.current < 400 && overlayWhat.current === null) return
    if (extMenu) { setExtMenu(null); return }
    const chat = chatRef.current
    const puzzle = puzzleButton.current?.getBoundingClientRect()
    if (!puzzle) return
    const width = 300
    const top = Math.round(puzzle.bottom + 6)
    const right = Math.max(8, Math.round(window.innerWidth - puzzle.right))
    if (!overlayMissing.current && chat) {
      const rows = Math.max(1, extensions.length)
      // 메뉴 + … 작은 메뉴가 들어갈 자리.
      const height = Math.round(Math.max(220, Math.min(480, window.innerHeight - top - 12, 120 + rows * 34 + 170)))
      const m = LOGIN_CARD_MARGIN
      try {
        const id = await openPanelOverlay(overlayKind('extensions', {}), chat, { x: Math.round(puzzle.right - width) - m.side, y: top - m.top, width: width + m.side * 2, height: height + m.top + m.bottom })
        overlayIdRef.current = id
        overlayWhat.current = 'extensions'
        setOverlayId(id)
        return
      } catch (error) {
        if (isMissingCommand(error)) overlayMissing.current = true
      }
    }
    setExtMenu({ top, right, width })
  }
  /** 고정한 확장 단추: 그 확장의 팝업을 떠 있는 창으로(panel_extension_popup_open). 같은 단추를 다시 누르면 닫는다. */
  async function togglePopup(item: PanelExtension, button: HTMLElement) {
    const what = `popup:${item.id}`
    if (overlayIdRef.current && overlayWhat.current === what) { void closePanelOverlay(overlayIdRef.current).catch(() => {}); return }
    if (Date.now() - overlayClosedAt.current < 400 && overlayWhat.current === null) return
    const chat = chatRef.current
    if (!chat) return
    const at = button.getBoundingClientRect()
    const width = Math.round(Math.min(380, window.innerWidth - 24))
    const top = Math.round(at.bottom + 6)
    const height = Math.round(Math.max(200, Math.min(560, window.innerHeight - top - 12)))
    const left = Math.round(Math.max(12, Math.min(at.right - width, window.innerWidth - width - 12)))
    try {
      const id = await openExtensionPopup(item.id, chat, { x: left, y: top, width, height })
      overlayIdRef.current = id
      overlayWhat.current = what
      setOverlayId(id)
    } catch (error) {
      callbacks.current.onNotice(extensionErrorCode(error) === 'no_popup' ? W.extensions.popupNone : W.extensions.popupFailed)
    }
  }
  useEffect(() => {
    if (!extMenu) return
    const onPress = (event: PointerEvent) => {
      const target = event.target as Element | null
      if (target?.closest('.ext-menu-card, .chat-menu') || puzzleButton.current?.contains(target as Node)) return
      setExtMenu(null)
    }
    document.addEventListener('pointerdown', onPress)
    return () => document.removeEventListener('pointerdown', onPress)
  }, [extMenu])
  const extMenuRef = useRef<HTMLElement>(null)
  useAvoidPanelBrowser(extMenuRef, extMenu !== null)
  const pinnedOn = pins.flatMap((id) => { const item = extensions.find((entry) => entry.id === id && entry.enabled); return item ? [item] : [] })

  // 떠 있는 창이 닫힘(closed·outside·chat_changed·panel_hidden·window_changed·replaced·failed): 같은 id 만. 명시적으로 닫았으면 열쇠로 초점.
  // 떠 있는 창의 열고 채우기가 채움: 이 방이면 패널을 보이게 두고 알린다. 이 부품이 사라지면 떠 있는 창도 닫는다.
  useEffect(() => {
    let disposed = false
    const stops: Array<() => void> = []
    const keep = (stop: () => void) => { if (disposed) stop(); else stops.push(stop) }
    void listenPanelOverlayClosed((id, reason) => {
      if (id !== overlayIdRef.current) return
      const what = overlayWhat.current
      overlayIdRef.current = null
      overlayWhat.current = null
      setOverlayId(null)
      overlayClosedAt.current = Date.now()
      if (reason === 'closed') requestAnimationFrame(() => (what === 'extensions' ? puzzleButton.current : what === null || what === 'logins' ? keyButton.current : null)?.focus())
    }).then(keep).catch(() => {})
    void listenOverlayFilled((owner) => {
      if (owner !== chatRef.current) return
      if (!openedRef.current) { openedRef.current = true; setOpened(true) }
      callbacks.current.onNotice(W.savedLogins.openFillDone)
    }).then(keep).catch(() => {})
    return () => {
      disposed = true
      stops.forEach((stop) => stop())
      if (overlayIdRef.current) void closePanelOverlay(overlayIdRef.current).catch(() => {})
    }
  }, [])
  // 이 탭이 안 보이게 되면(다른 탭·다른 방·패널 닫힘) 떠 있는 창도 닫는다(뒤쪽도 방 바뀜·패널 숨김에 닫는다).
  useEffect(() => {
    if (!active && overlayIdRef.current) void closePanelOverlay(overlayIdRef.current).catch(() => {})
  }, [active])
  /**
   * 열쇠: 떠 있는 창이 열려 있으면 닫고, 아니면 연다. 창 = 카드(열쇠 아래 6, 오른쪽 끝을 열쇠에 맞춤, 어사이드 크기 570x410 안) + 그림자 자리.
   * 카드가 그릴 것(지금 사이트·페이지 주소·열고 채우기의 패널 자리)은 kind 에 싣는다. 창이 없는 뒤쪽이거나 열지 못하면 예전 카드(웹뷰를 숨김).
   */
  async function toggleLogins() {
    if (overlayIdRef.current && (overlayWhat.current === 'logins' || overlayWhat.current === null)) { void closePanelOverlay(overlayIdRef.current).catch(() => {}); return }
    // 열쇠를 누르면 떠 있는 창이 먼저 초점을 잃어(outside) 닫힌다. 그 직후의 같은 누름으로 다시 열지 않는다.
    if (Date.now() - overlayClosedAt.current < 400) return
    if (logins) { setLogins(false); return }
    const chat = chatRef.current
    const key = keyButton.current?.getBoundingClientRect()
    if (!overlayMissing.current && chat && key) {
      const top = Math.round(key.bottom + 6)
      const width = Math.round(Math.min(570, key.right - 12))
      const height = Math.round(Math.max(240, Math.min(410, window.innerHeight - top - 12)))
      const left = Math.round(key.right - width)
      const m = LOGIN_CARD_MARGIN
      const kind = overlayKind('saved-logins', { site: loginSite, pageUrl: loginPageUrl, spot: measure() })
      try {
        const id = await openPanelOverlay(kind, chat, { x: left - m.side, y: top - m.top, width: width + m.side * 2, height: height + m.top + m.bottom })
        overlayIdRef.current = id
        overlayWhat.current = 'logins'
        setOverlayId(id)
        return
      } catch (error) {
        if (isMissingCommand(error)) overlayMissing.current = true
      }
    }
    setLogins(true)
  }
  async function openAndFill(login: SavedLogin) {
    const chat = chatRef.current
    const bounds = measure()
    if (!chat || !bounds) return 'panel_closed'
    const status = await openAndFillSavedLogin(chat, login.id, bounds)
    if (status === 'filled') {
      if (!openedRef.current) { openedRef.current = true; setOpened(true) }
      setLogins(false)
      callbacks.current.onNotice(W.savedLogins.openFillDone)
    }
    return status
  }
  useLayoutEffect(() => {
    if (!logins) { setCardAt(null); return }
    // 어사이드 크기(570x410)를 넘지 않게, 창 안에 들게(왼쪽·아래 12 띄움).
    const place = () => {
      const key = keyButton.current?.getBoundingClientRect()
      if (!key) return
      const top = Math.round(key.bottom + 6)
      setCardAt({
        top,
        right: Math.max(8, Math.round(window.innerWidth - key.right)),
        width: Math.round(Math.min(570, key.right - 12)),
        height: Math.round(Math.max(240, Math.min(410, window.innerHeight - top - 12))),
      })
    }
    place()
    const observer = new ResizeObserver(place)
    if (browserBox.current) observer.observe(browserBox.current)
    window.addEventListener('resize', place)
    return () => { observer.disconnect(); window.removeEventListener('resize', place) }
  }, [logins])
  useEffect(() => {
    if (!logins) return
    pressInside.current = false
    requestAnimationFrame(() => loginCard.current?.focus())
    const onPress = () => {
      if (pressInside.current) { pressInside.current = false; return }
      setLogins(false)
    }
    document.addEventListener('pointerdown', onPress)
    return () => document.removeEventListener('pointerdown', onPress)
  }, [logins])
  // 빈 브라우저(url null)는 페이지가 오기 전까지 불러오는 중이 아니다. 여는 중(주소·AI 요청·친 주소)이면 불러오는 중.
  const loading = opened ? (page ? page.loading : url !== null) : selected && !failed && (url !== null || agentOpen || typing)
  return (
    <div ref={browserBox} className="panel-browser">
      <div className="zet-viewbar panel-browser-bar">
        {/* 관리 화면에서 뒤로 = 페이지로 돌아감. */}
        <button className="icon-button" type="button" aria-label={W.tip.back} title={W.tip.back} disabled={manage ? false : !opened || history.canGoBack === false} onClick={() => (manage ? setManage(null) : act('back'))}>
          <ArrowLeft size={16} aria-hidden />
        </button>
        <button className="icon-button" type="button" aria-label={W.tip.forward} title={W.tip.forward} disabled={!opened || history.canGoForward === false} onClick={() => act('forward')}>
          <ArrowRight size={16} aria-hidden />
        </button>
        {loading && opened ? (
          <button className="icon-button" type="button" aria-label={B.stop} title={B.stop} onClick={() => act('stop')}>
            <X size={16} aria-hidden />
          </button>
        ) : (
          <button className="icon-button" type="button" aria-label={B.reload} title={B.reload} disabled={!opened} onClick={() => act('reload')}>
            <RotateCw size={15} aria-hidden />
          </button>
        )}
        {manage ? (
          <span className="zet-search panel-browser-address" aria-label={`${B.address}: ${W.extensions.pageTitle}`}>
            <Puzzle size={13} aria-hidden />
            <span className="panel-browser-host">{W.extensions.pageTitle}</span>
          </span>
        ) : editing !== null ? (
          <form className="zet-search panel-browser-address" onSubmit={(event) => { event.preventDefault(); submitAddress() }}>
            {page?.secure ? <Lock size={13} aria-hidden /> : <LockOpen size={13} aria-hidden />}
            <input
              autoFocus
              value={editing}
              aria-label={B.address}
              spellCheck={false}
              onChange={(event) => setEditing(event.target.value)}
              onFocus={(event) => event.currentTarget.select()}
              onBlur={() => setEditing(null)}
              onKeyDown={(event) => { if (event.key === 'Escape') { event.preventDefault(); setEditing(null) } }}
            />
          </form>
        ) : (
          <button
            className="zet-search panel-browser-address"
            type="button"
            title={page ? (page.secure ? page.url : `${B.insecure} · ${page.url}`) : undefined}
            aria-label={host ? `${B.address}: ${host}` : B.address}
            disabled={!canType}
            onClick={() => setEditing(page?.url ?? '')}
          >
            {loading ? <LoaderCircle size={13} className="is-spinning" aria-hidden /> : page?.secure ? <Lock size={13} aria-hidden /> : <LockOpen size={13} aria-hidden />}
            {page && !page.secure && <span className="panel-browser-insecure">{B.insecure}</span>}
            {/* 넘치면 앞(하위 도메인)부터 잘라 끝(등록 도메인)이 늘 보이게. */}
            <span className="panel-browser-host"><bdi>{host ?? ''}</bdi></span>
          </button>
        )}
        {/* 고정한 확장(켜진 것만, 고정한 차례): 누르면 그 확장의 팝업. */}
        {pinnedOn.map((item) => (
          <button
            key={item.id}
            className="icon-button ext-bar-button"
            type="button"
            aria-label={item.name}
            title={item.name}
            aria-haspopup="dialog"
            aria-expanded={overlayId !== null && overlayWhat.current === `popup:${item.id}`}
            onClick={(event) => void togglePopup(item, event.currentTarget)}
          >
            <ExtensionIcon src={item.iconDataUrl} />
          </button>
        ))}
        <button
          ref={keyButton}
          className="icon-button"
          type="button"
          aria-label={W.savedLogins.title}
          title={W.savedLogins.title}
          aria-haspopup="dialog"
          aria-expanded={logins || (overlayId !== null && (overlayWhat.current === 'logins' || overlayWhat.current === null))}
          onPointerDown={() => { pressInside.current = true }}
          onClick={() => void toggleLogins()}
        >
          <KeyRound size={15} aria-hidden />
        </button>
        <button
          ref={puzzleButton}
          className="icon-button"
          type="button"
          aria-label={W.extensions.button}
          title={W.extensions.button}
          aria-haspopup="menu"
          aria-expanded={extMenu !== null || (overlayId !== null && overlayWhat.current === 'extensions')}
          onClick={() => void toggleExtensions()}
        >
          <Puzzle size={15} aria-hidden />
        </button>
        <button className="icon-button" type="button" aria-label={B.openExternal} title={B.openExternal} disabled={!page?.url} onClick={() => { if (page?.url) void openWebAddress(page.url).catch(() => onNotice(W.zetPanel.text18)) }}>
          <ExternalLink size={15} aria-hidden />
        </button>
      </div>
      {/* 저장된 로그인 카드(닫기 X 없음): 바깥을 누르거나 Esc 면 닫히고 열쇠 단추로 돌아간다. 안쪽은 SavedLoginsCard(왼쪽 목록, 오른쪽 보기·입력). */}
      {logins && createPortal(
        <section
          ref={loginCard}
          className="panel-login-card"
          role="dialog"
          aria-label={W.savedLogins.title}
          tabIndex={-1}
          hidden={!cardAt}
          style={cardAt ? { top: cardAt.top, right: cardAt.right, width: cardAt.width, height: cardAt.height } : undefined}
          onPointerDown={() => { pressInside.current = true }}
          onKeyDown={(event) => { if (event.key === 'Escape') { event.preventDefault(); closeLogins() } }}
        >
          <SavedLoginsCard site={loginSite} pageUrl={loginPageUrl} onOpenAndFill={chatId ? openAndFill : undefined} />
        </section>,
        document.body,
      )}
      {/* 퍼즐 메뉴(떠 있는 창이 없는 뒤쪽일 때만 화면 안에). */}
      {extMenu && createPortal(
        <section
          ref={extMenuRef}
          className="ext-menu-card is-inline"
          role="dialog"
          aria-label={W.extensions.button}
          style={{ top: extMenu.top, right: extMenu.right, width: extMenu.width, maxHeight: Math.max(220, window.innerHeight - extMenu.top - 12) }}
          onKeyDown={(event) => { if (event.key === 'Escape' && !(event.target as HTMLElement).closest('.chat-menu')) { event.preventDefault(); setExtMenu(null); puzzleButton.current?.focus() } }}
        >
          <ExtensionsMenu onManage={(section) => { setExtMenu(null); setManage(section) }} onPinsChanged={reloadExtensions} />
        </section>,
        document.body,
      )}
      {/* 팝업이 열려 있는 동안 한 줄: 어느 사이트에서 연 어떤 창인지(뒤쪽 값만, 창 제목은 풍선). */}
      {popup && popup.host && (
        <div className="zet-viewbar panel-agent-line" role="status" title={popup.title ?? undefined}>
          <span>{popup.openerHost ? B.popupFrom(popup.openerHost, popup.host) : B.popupOpen(popup.host)}</span>
        </div>
      )}
      {/* AI 가 이 패널을 쓰는 동안 한 줄(맨 위 줄과 같은 꼴). 직접 쓰기 = AI 를 멈추고 내가 쓴다. */}
      {agent && (
        <div className="zet-viewbar panel-agent-line" role={agent.state === 'login' ? 'alert' : 'status'}>
          <span>{agentText}</span>
          {agent.state === 'login' && agent.reason === 'google_login_external' && (
            <button className="secondary-button" type="button" onClick={openOutside}>{B.agentOpenOutside}</button>
          )}
          {/* AI 가 바깥 창에서 이어 하는 동안은 패널에서 할 일이 없어 단추를 두지 않는다. */}
          {agent.state !== 'external' && (
            <button className="secondary-button" type="button" disabled={agentWorking} onClick={() => control(agent.state === 'ai' ? 'take' : 'resume')}>
              {agent.state === 'ai' ? B.agentTakeOver : B.agentResume}
            </button>
          )}
        </div>
      )}
      <div ref={spot} className="panel-browser-spot">
        {manage && <ExtensionsManager section={manage} onPinsChanged={reloadExtensions} />}
        {failed && !manage && (
          <div className="zet-message" role="alert">
            <span>{B.openFailed}</span>
            <button className="secondary-button" type="button" onClick={() => { setFailed(false); openedRef.current = false; setOpened(false); callbacks.current.onTitle(null, null); void retryOpen() }}>
              <RefreshCw size={13} aria-hidden />{W.zetPanel.text25}</button>
          </div>
        )}
      </div>
    </div>
  )

  async function retryOpen() {
    await openHere(url, want, () => false)
  }
}
