import { W } from './words'
import { strFromU8, unzipSync, type UnzipFileInfo } from 'fflate'
import type { SummarySection, TranscriptLine, WorkspaceRecord } from './workbench-data'

export type AiProvider = 'codex' | 'claude-code' | 'chatgpt' | 'claude'
export type AiMessageRole = 'user' | 'assistant'

export type AiConversationMessage = {
  role: AiMessageRole
  text: string
  timestamp?: string
}

export type AiConversationPreview = {
  id: string
  provider: AiProvider
  title: string
  createdAt?: string
  updatedAt?: string
  projectLabel?: string
  messageCount: number
  sourceName: string
  sourceLocator?: string
  sampleMessages: AiConversationMessage[]
}

export type AiConversation = Omit<AiConversationPreview, 'sampleMessages'> & {
  messages: AiConversationMessage[]
}

export const AI_PROVIDER_INFO: Record<AiProvider, {
  label: string
  shortLabel: string
  localDiscovery: boolean
  fileHint: string
}> = {
  codex: {
    label: 'Codex',
    shortLabel: 'Codex',
    localDiscovery: true,
    fileHint: W.aiConversationImport.text1,
  },
  'claude-code': {
    label: 'Claude Code',
    shortLabel: 'Claude',
    localDiscovery: true,
    fileHint: W.aiConversationImport.text2,
  },
  chatgpt: {
    label: 'ChatGPT',
    shortLabel: 'ChatGPT',
    localDiscovery: false,
    fileHint: W.aiConversationImport.text3,
  },
  claude: {
    label: 'Claude',
    shortLabel: 'Claude',
    localDiscovery: false,
    fileHint: W.aiConversationImport.text4,
  },
}

type JsonObject = Record<string, unknown>

