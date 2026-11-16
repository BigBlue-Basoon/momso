// 스킬로 하는 연결에서 키가 필요할 때 사람이 직접 넣는 칸(2026-11-16 본부 지시).
// 모양은 archive-backend-port-notes.md "2026-11-16 옮긴 것"의 보안 입력 계약과 T48(화면 폴더에 옮겨짐, 74220277·cef940fa):
//   이벤트 agent-setup-secret-request { requestId, chatId, service, account, resourceName, fields }
//     fields 는 칸 이름 글자의 목록: accessKeyId, secretAccessKey, GCP 는 gcpOauthToken 도.
//   명령 setup_secret_submit({ requestId, chatId, input: { accessKeyId, secretAccessKey, gcpOauthToken? } })
//   명령 agent_setup_secret_cancel({ requestId, chatId }) → 값 없음(T48, 사람이 "그만두기"를 눌렀을 때)
//   이벤트 agent-setup-secret-closed { requestId, chatId } (그만두거나 300초가 지나면 닫힌다)
// 넣은 값은 AI 에게 가지 않고 몸소가 보관한다. 화면은 값을 상태·기록·채팅 글 어디에도 남기지 않는다(보낸 뒤 바로 비운다).
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isDesktopRuntime } from './runtime'

export type SecretField = { id: string; label: string; secret: boolean }

export type SecretRequest = {
  requestId: string
  chatId: string | null
  /** 회사: github | aws | gcp | cloudflare | ncp (모르는 값은 받은 그대로 보인다). */
  service: string
  account: string | null
  resourceName: string | null
  fields: SecretField[]
}

/** 뒤쪽이 주는 칸을 같은 모양으로. 글자만 오면 그 글자를 id 와 이름으로 쓴다. */
function readFields(value: unknown): SecretField[] {
  if (!Array.isArray(value)) return []
  return value.flatMap((entry) => {
    if (typeof entry === 'string' && entry.trim()) return [{ id: entry, label: entry, secret: true }]
    if (entry && typeof entry === 'object') {
      const item = entry as { id?: unknown; name?: unknown; label?: unknown; secret?: unknown }
      const id = typeof item.id === 'string' ? item.id : typeof item.name === 'string' ? item.name : null
      if (!id) return []
      return [{ id, label: typeof item.label === 'string' && item.label.trim() ? item.label : id, secret: item.secret !== false }]
    }
    return []
  })
}

export async function listenSecretRequests(handlers: { onRequest: (request: SecretRequest) => void; onClosed: (requestId: string) => void }) {
  if (!isDesktopRuntime()) return () => {}
  const stops = await Promise.all([
    listen<Record<string, unknown>>('agent-setup-secret-request', (event) => {
      const p = event.payload ?? {}
      if (typeof p.requestId !== 'string') return
      handlers.onRequest({
        requestId: p.requestId,
        chatId: typeof p.chatId === 'string' ? p.chatId : null,
        service: typeof p.service === 'string' ? p.service : '',
        account: typeof p.account === 'string' && p.account ? p.account : null,
        resourceName: typeof p.resourceName === 'string' && p.resourceName ? p.resourceName : null,
        fields: readFields(p.fields),
      })
    }),
    listen<{ requestId?: string; chatId?: string }>('agent-setup-secret-closed', (event) => { if (event.payload?.requestId) handlers.onClosed(event.payload.requestId) }),
  ])
  return () => stops.forEach((stop) => stop())
}

export async function submitSecret(request: Pick<SecretRequest, 'requestId' | 'chatId'>, values: Record<string, string>) {
  if (!isDesktopRuntime()) throw new Error('secret entry needs the desktop runtime')
  await invoke('setup_secret_submit', { requestId: request.requestId, chatId: request.chatId, input: values })
}

export async function cancelSecret(request: Pick<SecretRequest, 'requestId' | 'chatId'>) {
  if (!isDesktopRuntime()) throw new Error('secret entry needs the desktop runtime')
  await invoke('agent_setup_secret_cancel', { requestId: request.requestId, chatId: request.chatId })
}
