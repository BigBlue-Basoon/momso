// 권한 단계의 화면용 한국어(한 곳에만 둔다).
// 열쇠는 '회사:단계 id'(예: 'claude:plan'). 값은 요가 선생님이 읽기 쉬운 이름과 한 줄 설명.
// 이 글은 본부가 써서 김성균 개발자의 검수를 받은 뒤에 낱말 파일에 넣는다. 화면 담당이 지어 넣지 않는다.
// 여기에 없는 단계는 백엔드가 준 회사의 원문(label, description)을 그대로 보인다.
import type { AiProvider } from './ai-models'
import { W } from './words'

export type PermissionKorean = { label: string; description?: string | null }

// 글은 낱말 파일(words.ts)의 permission 칸에 있다.
export const PERMISSION_KO: Record<string, PermissionKorean> = W.permission

// 회사마다 가장 높은 권한 단계. 메뉴와 단추에서 주의를 주는 색으로 구분한다(글자와 작은 아이콘만).
const STRONGEST_PERMISSION = new Set(['codex:Full access', 'claude:bypassPermissions', 'gemini:yolo'])

// 메뉴의 차례: 그 회사 앱이 보이는 차례(Claude 앱: 자동, 수동, 편집 자동 수락, 계획, 권한 무시). 없는 회사는 백엔드 차례 그대로.
const PERMISSION_ORDER: Partial<Record<AiProvider, string[]>> = {
  claude: ['auto', 'default', 'acceptEdits', 'plan', 'bypassPermissions'],
}

export function sortPermissionModes<T extends { id: string }>(provider: AiProvider, modes: T[]) {
  const order = PERMISSION_ORDER[provider]
  if (!order) return modes
  const rank = (id: string) => { const at = order.indexOf(id); return at < 0 ? order.length : at }
  return [...modes].sort((a, b) => rank(a.id) - rank(b.id))
}

export function isStrongestPermission(provider: AiProvider, id: string) {
  return STRONGEST_PERMISSION.has(`${provider}:${id}`)
}

type DevWindow = Window & { __momsoPermissionKo?: Record<string, PermissionKorean> }

/** 그 단계의 화면용 한국어. 없으면 null(원문을 쓴다). 개발용 실행에서만 시험용 표를 덧댈 수 있다. */
export function permissionKorean(provider: AiProvider, id: string): PermissionKorean | null {
  const key = `${provider}:${id}`
  if (import.meta.env.DEV) {
    const extra = (window as DevWindow).__momsoPermissionKo
    if (extra?.[key]) return extra[key]
  }
  return PERMISSION_KO[key] ?? null
}

// ---- 추론 수준의 화면용 한국어(같은 방식). 회사의 공식 한국어가 백엔드에서 오면 그것을 그대로 쓰고,
// 영어뿐인 것만 본부가 써서 검수받은 글을 여기에 적는다. 열쇠는 '회사:추론 수준 id', 설명 한 줄은 '회사:note'. ----

// 글은 낱말 파일(words.ts)의 effort 칸에 있다.
export const EFFORT_KO: Record<string, PermissionKorean> = W.effort

/**
 * 추론 수준의 화면 이름. 차례: 백엔드가 한글 이름을 주면 그것 → 그 회사 표(words.effort) → 값(id)마다의 한글(words.effortName) → 없음(원문).
 */
export function effortKorean(provider: AiProvider, id: string, backendLabel?: string | null): PermissionKorean | null {
  if (backendLabel && /[가-힣]/.test(backendLabel)) return { label: backendLabel }
  const byCompany = EFFORT_KO[`${provider}:${id}`]
  if (byCompany) return byCompany
  const byId = W.effortName[id.toLowerCase()]
  return byId ? { label: byId } : null
}

export function effortNoteKorean(provider: AiProvider): string | null {
  return EFFORT_KO[`${provider}:note`]?.label ?? null
}
