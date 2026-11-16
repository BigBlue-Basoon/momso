import assert from 'node:assert/strict'
import test from 'node:test'
import { strToU8, zipSync } from 'fflate'
import {
  conversationToWorkspaceRecord,
  parseAiConversationFiles,
} from '../src/ai-conversation-import'

function chatGptExport() {
  return [{
    id: 'chatgpt-1',
    title: '요가원 운영 구조',
    create_time: 1_784_000_000,
    current_node: 'assistant-1',
    mapping: {
      root: { id: 'root', parent: null, message: null },
      system: {
        id: 'system',
        parent: 'root',
        message: { author: { role: 'system' }, content: { parts: ['숨은 시스템 지시'] } },
      },
      'user-1': {
        id: 'user-1',
        parent: 'system',
        message: {
          author: { role: 'user' },
          create_time: 1_784_000_001,
          content: { parts: ['회원 기록을 어떻게 보존해야 해?'] },
        },
      },
      'assistant-1': {
        id: 'assistant-1',
        parent: 'user-1',
        message: {
          author: { role: 'assistant' },
          create_time: 1_784_000_002,
          content: { parts: ['원문과 검수본을 분리해 보존하세요.'] },
        },
      },
    },
  }]
}

test('ChatGPT conversations.json에서 사용자와 AI 원문만 순서대로 가져온다', async () => {
  const file = new File([JSON.stringify(chatGptExport())], 'conversations.json', { type: 'application/json' })
  const conversations = await parseAiConversationFiles([file], 'chatgpt')

  assert.equal(conversations.length, 1)
  assert.equal(conversations[0].provider, 'chatgpt')
  assert.equal(conversations[0].title, '요가원 운영 구조')
  assert.deepEqual(conversations[0].messages.map(({ role, text }) => ({ role, text })), [
    { role: 'user', text: '회원 기록을 어떻게 보존해야 해?' },
    { role: 'assistant', text: '원문과 검수본을 분리해 보존하세요.' },
  ])
})

test('Claude Code JSONL의 제목, 프로젝트, 텍스트 블록을 보존한다', async () => {
  const jsonl = [
    JSON.stringify({ type: 'custom-title', customTitle: '로컬 우선 설계', sessionId: 'claude-1' }),
    JSON.stringify({
      type: 'user',
      sessionId: 'claude-1',
      cwd: 'C:\\work\\momso',
      timestamp: '2026-07-14T01:00:00Z',
      message: { role: 'user', content: '데이터를 기기에만 둘 수 있을까?' },
    }),
    JSON.stringify({
      type: 'assistant',
      sessionId: 'claude-1',
      timestamp: '2026-07-14T01:01:00Z',
      message: {
        role: 'assistant',
        content: [
          { type: 'text', text: '가능합니다.' },
          { type: 'tool_use', name: 'ignored' },
        ],
      },
    }),
  ].join('\n')
  const conversations = await parseAiConversationFiles([
    new File([jsonl], 'claude-session.jsonl', { type: 'application/x-ndjson' }),
  ], 'claude-code')

  assert.equal(conversations[0].provider, 'claude-code')
  assert.equal(conversations[0].title, '로컬 우선 설계')
  assert.equal(conversations[0].projectLabel, 'momso')
  assert.equal(conversations[0].messages[1].text, '가능합니다.')
})

test('ChatGPT 내보내기 ZIP을 읽고 momso 원문 기록으로 변환한다', async () => {
  const archive = zipSync({
    'export/conversations.json': strToU8(JSON.stringify(chatGptExport())),
    'export/user.json': strToU8(JSON.stringify({ email: 'private@example.com' })),
  })
  const file = new File([archive], 'chatgpt-export.zip', { type: 'application/zip' })
  const [conversation] = await parseAiConversationFiles([file], 'chatgpt')
  const record = await conversationToWorkspaceRecord(conversation)

  assert.equal(record.sourceType, 'ai-conversation')
  assert.equal(record.externalProvider, 'chatgpt')
  assert.equal(record.externalConversationId, 'chatgpt-1')
  assert.equal(record.transcript.length, 2)
  assert.equal(record.originalFileName, 'chatgpt-export.zip / export/conversations.json')
  assert.equal(record.transcript[0].speaker, '나')
  assert.equal(record.transcript[1].speaker, 'ChatGPT')
  assert.match(record.checksum, /^SHA-256/)
})

test('ZIP 안의 대화 파일 수가 안전 한도를 넘으면 압축 해제 전에 거절한다', async () => {
  const entries = Object.fromEntries(Array.from({ length: 201 }, (_, index) => [
    `history-${index}.txt`,
    strToU8(`user: message ${index}\nassistant: reply ${index}`),
  ]))
  const archive = zipSync(entries)
  const file = new File([archive], 'oversized-export.zip', { type: 'application/zip' })

  await assert.rejects(
    () => parseAiConversationFiles([file], 'chatgpt'),
    /200개를 넘어/,
  )
})

test('지원하지 않는 파일도 지나치게 많은 ZIP은 중앙 목록 단계에서 거절한다', async () => {
  const entries = Object.fromEntries(Array.from({ length: 5_001 }, (_, index) => [
    `assets/image-${index}.bin`,
    new Uint8Array(),
  ]))
  const archive = zipSync(entries)
  const file = new File([archive], 'entry-flood.zip', { type: 'application/zip' })

  await assert.rejects(
    () => parseAiConversationFiles([file], 'chatgpt'),
    /전체 항목이 5,000개를 넘습니다/,
  )
})
