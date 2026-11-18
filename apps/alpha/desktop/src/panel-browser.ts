// 오른쪽 패널 브라우저(2026-11-17, 뒤쪽 panel_browser.rs · desktop-panel-browser-contract.md). 사람이 쓰는 패널이다(AI 는 다루지 않는다).
//   명령: panel_browser_open({ url, x, y, width, height }) → 이미 열렸으면 already_open, Google 계정 주소면 google_login_required
//         panel_browser_close(), panel_browser_visibility({ visible }), panel_browser_bounds({ x, y, width, height }) (창 안의 논리 좌표)
//         panel_browser_navigate({ url }), panel_browser_action({ action: back|forward|reload|stop })
//         panel_browser_open_human({ url }): 사람이 쓰는 전용 창(Edge, 없으면 Chrome, 둘 다 없으면 browser_missing)
//         그 밖의 오류: invalid_url, invalid_bounds, invalid_action, unavailable
//   이벤트: panel-browser-page { url, host, secure, loading }, panel-browser-title { title }, panel-browser-history { canGoBack, canGoForward },
//           panel-browser-download { state: started|finished|blocked|failed, name, path, size } (2026-11-18 새 꼴. 다운로드 폴더에 새 파일로 저장, 실행 파일은 막음),
//           panel-browser-google-login { host, siteUrl } (패널 이동을 막고 보냄, siteUrl 은 null 일 수 있음)
// 패널 웹뷰는 몸소 화면 위의 따로 뜨는 층이다. 몸소의 묻는 창·찾기 창 같은 것이 그 자리를 덮는 동안은 웹뷰를 숨긴다(holdPanelBrowser).
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { type RefObject, useEffect, useLayoutEffect, useRef, useSyncExternalStore } from 'react'
import { isDesktopRuntime } from './runtime'

export type PanelBounds = { x: number; y: number; width: number; height: number }
export type PanelPage = { url: string; host: string | null; secure: boolean; loading: boolean }

export const PANEL_BROWSER_ERRORS = ['already_open', 'google_login_required', 'invalid_url', 'invalid_bounds', 'invalid_action', 'browser_missing', 'unavailable'] as const
export type PanelBrowserError = (typeof PANEL_BROWSER_ERRORS)[number]

/** 뒤쪽 오류 글에서 정해 둔 값만 꺼낸다. 모르는 것은 unavailable. */
export function panelBrowserError(error: unknown): PanelBrowserError {
  const text = error instanceof Error ? error.message : String(error)
  return PANEL_BROWSER_ERRORS.find((code) => text.includes(code)) ?? 'unavailable'
}

