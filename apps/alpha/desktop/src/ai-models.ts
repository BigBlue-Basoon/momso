// 입력 칸 아래 줄(모델, 권한 단계)이 부르는 얇은 층.
// 권한 단계: 백엔드에 있는 명령(permission_mode_get, permission_mode_set, permission_modes_available).
//   모양은 momso-public-prep/archive-backend-port-notes.md 의 "권한 단계" 절을 따른다.
// 모델 목록: 본부가 전한 계약(ai_models). 백엔드가 아직 없다. 들어오면 그 절과 견준다.
import { invoke } from '@tauri-apps/api/core'
import { latestPerFamily } from './model-families'
import { W, modelDisplayName } from './words'
import { isDesktopRuntime } from './runtime'

export type AiProvider = 'codex' | 'claude' | 'gemini'

// 회사 이름은 낱말 파일에서(한글판: 코덱스, 클로드, 제미나이).
export const PROVIDER_NAME: Record<AiProvider, string> = W.provider

export const PROVIDER_ORDER: AiProvider[] = ['codex', 'claude', 'gemini']

/**
 * 모델 하나(계열마다 가장 최신 것 하나만 온다). 본부가 전한 계약:
 * { id, family, label, resolved, efforts, defaultEffort, isDefault }. 이름은 백엔드가 준 그대로 보인다.
 */
export type AiModel = {
  /** 요청에 넣는 값. */
  id: string
  /** 계열 이름(예: 그 회사가 쓰는 표기 그대로). */
  family?: string | null
  /** 화면에 보이는 이름. 없으면 family, 그것도 없으면 id. */
  label?: string | null
  /** 별칭이 가리키는 실제 모델(읽히면). 화면에는 보이지 않는다. */
  resolved?: string | null
  /** 이 모델이 받는 추론 수준. 예전 계약은 값의 목록, 새 계약은 항목의 목록. 둘 이상일 때만 고르기가 보인다. */
  efforts?: Array<string | EffortOption> | null
  defaultEffort?: string | null
  /** 추론 수준 전체의 설명 한 줄(그 회사의 글). */
  effortNote?: string | null
  /** 켜고 끄는 특별한 항목(예: 클로드 Ultracode). 계약: { id, label, description, available, state }. 슬라이더 아래 스위치 자리에 둔다. */
  toggles?: Array<{ id: string; label?: string | null; description?: string | null; available?: boolean; state?: boolean | null }> | null
  /** 빠른 모드(가장 비싼 요금제 등에서 켤 수 있는 것). 받지 않는 모델이면 없거나 supported false. */
  fast?: { supported: boolean; state: 'on' | 'off' | 'cooldown' | 'unavailable'; label?: string | null } | null
  /** 그 곳의 기본 모델인지. */
  isDefault?: boolean
  /** 예전 짐작 모양(name, description). 백엔드가 주면 쓴다. */
  name?: string | null
  description?: string | null
}

/** 곳 하나의 모델 목록. 읽지 못했으면 state 가 ok 가 아니고 reason 에 그 까닭이 온다. */
export type ProviderModels = {
  provider: AiProvider
  state: 'ok' | 'unavailable' | 'not_connected'
  reason?: string | null
  models: AiModel[]
}

export type ChosenModel = {
  provider: AiProvider
  /** 모델 목록을 읽지 못했을 때는 null(그 곳의 기본 모델로 보낸다). */
  model: string | null
  /** 고른 추론 수준. 없으면 그 모델의 기본값. */
  effort?: string | null
  /** 빠른 모드를 켰는지. 없으면 백엔드가 준 상태(on 이면 켜짐). */
  fast?: boolean | null
}

/**
 * 빠른 모드를 고를 수 있는지는 ai_models 가 준 값대로만 정한다: 모델이 받고(supported) 상태가 on/off/unavailable 이면 고를 수 있다.
 * unavailable 은 "지금 켜져 있는지 확인되지 않음"이라 보내 보고, 안 되면 백엔드가 알린다(클로드의 fast_unavailable).
 * 그렇게 한 번 안 된 모델(refused)은 다시 고를 수 없다. 잠시 못 씀(cooldown)은 고를 수 없다.
 * 코덱스의 fast 도 그대로 전달된다(적용됐는지는 확인되지 않는다). provider 는 예전 부르는 곳과 맞추려고 남겨 둔다.
 */
export function fastChoosable(_provider: AiProvider, model: AiModel | undefined, refused = false) {
  const fast = model?.fast
  if (!fast?.supported || refused) return false
  return fast.state === 'on' || fast.state === 'off' || fast.state === 'unavailable'
}

/** 그 모델에서 빠른 모드가 지금 켜진 것으로 보낼지. 받지 않거나 쓸 수 없으면 null(보내지 않음). */
export function resolveFast(provider: AiProvider, model: AiModel | undefined, wanted: boolean | null | undefined, refused = false) {
  if (!fastChoosable(provider, model, refused)) return null
  const state = model?.fast?.state
  if (state === 'on' || state === 'off') return wanted ?? state === 'on'
  return wanted === true ? true : null
}

/** 백엔드에 그 명령이 아직 없을 때 Tauri 가 돌려주는 글인지. */
export function isMissingCommand(error: unknown) {
  const message = error instanceof Error ? error.message : String(error)
  return /command\s+\S+\s+not found|unknown command/i.test(message)
}

/** 회사별 모델 목록. 계열마다 가장 최신 것 하나만 남긴다(model-families.ts). */
export async function listAiModels() {
  if (!isDesktopRuntime()) throw new Error('ai_models needs the desktop runtime')
  const lists = await invoke<ProviderModels[]>('ai_models')
  return lists.map((list) => ({ ...list, models: latestPerFamily(list.models ?? []) }))
}

