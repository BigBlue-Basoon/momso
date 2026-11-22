// 용어 사전. 계약: 뒤쪽 docs/development/desktop-screen-backend-contract.md "용어 사전"과 yoga-dictionary-draft.md(yoga_dictionary.rs).
//   yoga_terms_list() → { bundleVersion, reviewStatus, baseStatus, baseConflicts, terms, fragments, people, peopleStatus, userRevision, peopleRevision }
//   terms[]: { id, enabled, sanskrit, roman, korean[], koreanNames[], meaning, category, variants[], searchOnly[], origin, sourceFile, conflict, … }
//     origin: 'default'(몸소가 준 기본 용어) | 'user'(내가 더하거나 고친 것, 내 파일에만 쓰인다)
//     variants[]: 화면에 보일 표기. origin 'user' 는 내가 더한 표기. searchOnly[]: 화면에 보이지 않고 찾기에만 쓰는 표기("할 아사나").
//   people[]: { id, name, aliases[], confirmed, origin, sourceFile }. peopleStatus 'invalid' 이면 이름 파일을 읽지 못한 것(원본은 그대로).
//   yoga_term_edit({ input: { id(새 용어면 null), addVariants, removeVariants, enabled, meaning, expectedRevision } }) → 항목 id
//   yoga_person_edit({ input: { id, name, aliases, confirmedByUser, expectedRevision } }) → 항목 id
//   yoga_person_delete({ id, expectedRevision }) → null
//   expectedRevision 은 직전 목록의 userRevision(용어) / peopleRevision(이름). 낡았으면 dictionary_changed. 쓰고 나면 목록을 다시 읽는다.
//   성공하면 yoga-dictionary-updated { source: 'terms' | 'people' } 가 온다(AI 가 용어를 더했을 때도).
//   2026-11-17: 각 용어의 userKind 는 'modified'(기본을 고침) | 'added'(내가 새로 더함) | null(기본 그대로). origin 만으로 "내가 더함"을 적지 않는다.
//   canDelete 가 참이면 yoga_term_delete({ id, expectedRevision }) → 'deleted' | 'base_term'(기본 용어는 지울 수 없고 끄기만 된다).
//   yoga_term_edit 의 input.category 는 새 용어의 갈래(고르지 않으면 뒤쪽이 '사용자').
import { invoke, listen } from './host'
import { hasAppBackend, isDesktopRuntime } from './runtime'

export type DictionaryVariant = { text: string; origin?: string; label?: string; recognitionOnly?: boolean }
export type DictionaryTerm = {
  id: string
  enabled?: boolean
  sanskrit: string
  roman?: string
  korean: string[]
  koreanNames: string[]
  meaning: string
  category?: string
  variants?: DictionaryVariant[]
  searchOnly?: DictionaryVariant[]
  origin: 'default' | 'user' | string
  userKind?: 'modified' | 'added' | null
  canDelete?: boolean
}
export type DictionaryPerson = { id: string | null; name: string; aliases: string[]; confirmed: boolean; origin?: string }
export type DictionaryList = {
  terms: DictionaryTerm[]
  people: DictionaryPerson[]
  peopleStatus: string
  userRevision: string
  peopleRevision: string
}

// 데스크톱 또는 휴대폰 몸소(S75 server 의 /dictionary). 바뀜 알림(yoga-dictionary-updated)은 데스크톱에만 있다.
function need() {
  if (!hasAppBackend()) throw new Error('yoga dictionary needs the desktop runtime')
}

export async function listYogaTerms(): Promise<DictionaryList> {
  need()
  const value = await invoke<Partial<DictionaryList> | null>('yoga_terms_list')
  return {
    terms: Array.isArray(value?.terms) ? value.terms : [],
    people: Array.isArray(value?.people) ? value.people : [],
    peopleStatus: value?.peopleStatus ?? 'ok',
    userRevision: value?.userRevision ?? '',
    peopleRevision: value?.peopleRevision ?? '',
  }
}

export type TermEditInput = { id: string | null; addVariants?: string[]; removeVariants?: string[]; enabled?: boolean; meaning?: string; category?: string; expectedRevision: string }
export async function editYogaTerm(input: TermEditInput) { need(); return invoke<string>('yoga_term_edit', { input }) }
/** 내가 더한 용어 지우기. 기본 용어면 'base_term' 을 돌려주므로 실패로 던진다. */
export async function deleteYogaTerm(id: string, expectedRevision: string) {
  need()
  const result = await invoke<string>('yoga_term_delete', { id, expectedRevision })
  if (result !== 'deleted') throw result || 'base_term'
  return result
}

export type PersonEditInput = { id: string | null; name: string; aliases: string[]; confirmedByUser: boolean; expectedRevision: string }
export async function editYogaPerson(input: PersonEditInput) { need(); return invoke<string>('yoga_person_edit', { input }) }
export async function deleteYogaPerson(id: string, expectedRevision: string) { need(); return invoke<null>('yoga_person_delete', { id, expectedRevision }) }

export async function listenDictionaryUpdated(onUpdated: () => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen('yoga-dictionary-updated', () => onUpdated())
}

const DICTIONARY_ERRORS = ['base_term', 'dictionary_changed', 'dictionary_duplicate', 'invalid_input', 'not_found', 'dictionary_unsafe', 'dictionary_invalid', 'dictionary_unavailable'] as const
export function dictionaryErrorCode(error: unknown) {
  const text = String((error as { message?: string })?.message ?? error)
  return DICTIONARY_ERRORS.find((code) => text.includes(code)) ?? null
}

/** 화면에 보일 표기들: 한글 표기, 우리말 이름, 화면용 다른 표기(알아보기용은 뺀다). 같은 글은 한 번만. */
export function shownSpellings(term: DictionaryTerm) {
  const seen = new Set<string>()
  const out: string[] = []
  const add = (text: string | undefined) => {
    const value = text?.trim()
    if (!value) return
    const key = value.toLocaleLowerCase('ko-KR')
    if (seen.has(key)) return
    seen.add(key)
    out.push(value)
  }
  term.korean.forEach(add)
  term.koreanNames.forEach(add)
  ;(term.variants ?? []).filter((variant) => !variant.recognitionOnly).forEach((variant) => add(variant.text))
  return out
}

/** 내가 더한 표기(지울 수 있는 것). */
export function mySpellings(term: DictionaryTerm) {
  return new Set((term.variants ?? []).filter((variant) => variant.origin === 'user').map((variant) => variant.text.trim()))
}

function fold(text: string) {
  return text.toLocaleLowerCase('ko-KR').normalize('NFD').replace(/[̀-ͯ]/g, '').replace(/\s+/g, '')
}

/** 찾기에 쓰는 글: 산스크리트 로마자, 보이는 표기, 찾기에만 쓰는 표기(searchOnly), 뜻. 띄어쓰기와 로마자의 점은 무시한다. */
export function searchText(term: DictionaryTerm) {
  return fold([term.sanskrit, term.roman ?? '', ...shownSpellings(term), ...(term.searchOnly ?? []).map((variant) => variant.text), term.meaning].join(' '))
}

export function personSearchText(person: DictionaryPerson) {
  return fold([person.name, ...person.aliases].join(' '))
}

export function normalizeQuery(text: string) {
  return fold(text.trim())
}
