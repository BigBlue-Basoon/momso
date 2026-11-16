import { W } from './words'
import type { ChosenModel } from './ai-models'
import type { AssistantMessage, WorkspaceView } from './workbench-data'

export type SourceRetention = 'keep' | 'after-review' | 'ask'
export type ResponseDensity = 'concise' | 'balanced' | 'detailed'
export type AssistantProvider = 'codex' | 'claude'
export type AssistantAuthMode = 'subscription' | 'api_key'

export type AppPreferences = {
  workspaceName: string
  startupView: WorkspaceView
  defaultParticipantLabel: string
  reviewerName: string
  confirmRecordingConsent: boolean
  sourceRetention: SourceRetention
  moveReviewedToArchive: boolean
  autoAttachContext: boolean
  persistAssistantHistory: boolean
  responseDensity: ResponseDensity
  assistantProvider: AssistantProvider
  assistantAuthMode: AssistantAuthMode
  /** 입력 칸 아래에서 고른 모델. null 이면 설정의 AI 곳(assistantProvider)의 기본 모델. */
  chatModel: ChosenModel | null
}

const PREFERENCES_KEY = 'momso.desktop.preferences.v1'
const CHAT_MODELS_KEY = 'momso.desktop.chat-models.v1'

/** 대화마다 고른 모델과 추론 수준. 키는 대화 번호, 저장 전인 새 대화는 'new'. */
export function loadChatModels(): Record<string, ChosenModel> {
  try {
    const stored = window.localStorage.getItem(CHAT_MODELS_KEY)
    return stored ? JSON.parse(stored) as Record<string, ChosenModel> : {}
  } catch {
    return {}
  }
}

export function saveChatModels(value: Record<string, ChosenModel>) {
  try {
    window.localStorage.setItem(CHAT_MODELS_KEY, JSON.stringify(value))
  } catch {
    // 저장하지 못해도 지금 고른 것으로 보낸다.
  }
}
const ASSISTANT_HISTORY_KEY = 'momso.desktop.assistant-history.v1'

export const DEFAULT_PREFERENCES: AppPreferences = {
  workspaceName: W.preferences.text1,
  startupView: 'inbox',
  defaultParticipantLabel: W.preferences.text2,
  reviewerName: '',
  confirmRecordingConsent: true,
  sourceRetention: 'keep',
  moveReviewedToArchive: true,
  autoAttachContext: false,
  persistAssistantHistory: false,
  responseDensity: 'balanced',
  assistantProvider: 'codex',
  assistantAuthMode: 'subscription',
  chatModel: null,
}

export function loadPreferences(): AppPreferences {
  try {
    const stored = window.localStorage.getItem(PREFERENCES_KEY)
    if (!stored) return DEFAULT_PREFERENCES
    return { ...DEFAULT_PREFERENCES, ...JSON.parse(stored) as Partial<AppPreferences> }
  } catch {
    return DEFAULT_PREFERENCES
  }
}

export function savePreferences(preferences: AppPreferences) {
  try {
    window.localStorage.setItem(PREFERENCES_KEY, JSON.stringify(preferences))
  } catch {
    // The app remains usable when a hardened webview blocks local preferences.
  }
}

export function loadAssistantHistory(): Record<string, AssistantMessage[]> {
  try {
    const stored = window.localStorage.getItem(ASSISTANT_HISTORY_KEY)
    return stored ? JSON.parse(stored) as Record<string, AssistantMessage[]> : {}
  } catch {
    return {}
  }
}

export function saveAssistantHistory(history: Record<string, AssistantMessage[]>) {
  try {
    window.localStorage.setItem(ASSISTANT_HISTORY_KEY, JSON.stringify(history))
  } catch {
    // Conversation history is optional and never blocks the workspace.
  }
}

export function clearAssistantHistory() {
  try {
    window.localStorage.removeItem(ASSISTANT_HISTORY_KEY)
  } catch {
    // Ignore storage cleanup failures in restricted browser environments.
  }
}

export function getRetentionLabel(retention: SourceRetention) {
  if (retention === 'after-review') return W.preferences.text3
  if (retention === 'ask') return W.preferences.text4
  return W.preferences.text5
}
