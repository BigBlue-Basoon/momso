import { useEffect, useRef, useState, useLayoutEffect } from 'react'
import { ArrowLeft, ArrowRight, ExternalLink, LoaderCircle, Lock, LockOpen, RefreshCw, RotateCw, X } from 'lucide-react'
import { openWebAddress } from '../archive'
import {
  closePanelBrowser,
  listenPanelAgent,
  listenPanelBrowser,
  openPanelAgent,
  panelAgentStatus,
  resumePanelAgent,
  takeOverPanel,
  navigatePanelBrowser,
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
  usePanelBrowserHeld,
  type PanelBounds,
  type PanelBrowserError,
  type PanelPage,
} from '../panel-browser'
import { W } from '../words'
import type { SourceNotice } from './SourceDelete'

const B = W.panelBrowser

/**
 * 오른쪽 패널의 브라우저 탭 안(2026-11-17 본부 결정, ui-panel-browser-plan-20261117.md).
 * 맨 위 한 줄은 문서 탭의 zet-viewbar 줄 꼴 그대로: 뒤로·앞으로·새로 고침(불러오는 중에는 멈추기), 자물쇠와 호스트(누르면 전체 주소를 고치는 칸), 기본 브라우저로 열기.
 * 그 아래 빈 자리를 재서 뒤쪽 웹뷰를 그 위에 띄운다. 호스트·자물쇠는 뒤쪽 이벤트의 값만 쓴다(사이트 화면 안의 글은 쓰지 않는다).
 */
