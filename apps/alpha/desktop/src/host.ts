// 화면이 뒤쪽을 부르는 한 겹(2026-11-21 본부 지시: momso mobile 첫 걸음).
//
// 같은 화면이 두 길로 돈다.
//   데스크톱(Tauri): invoke·listen 을 Tauri 의 것에 그대로 넘긴다(인자도 그대로). 데스크톱의 동작은 바뀌지 않는다.
//   네트워크(momso server): Tauri 가 없는 브라우저에서 네트워크 길을 켰을 때. 명령과 이벤트를 server-client.ts 가 momso server 의
//     HTTP 계약(momso-server-s17-contract.md)으로 바꿔 부른다. server 에 없는 명령은 Tauri 의 "없는 명령"과 같은 오류로 거절해,
//     화면이 이미 갖고 있는 "그 기능이 없는 뒤쪽" 길(단추 숨김·조용히 넘어감)을 그대로 탄다.
//
// 네트워크 길을 켜는 법: 빌드 때 VITE_MOMSO_NETWORK=1(momso server 가 같은 출처로 내주는 화면), 또는 주소에 ?network(개발·시험용).
// 둘 다 없고 Tauri 도 없으면 예전처럼 미리 보기. 네트워크 길은 같은 출처의 상대 경로로만 부르고 토큰을 쓰지 않는다(S19b, server-client.ts).
import { invoke as tauriInvoke, type InvokeArgs, type InvokeOptions } from '@tauri-apps/api/core'
import { listen as tauriListen, type EventCallback, type EventName, type Options, type UnlistenFn } from '@tauri-apps/api/event'
import { networkInvoke, networkListen } from './server-client'

function hasTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

/** 이 화면이 momso server 와 네트워크로 통신하는가(Tauri 가 없고 네트워크 길을 켰을 때). */
export function isNetworkRuntime() {
  if (hasTauri()) return false
  if (import.meta.env.VITE_MOMSO_NETWORK === '1') return true
  try {
    return new URLSearchParams(window.location.search).has('network')
  } catch {
    return false
  }
}

export function invoke<T>(cmd: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T> {
  if (isNetworkRuntime()) return networkInvoke<T>(cmd, args)
  return tauriInvoke<T>(cmd, args, options)
}

export function listen<T>(event: EventName, handler: EventCallback<T>, options?: Options): Promise<UnlistenFn> {
  if (isNetworkRuntime()) return networkListen<T>(event, handler)
  return tauriListen<T>(event, handler, options)
}