/** 추론 수준 하나(새 계약). special 은 그 회사에만 있는 특별한 수준(예: ultra). */
export type EffortOption = {
  id: string
  label: string
  description: string | null
  special: boolean
  available: boolean
  /** 그 회사가 추천으로 표시한 수준(오면). */
  recommended?: boolean
  /** 'level' = 차례가 있는 수준(슬라이더), 'toggle' = 켜고 끄는 것(슬라이더 아래 스위치). 오지 않으면 수준으로 본다. */
  kind?: 'level' | 'toggle' | null
}

/** 예전(값의 목록)과 새(항목의 목록) 계약을 같은 모양으로. 따로 오는 toggles 는 kind 'toggle' 로 뒤에 붙인다. */
export function effortOptions(model: AiModel | undefined): EffortOption[] {
  const toggles: EffortOption[] = (model?.toggles ?? []).map((item) => ({
    id: item.id,
    label: item.label?.trim() || item.id,
    description: item.description ?? null,
    special: false,
    available: item.available === true,
    recommended: false,
    kind: 'toggle',
  }))
  const levels = levelOptions(model)
  return [...levels, ...toggles.filter((toggle) => !levels.some((item) => item.id === toggle.id))]
}

function levelOptions(model: AiModel | undefined): EffortOption[] {
  return (model?.efforts ?? []).map((item) => (typeof item === 'string'
    ? { id: item, label: item, description: null, special: false, available: true, recommended: false, kind: null }
    : {
        id: item.id,
        label: item.label?.trim() || item.id,
        description: item.description ?? null,
        special: Boolean(item.special),
        available: item.available !== false,
        recommended: Boolean(item.recommended),
        kind: item.kind === 'toggle' ? 'toggle' : item.kind === 'level' ? 'level' : null,
      }))
}

/** 이름을 짜 맞출 때 읽는 회사의 글(화면 이름, 계열, 이름, id 차례). */
export function modelLabelSource(model: AiModel) {
  return model.label?.trim() || model.family?.trim() || model.name?.trim() || model.id
}

export function modelName(model: AiModel) {
  return modelDisplayName(modelLabelSource(model))
}

/**
 * 그 모델에 보낼 추론 수준: 고른 값이 그 모델이 쓸 수 있는 값이면 그것, 아니면 백엔드가 준 기본값.
 * 기본값도 없으면 null(보내지 않아 그 회사의 기본으로 가게 한다. 화면이 값을 짐작해 고르지 않는다).
 */
export function resolveEffort(model: AiModel | undefined, wanted: string | null | undefined) {
  const usable = effortOptions(model).filter((item) => item.available).map((item) => item.id)
  if (wanted && usable.includes(wanted)) return wanted
  if (model?.defaultEffort && (usable.length === 0 || usable.includes(model.defaultEffort))) return model.defaultEffort
  return null
}

/** 모델 이름 찾기. 목록에 없으면 받은 값을 그대로 보인다. */
export function findModelName(lists: ProviderModels[] | null, provider: AiProvider | null | undefined, id: string | null | undefined) {
  if (!id) return null
  const found = lists?.find((list) => list.provider === provider)?.models.find((model) => model.id === id)
  return found ? modelName(found) : modelDisplayName(id)
}

export function defaultModel(list: ProviderModels | undefined) {
  if (!list || list.state !== 'ok' || list.models.length === 0) return null
  return list.models.find((model) => model.isDefault) ?? list.models[0]
}

// ---- 권한 단계(회사마다 다르다) ----
// 본부가 전한 계약: permission_modes_available({ provider }) → [{ id, label, description, available, isDefault }]
// (가장 높은 단계에 그 회사 앱의 경고가 있으면 warning 으로 온다고 짐작해 둠),
// permission_mode_get({ provider }) → id, permission_mode_set({ provider, mode }).
// 백엔드의 권한 대목이 고쳐 쓰이면 그것과 견준다.

export type PermissionModeOption = {
  /** 그 회사의 값 그대로. */
  id: string
  /** 그 회사의 원문 이름(화면용 한국어가 없을 때 보인다). */
  label: string
  description?: string | null
  available: boolean
  isDefault?: boolean
  /** 그 회사 앱이 이 단계를 고를 때 보이는 경고(오면 그대로 보인다). */
  warning?: string | null
}

export async function getPermissionMode(provider: AiProvider) {
  if (!isDesktopRuntime()) throw new Error('permission_mode_get needs the desktop runtime')
  return invoke<string>('permission_mode_get', { provider })
}

export async function setPermissionMode(provider: AiProvider, mode: string) {
  if (!isDesktopRuntime()) throw new Error('permission_mode_set needs the desktop runtime')
  await invoke<null>('permission_mode_set', { provider, mode })
}

/** 회사의 권한 단계 목록. 예전 계약(다섯 값, 이름 없음)으로 오면 쓰지 않는다(화면이 이름을 지어 넣지 않게). */
export async function listPermissionModes(provider: AiProvider) {
  if (!isDesktopRuntime()) throw new Error('permission_modes_available needs the desktop runtime')
  const items = await invoke<Array<Partial<PermissionModeOption>>>('permission_modes_available', { provider })
  const options = items.filter((item): item is PermissionModeOption => typeof item.id === 'string' && typeof item.label === 'string')
  if (options.length === 0 || options.length !== items.length) throw new Error('permission_modes_available: old contract')
  return options
}