// 여는 요청은 한 줄로 세운다. 둘이 겹치면 뒤쪽이 같은 이름의 웹뷰를 두 번 만들려다 뒤의 것이 unavailable 로 실패한다(2026-11-17 실제 앱).
// 줄을 세우면 뒤의 것은 already_open 을 받아 앞의 웹뷰를 쓴다.
let openQueue: Promise<unknown> = Promise.resolve()
export function openPanelBrowser(url: string, bounds: PanelBounds) {
  const run = openQueue.then(() => invoke('panel_browser_open', { url, ...bounds })).then((value) => { placed = bounds; return value })
  openQueue = run.catch(() => {})
  return run
}
export async function closePanelBrowser() {
  await invoke('panel_browser_close')
  placed = null
}
export async function setPanelBrowserVisible(visible: boolean) {
  await invoke('panel_browser_visibility', { visible })
}
export async function setPanelBrowserBounds(bounds: PanelBounds) {
  await invoke('panel_browser_bounds', bounds)
  placed = bounds
}
export async function navigatePanelBrowser(url: string) {
  await invoke('panel_browser_navigate', { url })
}
export async function panelBrowserAction(action: 'back' | 'forward' | 'reload' | 'stop') {
  await invoke('panel_browser_action', { action })
}
export async function openHumanBrowser(url: string) {
  await invoke('panel_browser_open_human', { url })
}
// ---- AI 가 패널을 쓸 때(2026-11-18 계약 panel-agent-integration-g52.md, 뒤쪽 feat/desktop-panel-agent) ----
//   이벤트 panel-agent-open-request { chatId }: 패널이 없으면 뒤쪽이 보내고 20초 기다린다. 화면은 브라우저 탭을 열고 실제 자리로
//     panel_browser_open_agent({ x, y, width, height }) 를 불러 빈 패널을 연다(주소는 AI 가 승인 뒤 연다).
//   panel_agent_status({ chatId }) → { ownerChatId, humanControls }
//   panel_agent_take_over({ chatId }) / panel_agent_resume({ chatId }) → 이벤트 panel-agent-control-changed { chatId, state: human | available }.
//     AI 조작 시작·끝도 같은 이벤트의 started / ended. 오류: panel_busy(다른 채팅방이 씀), human_takeover, panel_closed, panel_page_changed.
//   계약에 없는 것: 로그인 화면이라 사람에게 넘겼다는 이벤트·값(화면은 아직 그 안내를 그리지 않는다).
export async function openPanelAgent(bounds: PanelBounds) {
  await invoke('panel_browser_open_agent', bounds)
  placed = bounds
}
export async function panelAgentStatus(chatId: string) {
  const value = await invoke<{ ownerChatId?: unknown; humanControls?: unknown } | null>('panel_agent_status', { chatId })
  return { ownerChatId: typeof value?.ownerChatId === 'string' ? value.ownerChatId : null, humanControls: value?.humanControls === true }
}
export async function takeOverPanel(chatId: string) {
  await invoke('panel_agent_take_over', { chatId })
}
export async function resumePanelAgent(chatId: string) {
  await invoke('panel_agent_resume', { chatId })
}
/**
 * 2026-11-18 뒤쪽 cdd3ebe(G57b): panel-agent-control-changed { chatId, state, reason, provider, chatTitle }.
 *   state: started | ended | human | available | login. login 의 reason: login_page | one_time_code | captcha | verification |
 *   google_login_external(구글 로그인은 패널에서 못 해 바깥 창에서). 로그인이 끝나면 panel_agent_resume → 뒤쪽이 확인하면 started,
 *   아직이면 human_required 오류. ended 의 reason: panel_not_opened(열어 달라고 했는데 AI 의 턴이 끝남) 또는 null.
 *   provider: codex | claude | gemini, chatTitle: 그 채팅방 이름(없으면 null).
 */
export type PanelAgentControl = {
  chatId: string
  state: 'started' | 'ended' | 'human' | 'available' | 'login' | string
  reason: string | null
  provider: string | null
  chatTitle: string | null
  /** google_login_external 일 때(2026-11-18 뒤쪽 2ae5d7e G58b): 열 로그인 주소(accounts.google.com)와 지금 패널의 사이트. */
  loginUrl?: string | null
  siteUrl?: string | null
}
// browser-human-required { chatId, reason }(login | password | one_time_code | captcha | verification | external_login): 몸소 브라우저가 사람에게 넘길 때.
// 패널 AI 의 채팅방이면 패널 위 로그인 줄로도 보인다(뒤쪽은 보통 panel-agent-control-changed login 도 함께 보내지만, 그것이 없을 때를 위해).
export async function listenPanelAgent(handlers: { onOpenRequest?: (chatId: string | null) => void; onControl?: (control: PanelAgentControl) => void; onHumanRequired?: (chatId: string, reason: string, loginUrl: string | null) => void }) {
  if (!isDesktopRuntime()) return () => {}
  const stops = await Promise.all([
    listen<{ chatId?: unknown }>('panel-agent-open-request', (event) => handlers.onOpenRequest?.(typeof event.payload?.chatId === 'string' ? event.payload.chatId : null)),
    listen<{ chatId?: unknown; state?: unknown; reason?: unknown; provider?: unknown; chatTitle?: unknown; loginUrl?: unknown; siteUrl?: unknown }>('panel-agent-control-changed', (event) => {
      const p = event.payload ?? {}
      const text = (value: unknown) => (typeof value === 'string' && value.trim() ? value : null)
      if (typeof p.chatId === 'string' && typeof p.state === 'string') {
        handlers.onControl?.({ chatId: p.chatId, state: p.state, reason: text(p.reason), provider: text(p.provider), chatTitle: text(p.chatTitle), loginUrl: text(p.loginUrl), siteUrl: text(p.siteUrl) })
      }
    }),
    listen<{ chatId?: unknown; reason?: unknown; loginUrl?: unknown }>('browser-human-required', (event) => {
      const p = event.payload ?? {}
      if (typeof p.chatId === 'string' && typeof p.reason === 'string') handlers.onHumanRequired?.(p.chatId, p.reason, typeof p.loginUrl === 'string' && p.loginUrl ? p.loginUrl : null)
    }),
  ])
  return () => stops.forEach((stop) => stop())
}

