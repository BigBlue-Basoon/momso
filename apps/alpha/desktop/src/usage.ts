// AI 세 곳의 남은 사용량을 부르는 얇은 층. 모양은 본부가 전한 계약을 따른다(usage_status, usage-updated 이벤트).
// 백엔드가 들어오면 momso-public-prep/archive-backend-port-notes.md 의 사용량 절과 견준다.
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isMissingCommand } from './ai-models'
import { isDesktopRuntime } from './runtime'

export type UsageProvider = 'codex' | 'claude' | 'gemini'

/** 사용량의 한 기간(예: 5시간, 1주). usedPercent 는 쓴 양(0~100), resetsAt 은 다시 차는 때(백엔드는 Unix 초로 준다). */
export type UsageWindow = {
  kind: string
  usedPercent: number | null
  resetsAt: number | string | null
  /** 그 회사가 붙인 갈래 이름(오면 그대로 보인다). */
  label?: string | null
  /** 특정 모델(또는 서비스)만 따로 세는 갈래. 코덱스는 글, 클로드는 회사가 준 JSON 그대로(예: { model: { display_name } }). */
  scope?: unknown
  /** 코덱스가 붙인 갈래 번호(오면). */
  limitId?: string | null
}

/** 갈래가 가리키는 모델 이름. 회사가 준 이름 칸만 읽고, 없으면 null(지어 넣지 않는다). */
export function scopeName(scope: unknown): string | null {
  if (typeof scope === 'string') return scope.trim() || null
  if (!scope || typeof scope !== 'object') return null
  const value = scope as Record<string, unknown>
  for (const inner of [value.model, value]) {
    if (typeof inner === 'string' && inner.trim()) return inner.trim()
    if (inner && typeof inner === 'object') {
      const item = inner as Record<string, unknown>
      for (const key of ['display_name', 'displayName', 'name']) {
        const text = item[key]
        if (typeof text === 'string' && text.trim()) return text.trim()
      }
    }
  }
  return null
}

export type ProviderUsage = {
  provider: UsageProvider
  /** ok: 읽었다, not_connected: 연결하지 않았다, unavailable: 연결했지만 사용량을 읽을 수 없다. */
  state: 'ok' | 'not_connected' | 'unavailable'
  /** 백엔드가 주는 까닭 칸. 화면에는 내보이지 않는다(SCREEN_RULES 1절). */
  reason: string | null
  windows: UsageWindow[]
  plan: string | null
  /** 알아보기 쉬운 요금제 이름(백엔드가 아는 것만). 없으면 요금제를 보이지 않는다. */
  planLabel?: string | null
  checkedAt: string | null
  /** 개발용 보기 숫자인지(화면 안에서만 쓰는 표시, 백엔드는 보내지 않는다). 보기 숫자는 저장하지 않는다. */
  sample?: boolean
}

/** 읽는 데 시간이 걸리는 곳(백엔드가 그 회사의 CLI 에 물어 읽는다). 마우스를 올리면 새로 읽는다. */
export const SLOW_PROVIDERS: UsageProvider[] = ['claude', 'gemini']

export async function getUsageStatus(): Promise<ProviderUsage[]> {
  if (!isDesktopRuntime()) throw new Error('usage_status needs the desktop runtime')
  let result: ProviderUsage[]
  try {
    result = await invoke<ProviderUsage[]>('usage_status')
  } catch (error) {
    // 개발용 실행에서 백엔드에 usage_status 가 아예 없을 때: 세 곳 모두 보기 숫자.
    if (import.meta.env.DEV && isMissingCommand(error)) return PROVIDER_IDS.map(devSample)
    throw error
  }
  if (!import.meta.env.DEV) return result
  // 개발용 실행에서는 곳 단위로 가른다: unavailable 로 오는 곳만 보기 숫자, ok 와 not_connected 는 받은 그대로.
  return result.map((item) => (item.state === 'unavailable' ? devSample(item.provider) : item))
}

/**
 * 마우스를 올렸을 때 그 곳을 새로 읽는다. 계약대로 usage_status 를 다시 부른다
 * (Claude·Gemini 는 백엔드가 뒤에서 읽고 다 되면 usage-updated 로 알린다).
 */