export function PanelBrowserView({
  url,
  seq,
  chatId = null,
  active,
  onTitle,
  onOpenFailed,
  onNotice,
}: {
  /** 열어 달라는 주소. seq 가 바뀌면 같은 주소라도 다시 연다. null 이면 AI 가 열어 달라고 한 빈 패널. */
  url: string | null
  seq: number
  /** 지금 보는 채팅방(AI 가 쓰는 중인지 물을 때). */
  chatId?: string | null
  /** 이 탭이 지금 보이는가(패널이 열려 있고 이 탭이 고른 탭). */
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
  const held = usePanelBrowserHeld()
  const lastBounds = useRef<string>('')
  const openedRef = useRef(false)
  const pageRef = useRef<PanelPage | null>(null)
  pageRef.current = page
  const titleRef = useRef<string | null>(null)
  const callbacks = useRef({ onTitle, onOpenFailed, onNotice })
  callbacks.current = { onTitle, onOpenFailed, onNotice }

  function measure(): PanelBounds | null {
    const rect = spot.current?.getBoundingClientRect()
    if (!rect || rect.width < 1 || rect.height < 1) return null
    return { x: Math.round(rect.left), y: Math.round(rect.top), width: Math.round(rect.width), height: Math.round(rect.height) }
  }

  // 뒤쪽 이벤트(웹뷰는 앱에 하나라 이 탭도 하나).
  useEffect(() => {
    let stop: (() => void) | undefined
    let disposed = false
    void listenPanelBrowser({
      onPage: (next) => {
        setPage(next)
        if (!titleRef.current) callbacks.current.onTitle(null, next.host)
      },
      onTitle: (title) => { titleRef.current = title.trim() || null; callbacks.current.onTitle(titleRef.current, pageRef.current?.host ?? null) },
      onHistory: setHistory,
      // 다운로드(2026-11-18 새 꼴): 받는 중 · 다 받음(채팅방에 첨부할 수 있음) · 막음(실행 파일) · 실패. 알림 줄 한 줄씩.
      onDownload: (download) => {
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
      onGoogleLogin: (siteUrl) => {
        callbacks.current.onNotice(siteUrl ? B.google : B.googleNoSite, siteUrl ? {
          label: B.openHuman,
          run: () => { void openHumanBrowser(siteUrl).catch((error) => callbacks.current.onNotice(panelBrowserError(error) === 'browser_missing' ? B.browserMissing : B.humanFailed)) },
        } : undefined)
      },
    }).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [])

  // 처음 열기와 다시 열기(주소·seq 가 바뀌면).
  useEffect(() => {
    let cancelled = false
    async function go() {
      if (openedRef.current) {
        if (url === null) return
        try { await navigatePanelBrowser(url) } catch (error) { report(panelBrowserError(error)) }
        return
      }
      // 자리가 그려지고 멈출 때까지 기다린다. 패널이 열리는 중이면 폭이 0 이거나 오른쪽에서 밀려 들어오는 중이다.
      // 움직이는 중에 재면 웹뷰가 엉뚱한 자리(오른쪽)에 놓였다(2026-11-18 실제 앱 B72: 웹뷰 x 1181, 자리 x 780.6). 50ms 간격 두 번이 같을 때까지(최대 2초).
      let bounds = measure()
      for (let i = 0; i < 40 && !cancelled; i += 1) {
        await new Promise((r) => setTimeout(r, 50))
        const next = measure()
        const still = Boolean(bounds && next && JSON.stringify(bounds) === JSON.stringify(next))
        bounds = next
        if (still) break
      }
      if (cancelled || !bounds) return
      try {
        // AI 의 열기 요청이면 빈 패널을 연다(주소는 AI 가 승인 뒤 연다).
        if (url === null) await openPanelAgent(bounds)
        else await openPanelBrowser(url, bounds)
      } catch (error) {
        const code = panelBrowserError(error)
        if (code !== 'already_open') {
          if (code === 'unavailable') { setFailed(true); return }
          callbacks.current.onOpenFailed(code)
          return
        }
        // 앞서 연 웹뷰가 남아 있으면 그 웹뷰를 쓴다.
        if (url !== null) { try { await navigatePanelBrowser(url) } catch (navError) { report(panelBrowserError(navError)) } }
        await setPanelBrowserBounds(bounds).catch(() => {})
      }
      if (cancelled) return
      openedRef.current = true
      lastBounds.current = JSON.stringify(bounds)
      setFailed(false)
      setOpened(true)
    }
    void go()
    return () => { cancelled = true }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [url, seq])

  // 탭을 닫으면 웹뷰도 닫는다.
  useEffect(() => () => {
    if (openedRef.current) void closePanelBrowser().catch(() => {})
    openedRef.current = false
    setPanelBrowserShown(false)
  }, [])

  // 보임: 이 탭이 보이고, 몸소 것이 덮고 있지 않을 때만.
  const visible = opened && active && !held
  useEffect(() => {
    if (!opened) return
    void setPanelBrowserVisible(visible).catch(() => {})
  }, [opened, visible])
  useEffect(() => { setPanelBrowserShown(opened && active) }, [opened, active])

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
      void setPanelBrowserBounds(bounds)
        .then(() => { lastBounds.current = key; sending.current = false; follow.current() })
        .catch(() => { sending.current = false })
    }
  }
  useEffect(() => {
    if (!visible) return
    let frame = 0
    const tick = () => {
      follow.current()
      frame = window.requestAnimationFrame(tick)
    }
    frame = window.requestAnimationFrame(tick)
    // 창이 숨겨지거나 가려져 화면 그림이 멈춰도 따라가도록 시계로도 잰다(그때 브라우저는 1초쯤으로 늦춘다).
    const timer = window.setInterval(() => follow.current(), 250)
    return () => { window.cancelAnimationFrame(frame); window.clearInterval(timer) }
  }, [visible])
  // 패널 위 줄(AI 가 쓰는 중·로그인 안내·팝업 출처)이 생기거나 사라지면 그 그림 전에 곧바로 웹뷰를 내리거나 올린다(2026-11-18 실제 앱 B71).
  // 화면 그림마다 도는 것만으로는, 창이 가려졌거나 숨겨져 그림이 멈추면 웹뷰가 옛 자리에 남아 줄을 덮었다.
  useLayoutEffect(() => { follow.current() })

  function report(code: PanelBrowserError) {
    // Google 로그인은 뒤쪽 이벤트가 따로 알린다(두 번 알리지 않는다).
    if (code === 'google_login_required') return
    callbacks.current.onNotice(code === 'invalid_url' ? W.zetPanel.text18 : B.openFailed)
  }

  // 팝업(로그인 창 등): 열려 있는 동안 위쪽에 한 줄로 어느 사이트에서 연 창인지(호스트는 뒤쪽 값만). 닫히면 거둔다.
  const [popup, setPopup] = useState<PanelPopup | null>(null)
  useEffect(() => {
    let stop: (() => void) | undefined
    let disposed = false
    void listenPanelPopup((next) => {
      if (next.state === 'opened') setPopup(next)
      else if (next.state === 'closed') setPopup((current) => (!current || !next.host || current.host === next.host ? null : current))
    }).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [])

  // AI 가 이 패널을 쓰는지(panel-agent-control-changed). started → AI 가 쓰는 중(직접 쓰기 단추), human → 내가 쓰는 중(AI에게 돌려주기 단추),
  // ended·available → 줄 없음. 처음에는 panel_agent_status 로 내가 가져가 있는지만 본다(AI 가 지금 움직이는지는 이벤트로만 안다).
  // login(2026-11-18 G57b): 로그인·인증을 사람이 해야 함. 끝나면 [AI에게 돌려주기]. 구글 로그인은 바깥 창(몸소 브라우저 창)에서.
  // 채팅방 이름(title)은 지금 보는 채팅방이 아닐 때 "○○ 채팅방의 AI가…"로만 쓴다. 회사 이름은 줄을 짧게 두려고 적지 않는다.
  type AgentLine = { chatId: string; state: 'ai' | 'human' | 'login'; reason: string | null; title: string | null; loginUrl?: string | null }
  const [agent, setAgent] = useState<AgentLine | null>(null)
  const [agentWorking, setAgentWorking] = useState(false)
  useEffect(() => {
    let stop: (() => void) | undefined
    let disposed = false
    void listenPanelAgent({
      onControl: ({ chatId: owner, state, reason, chatTitle, loginUrl }) => {
        if (state === 'started') setAgent({ chatId: owner, state: 'ai', reason: null, title: chatTitle })
        else if (state === 'human') setAgent({ chatId: owner, state: 'human', reason: null, title: chatTitle })
        else if (state === 'login') setAgent({ chatId: owner, state: 'login', reason, title: chatTitle, loginUrl })
        else if (state === 'ended' || state === 'available') setAgent(null)
      },
      // 몸소 브라우저의 "사람이 해야 함"이 패널 AI 의 채팅방에서 오면 같은 로그인 줄로(까닭 이름은 panel-agent-control-changed 의 것으로 맞춤).
      onHumanRequired: (owner, reason, loginUrl) => {
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
        callbacks.current.onNotice(code.includes('panel_busy') ? B.agentBusy : code.includes('human_required') ? B.agentLoginNotYet : B.agentControlFailed)
      })
      .finally(() => setAgentWorking(false))
  }
  // 구글 로그인: 뒤쪽이 준 로그인 주소(loginUrl, 2026-11-18 G58b)를 몸소 브라우저 창으로 연다. 없으면(옛 뒤쪽) 지금 패널의 사이트, 그것도 없으면 구글 로그인 첫 화면.
  function openOutside() {
    const page = pageRef.current?.url && /^https:/i.test(pageRef.current.url) ? pageRef.current.url : null
    const site = agent?.loginUrl ?? page ?? 'https://accounts.google.com/'
    void openHumanBrowser(site).catch((error) => callbacks.current.onNotice(panelBrowserError(error) === 'browser_missing' ? B.browserMissing : B.humanFailed))
  }
  const agentOther = !!agent && !!chatId && agent.chatId !== chatId
  const agentText = !agent ? null
    : agent.state === 'human' ? B.agentHuman
      : agent.state === 'login'
        ? (agent.reason === 'google_login_external' ? B.agentLoginOutside
          : agent.reason === 'one_time_code' ? B.agentCode
            : agent.reason === 'captcha' || agent.reason === 'verification' ? B.agentCheck
              : B.agentLogin)
        : agentOther ? (agent.title ? B.agentOther(agent.title) : B.agentBusy) : B.agentUsing

  function act(action: 'back' | 'forward' | 'reload' | 'stop') {
    void panelBrowserAction(action).catch(() => {})
  }

  function submitAddress() {
    const next = editing === null ? null : typedAddress(editing)
    setEditing(null)
    if (!next) return
    if (!openedRef.current) { callbacks.current.onNotice(B.openFailed); return }
    void navigatePanelBrowser(next).catch((error) => report(panelBrowserError(error)))
  }

  const host = page?.host ?? null
  // AI 가 연 빈 패널(url null)은 페이지가 오기 전까지 불러오는 중이 아니다.
  const loading = !opened || (page ? page.loading : url !== null)
  return (
    <div className="panel-browser">
      <div className="zet-viewbar panel-browser-bar">
        <button className="icon-button" type="button" aria-label={W.tip.back} title={W.tip.back} disabled={!opened || history.canGoBack === false} onClick={() => act('back')}>
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
        {editing !== null ? (
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
            disabled={!opened}
            onClick={() => setEditing(page?.url ?? '')}
          >
            {loading ? <LoaderCircle size={13} className="is-spinning" aria-hidden /> : page?.secure ? <Lock size={13} aria-hidden /> : <LockOpen size={13} aria-hidden />}
            {page && !page.secure && <span className="panel-browser-insecure">{B.insecure}</span>}
            {/* 넘치면 앞(하위 도메인)부터 잘라 끝(등록 도메인)이 늘 보이게. */}
            <span className="panel-browser-host"><bdi>{host ?? ''}</bdi></span>
          </button>
        )}
        <button className="icon-button" type="button" aria-label={B.openExternal} title={B.openExternal} disabled={!page?.url} onClick={() => { if (page?.url) void openWebAddress(page.url).catch(() => onNotice(W.zetPanel.text18)) }}>
          <ExternalLink size={15} aria-hidden />
        </button>
      </div>
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
          <button className="secondary-button" type="button" disabled={agentWorking} onClick={() => control(agent.state === 'ai' ? 'take' : 'resume')}>
            {agent.state === 'ai' ? B.agentTakeOver : B.agentResume}
          </button>
        </div>
      )}
      <div ref={spot} className="panel-browser-spot">
        {failed && (
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
    const bounds = measure()
    if (!bounds) return
    try {
      if (url === null) await openPanelAgent(bounds)
      else await openPanelBrowser(url, bounds)
      openedRef.current = true
      lastBounds.current = JSON.stringify(bounds)
      setOpened(true)
    } catch (error) {
      const code = panelBrowserError(error)
      if (code === 'unavailable') setFailed(true)
      else callbacks.current.onOpenFailed(code)
    }
  }
}
