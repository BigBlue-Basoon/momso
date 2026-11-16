import type { ChosenModel } from './ai-models'

export type AssistantProvider = 'codex' | 'claude'

// 2026-11-16 옛 화면을 없애며 그 화면만 쓰던 값(작업공간 이름, 처음 화면, 참여자 표기, 검수자, 녹음 동의 확인,
// 원자료 보관, 검수 뒤 옮기기, 문맥 붙이기, 대화 이력 보관)과 보내기의 예전 길이 쓰던 값(응답 밀도, 로그인 방식)을 뺐다.
// 예전에 저장된 값은 읽어도 쓰지 않는다.
export type AppPreferences = {
  assistantProvider: AssistantProvider
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

export const DEFAULT_PREFERENCES: AppPreferences = {
  assistantProvider: 'codex',
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