export async function readUsageNow(provider: UsageProvider) {
  if (import.meta.env.DEV && devSampled.has(provider)) {
    // 보기 숫자: 읽는 시간을 흉내 낸 뒤 70% 남음에서 40% 남음으로 내려간다. 앱을 새로 고치면 다시 70%.
    await new Promise((resolve) => window.setTimeout(resolve, DEV_READ_MS))
    devRead[provider] = Date.now()
  }
  // 사람이 원해서 새로 읽을 때는 usage_refresh 로 백엔드의 5분 제한을 건너뛴다(없으면 usage_status 만).
  try {
    await invoke<unknown>('usage_refresh', { provider })
  } catch {
    // 명령이 없거나 실패해도 지금 가진 값으로 그린다.
  }
  return getUsageStatus()
}

// ---- 개발용 보기 숫자. import.meta.env.DEV 가 false 인 내보내는 빌드에서는 위의 두 길이 빠져 쓰이지 않는다. ----

const PROVIDER_IDS: UsageProvider[] = ['codex', 'claude', 'gemini']
const DEV_READ_MS = 1400
const devLoadedAt = Date.now()
const devSampled = new Set<UsageProvider>()
const devRead: Partial<Record<UsageProvider, number>> = {}

function devSample(provider: UsageProvider): ProviderUsage {
  devSampled.add(provider)
  const at = (hours: number) => Math.round((devLoadedAt + hours * 3600 * 1000) / 1000)
  if (provider === 'codex') {
    return { provider, state: 'ok', reason: null, plan: 'Plus', sample: true, checkedAt: new Date(devLoadedAt - 2 * 60 * 1000).toISOString(), windows: [
      { kind: 'five_hour', usedPercent: 38, resetsAt: at(2.5) },
      { kind: 'weekly', usedPercent: 12, resetsAt: at(80) },
    ] }
  }
  // Claude·Gemini: 처음에는 마지막으로 확인한 값처럼 70% 남음(3시간 전), 새로 읽으면 40% 남음.
  const readAt = devRead[provider]
  const usedPercent = readAt ? 60 : 30
  const checkedAt = new Date(readAt ?? devLoadedAt - 3 * 3600 * 1000).toISOString()
  return provider === 'claude'
    ? { provider, state: 'ok', reason: null, plan: 'Pro', sample: true, checkedAt, windows: [{ kind: 'weekly', usedPercent, resetsAt: at(30) }] }
    : { provider, state: 'ok', reason: null, plan: 'AI Pro', sample: true, checkedAt, windows: [{ kind: 'daily', usedPercent, resetsAt: at(11) }] }
}

// ---- 마지막으로 확인한 값. 실제 값만 이 기기에 적어 두고, 지금 읽지 못한 곳은 이것으로 그린다. ----

const LAST_KEY = 'momso.desktop.usage-last.v1'

export function loadLastUsage(): Partial<Record<UsageProvider, ProviderUsage>> {
  try {
    const stored = window.localStorage.getItem(LAST_KEY)
    return stored ? JSON.parse(stored) as Partial<Record<UsageProvider, ProviderUsage>> : {}
  } catch {
    return {}
  }
}

export function saveLastUsage(last: Partial<Record<UsageProvider, ProviderUsage>>) {
  try {
    window.localStorage.setItem(LAST_KEY, JSON.stringify(last))
  } catch {
    // 저장하지 못해도 화면은 지금 값으로 그린다.
  }
}

/** 백엔드가 사용량이 바뀌었다고 알려 주면 부른다. 알려 주지 않는 곳은 화면이 가끔 다시 묻는다. */
export async function onUsageUpdated(callback: () => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen('usage-updated', () => callback())
}

/** 남은 양(0~100). 기간이 여럿이면 가장 적게 남은 것. 알 수 없으면 null. */
export function remainingPercent(usage: ProviderUsage) {
  if (usage.state !== 'ok') return null
  const values = usage.windows
    .map((window) => window.usedPercent)
    .filter((value): value is number => typeof value === 'number' && Number.isFinite(value))
  if (values.length === 0) return null
  return Math.max(0, Math.min(100, 100 - Math.max(...values)))
}

/** 다시 차는 때를 Date 로. Unix 초(백엔드), 밀리초, 글자 시각을 모두 받는다. 알 수 없으면 null. */
export function resetDate(value: number | string | null) {
  if (value === null || value === undefined || value === '') return null
  const date = typeof value === 'number'
    ? new Date(value < 1e12 ? value * 1000 : value)
    : new Date(value)
  return Number.isNaN(date.getTime()) ? null : date
}
