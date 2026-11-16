// 모델 목록에서 계열마다 가장 최신 것 하나만 고른다(김성균 개발자: "각 종류당 최신 숫자 모델 하나씩만").
// 백엔드가 준 목록 안에서만 고른다(계정마다 쓸 수 있는 모델이 다르다). 백엔드가 계열마다 하나만 주면 그대로 나간다.
//
// 고르는 차례: 판 번호가 높은 것(5.2 > 5.1 > 5 > 4.8, 10 > 9) → 정식(미리보기·베타가 아닌 것) → 날짜가 늦은 것
//   → 별칭이 아닌 것(id 가 가리키는 실제 모델과 같은 것) → 목록에서 먼저 나온 것.
// 계열(family)이 비어 오면 실제 모델 이름(resolved), 화면 이름(label), id 차례로 읽어 계열을 찾는다.
import type { AiModel } from './ai-models'

type Parsed = {
  family: string | null
  version: number[]
  date: number
  preview: boolean
  alias: boolean
}

// 계열이 아닌 낱말(회사 이름, 별칭 이름, 꾸밈말).
const NOT_FAMILY = new Set(['gpt', 'claude', 'gemini', 'default', 'recommended', 'latest', 'preview', 'beta', 'experimental', 'exp', 'chatgpt', 'openai', 'model'])
const PREVIEW = /\b(preview|beta|experimental|exp|alpha|rc)\b/i

function tokens(text: string) {
  return text.toLowerCase().split(/[^a-z0-9.]+/).filter(Boolean)
}

/** 글에서 계열, 판 번호, 날짜를 읽는다. 예: "claude-opus-4-8" → opus, [4,8] / "GPT-6.1-Sol" → sol, [6,1]. */
export function parseModelText(text: string): { family: string | null; version: number[]; date: number } {
  let family: string | null = null
  const numbers: number[] = []
  let date = 0
  for (const token of tokens(text)) {
    if (/^\d{8}$/.test(token)) { date = Number(token); continue }
    if (/^\d+(\.\d+)*$/.test(token)) { numbers.push(...token.split('.').map(Number)); continue }
    // "4.5x" 같은 섞인 낱말은 숫자 부분만 판 번호로.
    const mixed = /^(\d+(?:\.\d+)*)[a-z]+$/.exec(token)
    if (mixed) { numbers.push(...mixed[1].split('.').map(Number)); continue }
    if (!family && /^[a-z]+$/.test(token) && !NOT_FAMILY.has(token)) family = token
  }
  return { family, version: numbers, date }
}

function parse(model: AiModel): Parsed {
  const source = [model.resolved, model.label, model.id].filter((value): value is string => Boolean(value && value.trim()))
  let family = model.family?.trim().toLowerCase() || null
  let version: number[] = []
  let date = 0
  for (const text of source) {
    const read = parseModelText(text)
    if (!family && read.family) family = read.family
    if (version.length === 0 && read.version.length > 0) version = read.version
    if (!date && read.date) date = read.date
  }
  const preview = source.some((text) => PREVIEW.test(text))
  const alias = Boolean(model.resolved && model.resolved !== model.id)
  return { family, version, date, preview, alias }
}

function compareVersion(a: number[], b: number[]) {
  const length = Math.max(a.length, b.length)
  for (let i = 0; i < length; i += 1) {
    const x = a[i] ?? 0
    const y = b[i] ?? 0
    if (x !== y) return x - y
  }
  return 0
}

/** a 가 b 보다 나으면 양수. */
function better(a: Parsed, b: Parsed) {
  const byVersion = compareVersion(a.version, b.version)
  if (byVersion !== 0) return byVersion
  if (a.preview !== b.preview) return a.preview ? -1 : 1
  if (a.date !== b.date) return a.date - b.date
  if (a.alias !== b.alias) return a.alias ? -1 : 1
  return 0
}

/**
 * 계열마다 가장 최신 것 하나. 계열은 목록에 처음 나온 차례대로. 계열 안에 기본 모델(isDefault)이 있었으면
 * 고른 것이 그 회사의 기본 모델이 된다. 계열을 읽을 수 없는 모델은 그대로 하나씩 둔다.
 */
export function latestPerFamily(models: AiModel[]): AiModel[] {
  const order: string[] = []
  const best = new Map<string, { model: AiModel; parsed: Parsed; index: number; hasDefault: boolean }>()
  models.forEach((model, index) => {
    const parsed = parse(model)
    const key = parsed.family ?? `#${model.id}`
    const current = best.get(key)
    if (!current) {
      order.push(key)
      best.set(key, { model, parsed, index, hasDefault: Boolean(model.isDefault) })
      return
    }
    const hasDefault = current.hasDefault || Boolean(model.isDefault)
    if (better(parsed, current.parsed) > 0) best.set(key, { model, parsed, index, hasDefault })
    else current.hasDefault = hasDefault
  })
  return order.map((key) => {
    const { model, hasDefault } = best.get(key)!
    return { ...model, isDefault: hasDefault }
  })
}
