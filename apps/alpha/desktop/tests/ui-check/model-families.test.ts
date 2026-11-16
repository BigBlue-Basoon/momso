// 계열마다 가장 최신 모델 하나 고르기(src/model-families.ts)의 단위 시험.
// 실행: npx tsx --test tests/ui-check/model-families.test.ts
import assert from 'node:assert/strict'
import test from 'node:test'
import { latestPerFamily, parseModelText } from '../../src/model-families'

const claudeList = [
  { id: 'default', family: null, label: 'Default (recommended)', resolved: 'claude-fable-5-2', isDefault: true },
  { id: 'opus', family: 'Opus', label: 'Opus 5.5', resolved: 'claude-opus-5-5' },
  { id: 'claude-fable-5-2', family: 'Fable', label: 'Fable 5.2', resolved: 'claude-fable-5-2' },
  { id: 'sonnet', family: 'Sonnet', label: 'Sonnet 5.5', resolved: 'claude-sonnet-5-5' },
  { id: 'haiku', family: 'Haiku', label: 'Haiku 5.5', resolved: 'claude-haiku-5-5' },
  { id: 'fable', family: 'Fable', label: 'Fable 5.1', resolved: 'claude-fable-5-1' },
  { id: 'claude-haiku-4-5-20251001', family: 'Haiku', label: 'Haiku 4.5', resolved: 'claude-haiku-4-5-20251001' },
  { id: 'claude-sonnet-5', family: 'Sonnet', label: 'Sonnet 5', resolved: 'claude-sonnet-5' },
  { id: 'claude-opus-5', family: 'Opus', label: 'Opus 5', resolved: 'claude-opus-5' },
  { id: 'claude-fable-5', family: 'Fable', label: 'Fable 5', resolved: 'claude-fable-5' },
  { id: 'claude-opus-4-8', family: 'Opus', label: 'Opus 4.8', resolved: 'claude-opus-4-8' },
  { id: 'claude-sonnet-4-6', family: 'Sonnet', label: 'Sonnet 4.6', resolved: 'claude-sonnet-4-6' },
]

const codexList = [
  { id: 'gpt-6.1-sol', family: null, label: 'GPT-6.1-Sol', resolved: null, isDefault: true },
  { id: 'gpt-6-astra', family: null, label: 'GPT-6-Astra', resolved: null },
  { id: 'gpt-6.1-terra', family: null, label: 'GPT-6.1 Terra', resolved: null },
  { id: 'gpt-6-sol', family: null, label: 'GPT-6-Sol', resolved: null },
  { id: 'gpt-6-luna', family: null, label: 'GPT-6-Luna', resolved: null },
  { id: 'gpt-5.6-sol', family: null, label: 'GPT-5.6-Sol', resolved: null },
  { id: 'gpt-5.6-terra', family: null, label: 'GPT-5.6-Terra', resolved: null },
  { id: 'gpt-5.6-luna', family: null, label: 'GPT-5.6-Luna', resolved: null },
]

test('Claude: 계열마다 가장 최신 하나(Fable 5.2, Opus 5.5, Sonnet 5.5, Haiku 5.5)', () => {
  const picked = latestPerFamily(claudeList)
  assert.deepEqual(picked.map((m) => m.label), ['Fable 5.2', 'Opus 5.5', 'Sonnet 5.5', 'Haiku 5.5'])
  // 기본(별칭 default)이 가리키던 Fable 계열의 고른 것이 기본이 된다. 별칭보다 실제 id 가 골라진다.
  assert.equal(picked[0].id, 'claude-fable-5-2')
  assert.equal(picked[0].isDefault, true)
  assert.equal(picked.filter((m) => m.isDefault).length, 1)
})

test('Codex: 계열 이름이 비어 와도 화면 이름에서 읽는다', () => {
  const picked = latestPerFamily(codexList)
  assert.deepEqual(picked.map((m) => m.id), ['gpt-6.1-sol', 'gpt-6-astra', 'gpt-6.1-terra', 'gpt-6-luna'])
  assert.equal(picked[0].isDefault, true)
})

test('판 번호 비교: 5.2 > 5.1 > 5 > 4.8, 10 > 9', () => {
  const list = ['x-4.8', 'x-5', 'x-5.1', 'x-5.2'].map((id) => ({ id, family: 'X', label: id, resolved: null }))
  assert.equal(latestPerFamily(list)[0].id, 'x-5.2')
  const tens = ['m-9', 'm-10'].map((id) => ({ id, family: 'M', label: id, resolved: null }))
  assert.equal(latestPerFamily(tens)[0].id, 'm-10')
})

test('판 번호가 같으면 정식 → 늦은 날짜', () => {
  const list = [
    { id: 'pro-2-preview', family: 'Pro', label: 'Pro 2 Preview', resolved: null },
    { id: 'pro-2-20250101', family: 'Pro', label: 'Pro 2', resolved: null },
    { id: 'pro-2-20250601', family: 'Pro', label: 'Pro 2', resolved: null },
  ]
  assert.equal(latestPerFamily(list)[0].id, 'pro-2-20250601')
})

test('계열마다 하나만 오면 그대로', () => {
  const list = [
    { id: 'a', family: 'Fable', label: 'Fable 5.2', resolved: null },
    { id: 'b', family: 'Opus', label: 'Opus 5.5', resolved: null },
  ]
  assert.deepEqual(latestPerFamily(list).map((m) => m.id), ['a', 'b'])
})

test('글 읽기', () => {
  assert.deepEqual(parseModelText('claude-opus-4-8'), { family: 'opus', version: [4, 8], date: 0 })
  assert.deepEqual(parseModelText('claude-haiku-4-5-20251001'), { family: 'haiku', version: [4, 5], date: 20251001 })
  assert.deepEqual(parseModelText('GPT-6.1 Terra'), { family: 'terra', version: [6, 1], date: 0 })
})