/** 이번 실행에서 다 받은 파일을 첨부할 선택으로 기록한다(그 뒤 첨부가 그 경로를 받아들인다). */
export async function selectDownloadForAttach(path: string) {
  await invoke('panel_browser_download_select', { path })
}
/** 이번 실행에서 다 받은 파일을 탐색기에서 골라 보인다(panel_download_reveal, 그 밖의 경로는 selection_required). */
export async function revealDownload(path: string) {
  await invoke('panel_download_reveal', { path })
}

/**
 * 팝업(2026-11-18 뒤쪽 144bbd7): panel-browser-popup { state: opened | closed, host, openerHost, title, reason }.
 * 닫힘의 reason: user | site | panel_closed | unknown. 팝업마다 이름이 없어 여럿이면 마지막 것만 보인다.
 */
export type PanelPopup = { state: string; host: string | null; openerHost: string | null; title: string | null; reason: string | null }
export async function listenPanelPopup(onPopup: (popup: PanelPopup) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<Partial<Record<keyof PanelPopup, unknown>>>('panel-browser-popup', (event) => {
    const p = event.payload ?? {}
    const text = (v: unknown) => (typeof v === 'string' && v ? v : null)
    if (typeof p.state === 'string') onPopup({ state: p.state, host: text(p.host), openerHost: text(p.openerHost), title: text(p.title), reason: text(p.reason) })
  })
}

export type PanelDownload = { state: 'started' | 'finished' | 'blocked' | 'failed' | string; name: string | null; path: string | null; size: number | null }

export async function listenPanelBrowser(handlers: {
  onPage?: (page: PanelPage) => void
  onTitle?: (title: string) => void
  onHistory?: (history: { canGoBack: boolean | null; canGoForward: boolean | null }) => void
  onDownload?: (download: PanelDownload) => void
  onGoogleLogin?: (siteUrl: string | null) => void
}) {
  if (!isDesktopRuntime()) return () => {}
  const stops = await Promise.all([
    listen<Partial<PanelPage>>('panel-browser-page', (event) => {
      const p = event.payload ?? {}
      if (typeof p.url === 'string') handlers.onPage?.({ url: p.url, host: typeof p.host === 'string' && p.host ? p.host : null, secure: p.secure === true, loading: p.loading === true })
    }),
    listen<{ title?: unknown }>('panel-browser-title', (event) => { if (typeof event.payload?.title === 'string') handlers.onTitle?.(event.payload.title) }),
    listen<{ canGoBack?: unknown; canGoForward?: unknown }>('panel-browser-history', (event) => handlers.onHistory?.({
      canGoBack: typeof event.payload?.canGoBack === 'boolean' ? event.payload.canGoBack : null,
      canGoForward: typeof event.payload?.canGoForward === 'boolean' ? event.payload.canGoForward : null,
    })),
    listen<{ state?: unknown; name?: unknown; path?: unknown; size?: unknown }>('panel-browser-download', (event) => {
      const p = event.payload ?? {}
      if (typeof p.state !== 'string') return
      handlers.onDownload?.({
        state: p.state,
        name: typeof p.name === 'string' && p.name ? p.name : null,
        path: typeof p.path === 'string' && p.path ? p.path : null,
        size: typeof p.size === 'number' && Number.isFinite(p.size) ? p.size : null,
      })
    }),
    listen<{ siteUrl?: unknown }>('panel-browser-google-login', (event) => handlers.onGoogleLogin?.(typeof event.payload?.siteUrl === 'string' && event.payload.siteUrl ? event.payload.siteUrl : null)),
  ])
  return () => stops.forEach((stop) => stop())
}

