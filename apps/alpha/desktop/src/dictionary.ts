// 용어 사전(읽기만). 계약: archive-backend-port-notes.md "2026-11-16 옮긴 것: 용어 사전과 내장 스킬"(yoga_dictionary.rs).
//   yoga_terms_list() → { bundleVersion, reviewStatus, baseStatus, baseConflicts, terms, fragments }
//   terms[]: { id, sanskrit, roman, language, korean[], koreanNames[], meaning, category, variants[], ambiguityReason, origin, sourceFile, conflict, … }
//     origin: 'default'(몸소가 준 기본 요가 용어, dictionary/yoga.json) | 'user'(내가 더하거나 고친 것, dictionary/my-terms.json)
//     variants[]: { text, origin, label, recognitionOnly, requiresConfirmation, … } — 뒤쪽이 화면용으로 실제로 본 표기만 남겨 준다
//     (규칙으로 만든 알아보기용 표기는 빼고 준다). 화면은 그래도 recognitionOnly 가 붙은 것은 한 번 더 거른다.
//   사람 이름(dictionary/people.json)은 이 명령이 주지 않는다. 쓰기 명령은 아직 없다.
import { invoke } from '@tauri-apps/api/core'
import { isDesktopRuntime } from './runtime'

export type DictionaryVariant = { text: string; origin?: string; label?: string; recognitionOnly?: boolean }
export type DictionaryTerm = {
  id: string
  sanskrit: string
  roman?: string
  korean: string[]
  koreanNames: string[]
  meaning: string
  category?: string
  variants?: DictionaryVariant[]
  origin: 'default' | 'user' | string
}
export type DictionaryList = { terms: DictionaryTerm[] }

export async function listYogaTerms() {
  if (!isDesktopRuntime()) throw new Error('yoga_terms_list needs the desktop runtime')
  const value = await invoke<Partial<DictionaryList> | null>('yoga_terms_list')
  return { terms: Array.isArray(value?.terms) ? value.terms : [] } as DictionaryList
}

/** 화면에 보일 표기들: 한글 표기, 우리말 이름, 실제로 본 다른 표기(알아보기용은 뺀다). 같은 글은 한 번만. */
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

/** 찾기에 쓰는 글: 산스크리트 로마자, 한글 표기, 우리말 이름, 실제로 본 표기, 뜻. 띄어쓰기는 무시한다("할 아사나"와 "할라사나"). */
export function searchText(term: DictionaryTerm) {
  return [term.sanskrit, term.roman ?? '', ...shownSpellings(term), term.meaning].join(' ').toLocaleLowerCase('ko-KR').normalize('NFD').replace(/[̀-ͯ]/g, '').replace(/\s+/g, '')
}

export function normalizeQuery(text: string) {
  return text.trim().toLocaleLowerCase('ko-KR').normalize('NFD').replace(/[̀-ͯ]/g, '').replace(/\s+/g, '')
}
