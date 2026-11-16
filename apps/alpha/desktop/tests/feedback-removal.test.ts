import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

test('AI panel and settings have no response rating controls or delivery notice', () => {
  const panel = readFileSync(fileURLToPath(new URL('../src/components/AssistantPanel.tsx', import.meta.url)), 'utf8')
  const settings = readFileSync(fileURLToPath(new URL('../src/components/SettingsView.tsx', import.meta.url)), 'utf8')
  assert.doesNotMatch(panel, /onFeedback|ThumbsUp|ThumbsDown|응답 평가/)
  assert.doesNotMatch(settings, /피드백 수신함|좋아요·싫어요/)
})
