// 사용자에 대한 정보(설정의 "나에 대해"). 본부가 전한 계약: user_profile_get → { name, about, style, updatedAt },
// user_profile_set({ name, about, style }). 적은 내용은 보관함의 AI 규칙 파일에 들어가 세 AI 가 읽는다(백엔드).
import { invoke } from '@tauri-apps/api/core'
import { isDesktopRuntime } from './runtime'

export type UserProfile = {
  name: string
  about: string
  style: string
  updatedAt: string | null
}

export async function getUserProfile() {
  if (!isDesktopRuntime()) throw new Error('user_profile_get needs the desktop runtime')
  return invoke<UserProfile>('user_profile_get')
}

export async function setUserProfile(profile: { name: string; about: string; style: string }) {
  if (!isDesktopRuntime()) throw new Error('user_profile_set needs the desktop runtime')
  await invoke<null>('user_profile_set', profile)
}