/** 사람이 친 주소. 앞에 http(s):// 가 없으면 https:// 를 붙인다(주소만 받는다. 찾기 말은 뒤쪽이 invalid_url 로 거절). */
export function typedAddress(text: string) {
  const value = text.trim()
  if (!value) return null
  return /^[a-z][a-z0-9+.-]*:\/\//i.test(value) ? value : `https://${value}`
}

// ---- 가림: 몸소 것이 웹뷰 자리를 덮는 동안 웹뷰를 숨긴다. 잡은 수가 0 이 되면 다시 보인다. ----
let holds = 0
let shown = false
const listeners = new Set<() => void>()
function changed() { listeners.forEach((listener) => listener()) }
function subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener) } }

// 웹뷰가 지금 놓인 자리(마지막으로 뒤쪽에 알린 값). 메뉴·풍선이 그 자리와 겹치는지 볼 때 쓴다.
let placed: PanelBounds | null = null

/**
 * 메뉴·풍선·작은 창처럼 HTML 위에 잠깐 뜨는 것이 웹뷰 자리와 겹치면, 떠 있는 동안 웹뷰를 숨긴다(2026-11-18 실제 앱 B71).
 * 웹뷰는 따로 뜨는 층이라 HTML 이 그 위에 그려지지 않는다. 겹치지 않으면 아무것도 하지 않는다. 열린 동안 한 번 숨기면 닫힐 때까지 둔다(깜빡임 없게).
 */
export function useAvoidPanelBrowser(element: RefObject<HTMLElement | null>, open: boolean) {
  const release = useRef<(() => void) | null>(null)
  useLayoutEffect(() => {
    if (!open) {
      release.current?.()
      release.current = null
      return
    }
    if (release.current || !shown || !placed) return
    const rect = element.current?.getBoundingClientRect()
    if (!rect || rect.width < 1 || rect.height < 1) return
    const over = rect.right > placed.x && rect.left < placed.x + placed.width && rect.bottom > placed.y && rect.top < placed.y + placed.height
    if (over) release.current = holdPanelBrowser()
  })
  useEffect(() => () => { release.current?.(); release.current = null }, [])
}

/** 덮는 것이 열리면 잡고, 닫히면 놓는다. 놓는 함수를 돌려준다. */
export function holdPanelBrowser() {
  holds += 1
  changed()
  let released = false
  return () => { if (released) return; released = true; holds -= 1; changed() }
}
/** 열려 있는 동안 웹뷰를 숨기는 부품(묻는 창·찾기 창·크게 보기)이 부른다. */
export function usePanelBrowserHold(active = true) {
  useEffect(() => (active ? holdPanelBrowser() : undefined), [active])
}
export function usePanelBrowserHeld() {
  return useSyncExternalStore(subscribe, () => holds > 0)
}

/** 패널 브라우저 탭이 지금 화면에 보이는가(알림 줄 자리를 옮기는 데 쓴다). */
export function setPanelBrowserShown(value: boolean) {
  if (shown === value) return
  shown = value
  changed()
}
export function usePanelBrowserShown() {
  return useSyncExternalStore(subscribe, () => shown)
}

/** 몸소 안의 웹 링크를 패널에서 열어 달라고 알린다(App 이 패널을 열고, 패널이 브라우저 탭을 연다). */
export function requestPanelBrowser(url: string | null) {
  window.dispatchEvent(new CustomEvent('momso:open-web', { detail: { url } }))
}
