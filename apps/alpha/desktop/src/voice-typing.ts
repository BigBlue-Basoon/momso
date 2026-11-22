// 입력 칸의 마이크: Windows 받아쓰기(Win+H)를 연다. 돈이 들지 않는 방법(2026-11-16 그의 말 "받아쓰기는 돈 안되는걸로 해주라").
// 계약(archive-backend-port-notes.md 307줄):
//   voice_typing_status() → { state: 'unknown' | 'unsupported', microphone, language, permission, network }
//     microphone 은 참고값일 뿐 준비됐다는 뜻이 아니라 화면은 막는 데 쓰지 않는다. unsupported 면 마이크 단추를 막는다.
//   voice_typing_open() → { state: 'requested' | 'not_focused' | 'keys_busy' | 'input_rejected' | 'unsupported' }
//     받아쓰기 글은 Windows 가 지금 글자 칸(입력 칸)에 넣는다. 그래서 부르기 전에 입력 칸에 커서를 둔다.
import { invoke } from './host'
import { isDesktopRuntime } from './runtime'

export type VoiceTypingStatus = { state: string; microphone?: string; language?: string; permission?: string; network?: string }

export async function getVoiceTypingStatus() {
  if (!isDesktopRuntime()) throw new Error('voice typing needs the desktop runtime')
  return invoke<VoiceTypingStatus>('voice_typing_status')
}

export async function openVoiceTyping() {
  if (!isDesktopRuntime()) throw new Error('voice typing needs the desktop runtime')
  const result = await invoke<{ state?: string } | string>('voice_typing_open')
  return typeof result === 'string' ? result : result?.state ?? 'unsupported'
}