function isObject(value: unknown): value is JsonObject {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function readString(object: JsonObject, ...keys: string[]) {
  for (const key of keys) {
    if (typeof object[key] === 'string' && object[key]) return object[key] as string
  }
  return undefined
}

function normalizeTimestamp(value: unknown) {
  if (typeof value === 'number' && Number.isFinite(value)) {
    const milliseconds = value > 10_000_000_000 ? value : value * 1000
    return new Date(milliseconds).toISOString()
  }
  if (typeof value !== 'string' || !value.trim()) return undefined
  const parsed = new Date(value)
  return Number.isNaN(parsed.getTime()) ? value : parsed.toISOString()
}

function extractText(content: unknown): string {
  if (typeof content === 'string') return content.trim()
  if (Array.isArray(content)) {
    return content.map(extractText).filter(Boolean).join('\n\n').trim()
  }
  if (!isObject(content)) return ''

  if (Array.isArray(content.parts)) return extractText(content.parts)
  const contentType = typeof content.type === 'string' ? content.type : undefined
  if (contentType && !['text', 'input_text', 'output_text'].includes(contentType)) return ''
  if (typeof content.text === 'string') return content.text.trim()
  if (content.content !== undefined) return extractText(content.content)
  return ''
}

function normalizeRole(value: unknown): AiMessageRole | null {
  if (value === 'user' || value === 'human') return 'user'
  if (value === 'assistant' || value === 'ai' || value === 'model') return 'assistant'
  return null
}

function removeCodexRuntimeContext(text: string) {
  return text
    .replace(/^# AGENTS\.md instructions[\s\S]*?<\/INSTRUCTIONS>/i, '')
    .replace(/<(environment_context|permissions instructions|app-context|skills_instructions|apps_instructions|plugins_instructions)>[\s\S]*?<\/\1>/gi, '')
    .trim()
}

function cleanMessages(messages: AiConversationMessage[]) {
  return messages
    .map((message) => ({
      ...message,
      text: message.role === 'user' ? removeCodexRuntimeContext(message.text) : message.text.trim(),
    }))
    .filter((message) => message.text.length > 0)
}

function compactTitle(value: string | undefined, messages: AiConversationMessage[], fallback: string) {
  const firstUserMessage = messages.find((message) => message.role === 'user')?.text
  const title = (value || firstUserMessage || fallback).replace(/\s+/g, ' ').trim()
  return title.length > 72 ? `${title.slice(0, 69)}...` : title
}

function fallbackId(sourceName: string, index: number) {
  return `${sourceName}-${index + 1}`.replace(/[^a-zA-Z0-9가-힣._-]+/g, '-').slice(0, 120)
}

function asPreview(conversation: AiConversation): AiConversationPreview {
  return {
    ...conversation,
    sampleMessages: conversation.messages.slice(0, 5),
  }
}

function parseChatGptMessage(value: unknown): AiConversationMessage | null {
  if (!isObject(value)) return null
  const message = isObject(value.message) ? value.message : value
  const author = isObject(message.author) ? message.author : null
  const role = normalizeRole(author?.role ?? message.role)
  if (!role) return null
  const text = extractText(message.content ?? message.text)
  if (!text) return null
  return {
    role,
    text,
    timestamp: normalizeTimestamp(message.create_time ?? value.create_time),
  }
}

function orderedChatGptNodes(conversation: JsonObject) {
  const mapping = isObject(conversation.mapping) ? conversation.mapping : null
  if (!mapping) return []

  const currentNode = readString(conversation, 'current_node', 'currentNode')
  if (currentNode && mapping[currentNode]) {
    const chain: unknown[] = []
    const visited = new Set<string>()
    let nodeId: string | undefined = currentNode
    while (nodeId && !visited.has(nodeId)) {
      visited.add(nodeId)
      const node: unknown = mapping[nodeId]
      if (!isObject(node)) break
      chain.unshift(node)
      nodeId = typeof node.parent === 'string' ? node.parent : undefined
    }
    return chain
  }

  return Object.values(mapping).sort((left, right) => {
    const leftObject = isObject(left) && isObject(left.message) ? left.message : null
    const rightObject = isObject(right) && isObject(right.message) ? right.message : null
    const leftTime = typeof leftObject?.create_time === 'number' ? leftObject.create_time : 0
    const rightTime = typeof rightObject?.create_time === 'number' ? rightObject.create_time : 0
    return leftTime - rightTime
  })
}

function parseChatGptConversations(values: unknown[], sourceName: string) {
  return values.flatMap((value, index): AiConversation[] => {
    if (!isObject(value)) return []
    const messages = cleanMessages(orderedChatGptNodes(value).map(parseChatGptMessage).filter((item): item is AiConversationMessage => Boolean(item)))
    if (messages.length === 0) return []
    const id = readString(value, 'conversation_id', 'conversationId', 'id') ?? fallbackId(sourceName, index)
    return [{
      id,
      provider: 'chatgpt',
      title: compactTitle(readString(value, 'title', 'name'), messages, W.aiConversationImport.text5),
      createdAt: normalizeTimestamp(value.create_time ?? value.created_at),
      updatedAt: normalizeTimestamp(value.update_time ?? value.updated_at),
      messageCount: messages.length,
      sourceName,
      messages,
    }]
  })
}

function parseClaudeExportMessage(value: unknown): AiConversationMessage | null {
  if (!isObject(value)) return null
  const role = normalizeRole(value.role ?? value.sender ?? value.author)
  if (!role) return null
  const text = extractText(value.content ?? value.text ?? value.message)
  if (!text) return null
  return {
    role,
    text,
    timestamp: normalizeTimestamp(value.created_at ?? value.timestamp ?? value.create_time),
  }
}

function parseClaudeExport(values: unknown[], sourceName: string, provider: AiProvider = 'claude') {
  return values.flatMap((value, index): AiConversation[] => {
    if (!isObject(value)) return []
    const rawMessages = Array.isArray(value.chat_messages)
      ? value.chat_messages
      : Array.isArray(value.messages)
        ? value.messages
        : []
    const messages = cleanMessages(rawMessages.map(parseClaudeExportMessage).filter((item): item is AiConversationMessage => Boolean(item)))
    if (messages.length === 0) return []
    const id = readString(value, 'uuid', 'id', 'conversation_id', 'conversationId') ?? fallbackId(sourceName, index)
    return [{
      id,
      provider,
      title: compactTitle(readString(value, 'name', 'title', 'summary'), messages, W.aiConversationImport.text6(AI_PROVIDER_INFO[provider].label)),
      createdAt: normalizeTimestamp(value.created_at ?? value.create_time),
      updatedAt: normalizeTimestamp(value.updated_at ?? value.update_time),
      projectLabel: readString(value, 'project_name', 'projectName'),
      messageCount: messages.length,
      sourceName,
      messages,
    }]
  })
}

function parseCodexRows(rows: unknown[], sourceName: string): AiConversation[] {
  const objects = rows.filter(isObject)
  const sessionRow = objects.find((row) => row.type === 'session_meta')
  const payload = sessionRow && isObject(sessionRow.payload) ? sessionRow.payload : null
  const messages = cleanMessages(objects.flatMap((row): AiConversationMessage[] => {
    if (row.type !== 'response_item' || !isObject(row.payload) || row.payload.type !== 'message') return []
    const role = normalizeRole(row.payload.role)
    if (!role) return []
    const text = extractText(row.payload.content)
    if (!text) return []
    return [{ role, text, timestamp: normalizeTimestamp(row.timestamp) }]
  }))
  if (messages.length === 0) return []

  const id = payload ? readString(payload, 'id', 'session_id') : undefined
  const cwd = payload ? readString(payload, 'cwd') : undefined
  return [{
    id: id ?? fallbackId(sourceName, 0),
    provider: 'codex',
    title: compactTitle(undefined, messages, W.aiConversationImport.text7),
    createdAt: normalizeTimestamp(payload?.timestamp ?? sessionRow?.timestamp),
    updatedAt: normalizeTimestamp(objects[objects.length - 1]?.timestamp),
    projectLabel: (() => {
      const parts = cwd?.split(/[\\/]/).filter(Boolean) ?? []
      return parts[parts.length - 1]
    })(),
    messageCount: messages.length,
    sourceName,
    messages,
  }]
}

function parseClaudeCodeRows(rows: unknown[], sourceName: string): AiConversation[] {
  const objects = rows.filter(isObject)
  const customTitleRow = [...objects].reverse().find((row) => row.type === 'custom-title')
  const firstMessageRow = objects.find((row) => isObject(row.message))
  const messages = cleanMessages(objects.flatMap((row): AiConversationMessage[] => {
    if (!isObject(row.message) || row.isMeta === true) return []
    const role = normalizeRole(row.message.role ?? row.type)
    if (!role) return []
    const text = extractText(row.message.content)
    if (!text) return []
    return [{ role, text, timestamp: normalizeTimestamp(row.timestamp) }]
  }))
  if (messages.length === 0) return []

  const id = readString(firstMessageRow ?? {}, 'sessionId') ?? fallbackId(sourceName, 0)
  const cwd = readString(firstMessageRow ?? {}, 'cwd')
  return [{
    id,
    provider: 'claude-code',
    title: compactTitle(readString(customTitleRow ?? {}, 'customTitle'), messages, W.aiConversationImport.text8),
    createdAt: normalizeTimestamp(firstMessageRow?.timestamp),
    updatedAt: normalizeTimestamp(objects[objects.length - 1]?.timestamp),
    projectLabel: (() => {
      const parts = cwd?.split(/[\\/]/).filter(Boolean) ?? []
      return parts[parts.length - 1]
    })(),
    messageCount: messages.length,
    sourceName,
    messages,
  }]
}

function parseTextConversation(text: string, sourceName: string, provider: AiProvider) {
  const lines = text.split(/\r?\n/)
  const messages: AiConversationMessage[] = []
  let active: AiConversationMessage | null = null

  for (const line of lines) {
    const match = line.match(/^\s*(user|human|사용자|나|assistant|claude|chatgpt|codex|ai)\s*:\s*(.*)$/i)
    if (match) {
      if (active?.text.trim()) messages.push({ ...active, text: active.text.trim() })
      active = {
        role: normalizeRole(match[1].toLocaleLowerCase()) ?? (match[1] === '나' || match[1] === '사용자' ? 'user' : 'assistant'),
        text: match[2],
      }
    } else if (active) {
      active.text += `\n${line}`
    }
  }
  if (active?.text.trim()) messages.push({ ...active, text: active.text.trim() })
  if (messages.length === 0 && text.trim()) messages.push({ role: 'user', text: text.trim() })
  if (messages.length === 0) return []

  const clean = cleanMessages(messages)
  return [{
    id: fallbackId(sourceName, 0),
    provider,
    title: compactTitle(undefined, clean, W.aiConversationImport.text6(AI_PROVIDER_INFO[provider].label)),
    messageCount: clean.length,
    sourceName,
    messages: clean,
  } satisfies AiConversation]
}

function parseJsonValue(value: unknown, sourceName: string, providerHint: AiProvider) {
  const root = isObject(value) && Array.isArray(value.conversations) ? value.conversations : value
  const values = Array.isArray(root) ? root : [root]
  if (values.some((item) => isObject(item) && isObject(item.mapping))) {
    return parseChatGptConversations(values, sourceName)
  }
  if (values.some((item) => isObject(item) && (Array.isArray(item.chat_messages) || Array.isArray(item.messages)))) {
    return parseClaudeExport(values, sourceName, providerHint === 'codex' || providerHint === 'chatgpt' ? 'claude' : providerHint)
  }
  return []
}

function parseDocument(text: string, sourceName: string, providerHint: AiProvider) {
  const trimmed = text.trim()
  if (!trimmed) return []
  const jsonLinesDocument = /\.jsonl(?:$|\s|\/)/i.test(sourceName)

  try {
    const parsed = JSON.parse(trimmed) as unknown
    const conversations = parseJsonValue(parsed, sourceName, providerHint)
    if (conversations.length > 0) return conversations
    if (!jsonLinesDocument) return []
  } catch {
    // JSONL and text exports are handled below.
  }

  const rows = trimmed.split(/\r?\n/).flatMap((line): unknown[] => {
    try {
      return [JSON.parse(line) as unknown]
    } catch {
      return []
    }
  })
  if (rows.length > 0) {
    const objects = rows.filter(isObject)
    if (objects.some((row) => row.type === 'session_meta' || row.type === 'response_item')) {
      return parseCodexRows(rows, sourceName)
    }
    if (objects.some((row) => row.type === 'custom-title' || (row.type === 'user' && isObject(row.message)))) {
      return parseClaudeCodeRows(rows, sourceName)
    }
  }

  return parseTextConversation(trimmed, sourceName, providerHint)
}

const MAX_ARCHIVE_ENTRIES = 200
const MAX_ARCHIVE_TOTAL_ENTRIES = 5_000
const MAX_ARCHIVE_ENTRY_BYTES = 128 * 1024 * 1024
const MAX_ARCHIVE_EXTRACTED_BYTES = 256 * 1024 * 1024

function isConversationArchiveEntry(name: string) {
  return /(^|\/)(conversations?|chat[_ -]?history|chats?)[^/]*\.json$/i.test(name)
    || /\.(jsonl|md|txt)$/i.test(name)
}

function createArchiveFilter(archiveName: string) {
  let totalEntryCount = 0
  let entryCount = 0
  let extractedBytes = 0
  return (entry: UnzipFileInfo) => {
    totalEntryCount += 1
    if (totalEntryCount > MAX_ARCHIVE_TOTAL_ENTRIES) {
      throw new Error(W.aiConversationImport.text9(archiveName))
    }
    if (!isConversationArchiveEntry(entry.name)) return false
    entryCount += 1
    extractedBytes += entry.originalSize
    if (entryCount > MAX_ARCHIVE_ENTRIES) {
      throw new Error(W.aiConversationImport.text10(archiveName))
    }
    if (entry.originalSize > MAX_ARCHIVE_ENTRY_BYTES) {
      throw new Error(W.aiConversationImport.text11(archiveName, entry.name))
    }
    if (extractedBytes > MAX_ARCHIVE_EXTRACTED_BYTES) {
      throw new Error(W.aiConversationImport.text12(archiveName))
    }
    return true
  }
}

async function parseArchive(file: File, providerHint: AiProvider) {
  if (file.size > 500 * 1024 * 1024) throw new Error(W.aiConversationImport.text13(file.name))
  const archive = unzipSync(new Uint8Array(await file.arrayBuffer()), {
    filter: createArchiveFilter(file.name),
  })
  const archiveEntries = Object.entries(archive)
  const namedConversationEntries = archiveEntries.filter(([name]) => (
    isConversationArchiveEntry(name) && name.toLocaleLowerCase().endsWith('.json')
  ))
  const entries = (namedConversationEntries.length > 0
    ? namedConversationEntries
    : archiveEntries.filter(([name]) => /\.(jsonl|md|txt)$/i.test(name)))
  if (entries.length === 0) throw new Error(W.aiConversationImport.text14(file.name))
  return entries.flatMap(([name, bytes]) => parseDocument(strFromU8(bytes), `${file.name} / ${name}`, providerHint))
}

export async function parseAiConversationFiles(files: File[], providerHint: AiProvider) {
  const conversations: AiConversation[] = []
  for (const file of files) {
    const parsed = file.name.toLocaleLowerCase().endsWith('.zip')
      ? await parseArchive(file, providerHint)
      : parseDocument(await file.text(), file.name, providerHint)
    conversations.push(...parsed)
  }

  const unique = new Map<string, AiConversation>()
  for (const conversation of conversations) {
    const key = `${conversation.provider}:${conversation.id}`
    if (!unique.has(key)) unique.set(key, conversation)
  }
  if (unique.size === 0) throw new Error(W.aiConversationImport.text15)
  return [...unique.values()]
}

export function getConversationKey(conversation: Pick<AiConversationPreview, 'provider' | 'id'>) {
  return `${conversation.provider}:${conversation.id}`
}

export function previewConversation(conversation: AiConversation) {
  return asPreview(conversation)
}

function formatRecordDate(value: string | undefined) {
  const date = value ? new Date(value) : new Date()
  const safeDate = Number.isNaN(date.getTime()) ? new Date() : date
  const today = new Date()
  const yesterday = new Date(today)
  yesterday.setDate(today.getDate() - 1)
  const sameDay = (left: Date, right: Date) => left.toDateString() === right.toDateString()
  const dateLabel = sameDay(safeDate, today)
    ? W.aiConversationImport.text16
    : sameDay(safeDate, yesterday)
      ? W.aiConversationImport.text17
      : new Intl.DateTimeFormat('ko-KR', { month: '2-digit', day: '2-digit' }).format(safeDate)
  const time = new Intl.DateTimeFormat('ko-KR', { hour: '2-digit', minute: '2-digit', hour12: false }).format(safeDate)
  const createdAt = new Intl.DateTimeFormat('ko-KR', { dateStyle: 'long', timeStyle: 'short' }).format(safeDate)
  return { dateLabel, time, createdAt }
}

function messageTime(message: AiConversationMessage, index: number) {
  if (!message.timestamp) return `#${index + 1}`
  const date = new Date(message.timestamp)
  if (Number.isNaN(date.getTime())) return `#${index + 1}`
  return new Intl.DateTimeFormat('ko-KR', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  }).format(date)
}

async function checksumForConversation(conversation: AiConversation) {
  const bytes = new TextEncoder().encode(JSON.stringify(conversation.messages))
  const digest = await crypto.subtle.digest('SHA-256', bytes)
  const hash = Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('')
  return `SHA-256 · ${hash.slice(0, 8)}…${hash.slice(-8)}`
}

export async function conversationToWorkspaceRecord(conversation: AiConversation): Promise<WorkspaceRecord> {
  const provider = AI_PROVIDER_INFO[conversation.provider]
  const firstUserIndex = conversation.messages.findIndex((message) => message.role === 'user')
  const firstUserMessage = firstUserIndex >= 0 ? conversation.messages[firstUserIndex] : conversation.messages[0]
  const sourceRange = conversation.messages.length > 1 ? W.aiConversationImport.text18(conversation.messages.length) : W.aiConversationImport.text19
  const summarySections: SummarySection[] = [
    {
      title: W.aiConversationImport.text20,
      body: firstUserMessage.text,
      source: W.aiConversationImport.text21(Math.max(firstUserIndex, 0) + 1),
    },
    {
      title: W.aiConversationImport.text22,
      body: W.aiConversationImport.text23(provider.label, conversation.messages.length),
      source: sourceRange,
    },
  ]
  const transcript: TranscriptLine[] = conversation.messages.map((message, index) => ({
    time: messageTime(message, index),
    speaker: message.role === 'user' ? W.aiConversationImport.text24 : provider.shortLabel,
    text: message.text,
  }))
  const date = formatRecordDate(conversation.createdAt ?? conversation.updatedAt)
  const safeId = conversation.id.replace(/[^a-zA-Z0-9_-]+/g, '-').slice(0, 72)

  return {
    id: `ai-${conversation.provider}-${safeId}`,
    title: conversation.title,
    preview: firstUserMessage.text.replace(/\s+/g, ' ').slice(0, 90),
    dateLabel: date.dateLabel,
    time: date.time,
    duration: W.aiConversationImport.text25(conversation.messages.length),
    sourceType: 'ai-conversation',
    sourceLabel: W.aiConversationImport.text26(provider.label),
    participantLabel: W.aiConversationImport.text27(provider.shortLabel),
    status: 'needs-review',
    summarySections,
    transcript,
    createdAt: date.createdAt,
    archiveId: `momso-local / ai-${conversation.provider}-${safeId}`,
    checksum: await checksumForConversation(conversation),
    aiNote: W.aiConversationImport.text28(conversation.messages.length),
    originalFileName: conversation.sourceName,
    retentionLabel: W.aiConversationImport.text29,
    externalProvider: conversation.provider,
    externalConversationId: conversation.id,
    projectLabel: conversation.projectLabel,
  }
}
