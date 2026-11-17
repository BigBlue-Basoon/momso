// 스킬. 계약: momso-desktop-backend/outputs/2026-11-16-momso-skills-contract.md (2026-11-16 이 화면 폴더에 옮겨짐)
//   skill_list → [{ name, title, description, enabled, builtin, updatedAt, providers, syncState, available, unavailableReason }]
//     enabled = 사용자가 켰는지, available = 이 기기에서 확인할 수 있는 조건으로 쓸 수 있는지(사이트·AI 회사 로그인까지는 보장하지 않음).
//     unavailableReason: disabled | skill_conflict | policy_modified | policy_unavailable | browser_unavailable | null (desktop-screen-backend-contract.md "스킬")
//   skill_read({ name }) → 목록 항목 + { body, contentHash }
//   skill_create({ name, title, description, body }), skill_update({ name, changes, expectedHash }),
//   skill_set_enabled({ name, enabled }), skill_remove({ name }) → { status: 'ok' } (고친 뒤 모양은 skill_read 로 다시 읽는다)
//   오류 문자열: invalid_name, already_exists, not_found, invalid_format, conflict, builtin, invalid_state, io_error, save_failed, rollback_failed
//   chat_send 의 skill(스킬 name, 고르지 않았으면 null) → 쓸 수 없는 스킬이면 말을 저장하지 않고 { status: 'skill_unavailable' }.
// skill_list 가 없으면 입력 칸의 / 목록과 설정의 "스킬" 쪽을 그리지 않는다.
import { invoke } from '@tauri-apps/api/core'
import { isDesktopRuntime } from './runtime'
import { W } from './words'

export type SkillEntry = {
  name: string
  title: string
  description: string
  enabled: boolean
  builtin: boolean
  updatedAt: string | null
  providers?: string[] | null
  /** ok = 정본과 도구 사본이 같음, disabled = 꺼짐, conflict = 사본이 정본과 다름. */
  syncState?: 'ok' | 'disabled' | 'conflict' | null
  available?: boolean
  unavailableReason?: string | null
}

/** 쓸 수 없는 까닭의 쉬운 말(꺼 둔 것은 null: 켜고 끄기 단추가 이미 보인다). 값을 모르면 '쓸 수 없음'만. */
export function skillUnavailableText(item: SkillEntry) {
  if (item.available !== false || item.unavailableReason === 'disabled') return null
  const reasons = W.skills.unavailableReason as Record<string, string>
  return (item.unavailableReason && reasons[item.unavailableReason]) || W.skills.unavailableTag
}

export type SkillDocument = SkillEntry & { body: string; contentHash: string }

export type SkillChanges = { title?: string; description?: string; body?: string }

function need() {
  if (!isDesktopRuntime()) throw new Error('skills need the desktop runtime')
}

export async function listSkills() { need(); return invoke<SkillEntry[]>('skill_list') }
export async function readSkill(name: string) { need(); return invoke<SkillDocument>('skill_read', { name }) }
type SkillOk = { status: 'ok' }
export async function createSkill(input: { name: string; title: string; description: string; body: string }) { need(); return invoke<SkillOk>('skill_create', input) }
export async function updateSkill(name: string, changes: SkillChanges, expectedHash: string) { need(); return invoke<SkillOk>('skill_update', { name, changes, expectedHash }) }
export async function setSkillEnabled(name: string, enabled: boolean) { need(); return invoke<SkillOk>('skill_set_enabled', { name, enabled }) }
export async function removeSkill(name: string) { need(); return invoke<SkillOk>('skill_remove', { name }) }

const SKILL_ERRORS = ['invalid_name', 'already_exists', 'not_found', 'invalid_format', 'conflict', 'builtin', 'invalid_state', 'io_error', 'save_failed', 'rollback_failed'] as const
export type SkillErrorCode = typeof SKILL_ERRORS[number]

/** 백엔드의 오류 문자열에서 상태 값을 읽는다. */
export function skillErrorCode(error: unknown): SkillErrorCode | null {
  const text = String((error as { message?: string })?.message ?? error)
  return SKILL_ERRORS.find((code) => text.includes(code)) ?? null
}

/** 부르는 이름(뒤쪽 skills.rs valid_name 과 같게): 영어 소문자·숫자·붙임표, 64자까지, 붙임표로 시작·끝나거나 둘 겹치지 않음. */
export const SKILL_NAME_PATTERN = /^(?=.{1,64}$)[a-z0-9]+(?:-[a-z0-9]+)*$/
