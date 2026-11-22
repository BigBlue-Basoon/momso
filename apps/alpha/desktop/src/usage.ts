// AI 세 곳의 남은 사용량을 부르는 얇은 층. 모양은 본부가 전한 계약을 따른다(usage_status, usage-updated 이벤트).
// 백엔드가 들어오면 momso-public-prep/archive-backend-port-notes.md 의 사용량 절과 견준다.
import { invoke, listen } from './host'
import { isDesktopRuntime } from './runtime'
import { W } from './words'

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
  /** 백엔드가 주는 까닭 값(G18, 화면 문장이 아님). 단추의 풍선에 쉬운 말로 보인다(usageReason). */
  reason: string | null
  windows: UsageWindow[]
  plan: string | null
  /** 알아보기 쉬운 요금제 이름(백엔드가 아는 것만). 없으면 요금제를 보이지 않는다. */
  planLabel?: string | null
  checkedAt: string | null
}

/** 읽는 데 시간이 걸리는 곳(백엔드가 그 회사의 CLI 에 물어 읽는다). 마우스를 올리면 새로 읽는다. */
export const SLOW_PROVIDERS: UsageProvider[] = ['claude', 'gemini']

export async function getUsageStatus(): Promise<ProviderUsage[]> {
  if (!isDesktopRuntime()) throw new Error('usage_status needs the desktop runtime')
  // 받은 그대로 그린다(개발용 실행도 같다). 읽지 못한 곳(unavailable)에 숫자를 지어 넣지 않는다(2026-11-16 그의 말: 화면에 임시·시험용 표시 없음).
  return invoke<ProviderUsage[]>('usage_status')
}

/**
 * 마우스를 올렸을 때 그 곳을 새로 읽는다. 계약대로 usage_status 를 다시 부른다
 * (Claude·Gemini 는 백엔드가 뒤에서 읽고 다 되면 usage-updated 로 알린다).
 */
export async function readUsageNow(provider: UsageProvider) {
  // 사람이 원해서 새로 읽을 때는 usage_refresh 로 백엔드의 5분 제한을 건너뛴다(없으면 usage_status 만).
  try {
    await invoke<unknown>('usage_refresh', { provider })
  } catch {
    // 명령이 없거나 실패해도 지금 가진 값으로 그린다.
  }
  return getUsageStatus()
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

/**
 * 숫자를 읽지 못한 곳의 까닭(usage_status 의 reason 값, G18: checking, not_installed, not_connected, api_key, no_quota_api, fetch_failed)을
 * 풍선에 보일 쉬운 말로. 모르는 값이나 null 이면 null(이름만 보인다).
 */
export function usageReason(usage: ProviderUsage | null | undefined, _name: string) {
  const reason = usage?.reason?.trim()
  return reason ? W.usageMeter.reason[reason] ?? null : null
}
