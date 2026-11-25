// momso server(고객에게는 momso anywhere 의 안쪽) 와 통신하는 길(2026-11-21).
//   API 계약: 공유 Git hq/s17-done(6ec22f472) 의 docs/development/momso-server-s17-contract.md
//   연결 계약: hq/s19b-done 의 docs/development/momso-mobile-connection-s19.md "S19b: 같은 출처 화면과 깨우기"
//     server 가 화면 파일도 같은 출처로 준다(MOMSO_SERVER_WEB_DIR). 브라우저는 지금 출처의 상대 경로로 화면·API·SSE 를 부르고, 토큰이나 Authorization 을 쓰지 않는다.
//     앞단의 로그인 문(Cloudflare Access)이 로그인을 맡고, VM 안 프록시가 server 토큰을 대신 붙인다.
//
// host.ts 가 네트워크 길일 때 화면의 명령·이벤트를 여기로 넘긴다. 화면 코드는 데스크톱과 같은 명령 이름과 이벤트 이름(chat-stream, chat-done, chat-error)을 쓴다.
//
//   GET  /health                         → 화면을 열 때 확인(ServerGate)
//   GET  /chats                          → chat_list, chat_list_page(최근 변경 순, 쪽 나눔 없음)
//   POST /chats { title? }               → chat_create, chat_create_with_message(제목 = 첫 말 앞 40자. 첫 말은 그 뒤 chat_send 가 turns 로 보낸다)
//   GET  /chats/{chatId}                 → chat_read, chat_read_page(제목·때·말, 쪽 나눔 없음)
//   POST /chats/{chatId}/turns           → chat_send → { turnId, userMessageId }
//   POST /turns/{turnId}/retry           → chat_send 의 retryMessageId 가 실패한 그 말이면(같은 말이 두 번 남지 않음)
//   POST /turns/{turnId}/cancel          → chat_cancel(그 방의 진행 중인 답)
//   GET  /turns/{turnId}/events?after=N  → SSE chat-delta(offset = 그 조각까지의 UTF-8 바이트) → chat-stream, chat-done → chat-done 또는 chat-error.
//                                          끊기면 받은 바이트로 after 를 주어 다시 붙는다(몇 번). 그래도 안 되면 GET /turns/{turnId} 를 1초마다 읽는다.
//
// 실패 까닭(chat-done failed 의 error) → 화면의 실패 줄(App 의 W.app.failure[kind]) 과 "다시 보내기":
//   cli_unavailable → serverCliUnavailable, provider_failed → serverProviderFailed, cli_disconnected·cli_io_error → serverCliStopped,
//   empty_response → serverEmpty, server_restarted → serverRestarted, storage_error → serverStorage, cli_timeout → timeout,
//   unavailable → serviceUnavailable, 그 밖 → unknown. cancelled(멈추기)는 실패 줄 없이 멈춘다.
//
// AI 연결: 줄3 S25. 계약 판 S17-2026-11-21.2 "AI 연결 계약 확정"(공유 Git hq/s17-contract a4a62e34b momso-server-s17-contract.md "S25", 구현 provider_login.rs). 코덱스만.
//   GET  /providers                      → provider_auth_status. { providers: [{ provider, state, accountLabel }] }.
//                                           코덱스 state: connected → account, not_connected → signedOut, cli_unavailable → unavailable. not_supported(옛 server 의 클로드·제미나이)는 뺀다.
//                                           클로드·제미나이(S55, hq/merge-s57-done momso-server-s17-contract.md "S55"): not_connected → signedOut,
//                                           credential_saved → apiKey + apiKeySaved(키를 보관했다는 뜻. 맞는지·잔액·연결은 확인하지 않음: keyVerification null),
//                                           storage_unavailable(암호화 보관소를 열 수 없음) → unavailable·runnable false. methods ['apiKey'].
//   provider_api_key_set({ provider, apiKey }) → GET /providers/{p}/key/request 의 inputs 에 대기 중 요청이 있으면 그것, 없으면 POST /providers/{p}/key/request 로
//                                           provider_api_key 입력 요청(requestId, 5분)을 만들고 POST /input-requests/{requestId}/respond { apiKey } → { state: credential_saved }.
//                                           결과 'keySaved' | 'invalidKey'(400 invalid_key·invalid_input) | 'storageUnavailable'(503) | 'unavailable'. 실패하면 그 요청을 취소한다.
//                                           키는 받은 그 순간 넘기기만 한다: 이 모듈·화면 상태·기록·주소 어디에도 두지 않는다(계약).
//   모델 목록(S62, hq/s62-done momso-server-s17-contract.md "공급자와 모델 목록 (S62)"): ai_models → GET /providers/models
//                                           { providers: [{ provider, state, models: [{ id, label, isDefault, defaultEffort, efforts, fast: { supported, state } }] }] }
//                                           → 데스크톱 ai_models 모양. state connected → ok, not_connected → not_connected, 그 밖 → unavailable(reason = state).
//                                           isDefault null 은 그대로 두어 데스크톱처럼 목록의 첫 값이 기본(defaultModel). fast.state unknown → unavailable
//                                           (데스크톱 뜻: 켜졌는지 확인되지 않음, 보내 보고 안 되면 fast_unavailable). 목록이 없는 옛 server(404)면 명령이 없는 것처럼.
//   chat_send: provider·model·effort·fast 를 turns 에 그대로(목록에서 고른 값만 화면이 보낸다). 빈 방 번호로 묻기(probeSendableProviders)는
//                                           GET /providers/models 의 그 회사 state 가 connected 면 보낼 수 있음(invalid_chat), 아니면 unavailable. 400 fast_unavailable → { status: fast_unavailable },
//                                           턴이 fast_unavailable 로 끝나면 chat-error 의 status fast_unavailable(화면이 그 모델의 고속 단추를 끈다).
//   첨부(S39, 같은 계약 "S39 휴대폰 첨부파일"): uploadAttachment(file) → POST /uploads { fileName, mediaType, byteLength } → { uploadId, received, total }
//                                           → PUT /uploads/{id}/chunks?offset=N(원본 바이트, 4 MiB씩. 최대 8 MiB) → 끊기거나 offset_mismatch 면 GET /uploads/{id} 의 received 부터 이어서
//                                           → POST /uploads/{id}/finish → { id, originalFileName, mediaType }. chat_send 의 attachments(assetId 배열)는 turns 의 attachments: [{ assetId }] 로.
//                                           GET /chats/{chatId} 의 내 말 attachments: [{ assetId, name, mediaType, byteLength }] → 말풍선의 첨부.
//                                           cli_unavailable 은 CLI 가 없거나 실행·상태 확인에 실패한 것(꼭 안 깔림은 아님): 설치 여부를 단정하지 않는다.
//                                           accountLabel(코덱스 계정 메일, 모르면 null)은 그대로. 없으면(404 등) AI 연결이 아직 없는 server 로 보고 묻지 않는다.
//   POST /providers/codex/login          → provider_login_start. { sessionId }. 새로 시작하면 server 가 진행 중인 이전 로그인을 취소한다(이전 sessionId 는 바로 못 씀).
//                                           sessionId 는 이 모듈의 메모리에만 둔다(계약: 메모리에만. 주소·기록·저장소에 없음).
//                                           아래 세 요청은 X-Momso-Login-Session 머리에 그 값을 넣는다. 다른 세션이면 404 login_not_found.
//   GET  /providers/codex/login          → { provider, state: starting|waiting|connected|failed|cancelled|expired|cli_unavailable, url, code, expiresInSeconds }.
//                                           starting·waiting 은 진행 중. 이벤트 길이 끊기면 2초마다, 이 탭으로 돌아오면(visibilitychange·focus) 바로 읽는다.
//   GET  /providers/codex/login/events   → SSE provider-login-state(코드 전) · provider-login-device { state: waiting, url, code, expiresInSeconds }
//                                           · provider-login-device-closed { provider, state: connected|failed|cancelled|expired|cli_unavailable }.
//   POST /providers/codex/login/cancel   → provider_login_cancel.
//   화면 이벤트(데스크톱과 같은 이름): provider-login-device { provider, url, code, expiresInSeconds }, provider-login-device-closed { provider, state },
//     provider-auth-changed { provider, state: account|signedOut, reason: 끝난 state(connected·failed·expired·cli_unavailable·login_not_found) }.
//   코드 다시 받기: 지켜보던 로그인을 놓고 새로 시작한다(이전 로그인은 server 가 취소).
//   채팅 실패가 cli_unavailable·provider_failed·auth_failed 면 연결 상태를 다시 읽어, 로그인이 필요하면 onServerAiNeeded 로 연결 화면을 부른다.
//   cli_unavailable 은 로그인으로 풀 수 없어 연결 화면을 띄우지 않는다(채팅 실패 줄이 알린다).
// 승인·브라우저·로그인 저장: 줄3 S23(공유 Git hq/s23-done 20d07ec04 docs/development/momso-server-approval-s23.md, 구현 apps/alpha/server/src/main.rs·browser_bridge.rs).
//   POST /chats/{id}/turns 에 permissionMode(Ask for approval 기본·Approve for me·Full access)를 싣는다. 재시도는 server 가 원래 단계를 쓴다.
//     화면의 권한 고르기(permission_modes_available·get·set): 코덱스의 세 단계(데스크톱 뒤쪽 permission_modes.rs 의 목록 그대로), 고른 값은 이 기기에 둔다(localStorage).
//   턴 SSE 의 agent-approval-request { requestId, chatId, turnId, provider, kind, description, options } → 화면 이벤트 agent-approval-request(데스크톱 모양):
//     command·file_change 는 그대로. browser_site(description = 호스트) → kind browser_action, reason site_access, targets[0] = 호스트, 물음 글은 데스크톱 뒤쪽 글
//     (데스크톱의 사이트 승인 카드와 같은 꼴: "이 채팅방에서 계속 허락"·"거절". S27 이 같은 채팅방에서 기억). browser_login(description = 출처) → browser_action,
//     reason saved_login, 하려는 일 "저장된 로그인"(데스크톱 뒤쪽은 "스킬 · 로그인 이름"을 주지만 S23 은 출처만 준다).
//     SSE 를 다시 붙으면 server 가 대기 중인 물음을 다시 보낸다. 조회로 넘어가면 GET /turns/{id}/approval({ approval|null })을 함께 읽는다.
//     화면을 다시 불러오면 GET /chats/{id} 의 사용자 말 turnState running 인 턴을 다시 따라가 물음이 다시 뜬다.
//   agent_approval_respond({ requestId, choice }) → POST /approvals/{id}/respond { choice }. 끝나면 agent-approval-closed reason responded.
//     409 approval_closed(그사이 5분 지남·멈춤·server 다시 시작) → agent-approval-closed reason 없음(화면은 "닫혔습니다" 줄). 턴이 끝나면 남은 물음은 provider_cancelled 로 닫는다.
//   턴 SSE 의 browser-human-required { id, chatId, turnId, reason: verification|one_time_code, origin } → 화면 이벤트 browser-human-required { chatId, reason }(id 마다 한 번).
//     조회로 넘어가면 GET /turns/{id}/browser-events({ events })를 읽는다. 인증 번호를 돌려주는 길은 아직 없다(줄3).
//   저장된 로그인(S23 판 S23-2026-11-21.1 "S32 화면과 설치 담당 계약"): saved_login_list → GET /saved-logins { logins }, saved_login_set 새 항목 → POST /saved-logins,
//     id 가 있으면 → PUT /saved-logins/{id}(바꿀 값만, password 를 빼면 그대로), saved_login_delete → DELETE /saved-logins/{id}. 비밀번호는 오지 않는다.
//     관리 화면의 입구는 아직 없다(그가 정할 일). 명령만 잇는다.
// 입력 물음·닫힘·다시 잇기: 줄3 S32(공유 Git hq/s32-done 0bcbb8c9c momso-server-approval-s23.md 판 S23-2026-11-21.1, human_prompt.rs, momso-server-s17-contract.md "S32").
//   턴 SSE 의 agent-input-request { requestId, chatId, turnId, kind, origin, expiresAt, fields: [{ id, secret }], choices }:
//     one_time_code(칸 code)·saved_login(칸 label·username·password) → 화면 이벤트 agent-setup-secret-request(키 넣기 카드와 같은 꼴, kind·origin 을 붙임).
//     select_saved_login(choices [{ id, label, username }]) → saved-login-selection-request(데스크톱의 로그인 고르기 카드).
//   답: setup_secret_submit → POST /input-requests/{id}/respond(인증 번호 { code }, 로그인 { label, username, password }), saved_login_select → { selectionId }.
//     취소: agent_setup_secret_cancel·saved_login_cancel → POST /input-requests/{id}/cancel. 잘못된 입력(400 invalid_input)은 물음이 남는다. 이미 닫힘은 409 request_closed.
//   agent-input-closed { requestId, chatId, turnId } → agent-setup-secret-closed / saved-login-selection-closed(이 화면이 답했으면 submitted·selected, 취소면 user_cancelled·cancelled,
//     expiresAt 이 지났으면 timeout, 그 밖은 까닭 없음).
//   agent-approval-closed { requestId, chatId, turnId, reason: responded|provider_cancelled|timeout|server_restarted, decision } → 화면 이벤트 agent-approval-closed
//     (이 화면이 답한 것은 이미 닫음, 다른 곳에서 답함·server 다시 시작은 까닭 없음 = "닫혔습니다").
//   다시 붙거나 조회로 따라갈 때 GET /turns/{id}/requests { approval, inputs } 를 정본으로: 거기 없는 열린 물음은 닫고, 있는 것은 띄운다(옛 server 면 /approval).
//   화면을 다시 불러오면 GET /chats/{id} 의 activeTurn { turnId, state: running, offset } 의 턴을 after=0 부터 다시 받는다(chat-resumed 로 그 채팅방을 답을 쓰는 중으로 두어
//     받은 글이 실시간으로 이어 보임. 저장된 말에는 쓰는 중인 답이 없어 겹치지 않는다). activeTurn 이 없는 옛 server 는 사용자 말의 turnState running 으로.
// 문서 읽기와 자료 도구: 줄3 S34(공유 Git hq/s34-done 087247e4a momso-server-s17-contract.md 판 S17-2026-11-21.4 "S34 문서 읽기 및 자료 도구").
//   server_documents_list → GET /documents { documents: [{ id, title, folder, updatedAt }] }, server_document_read({ id }) → GET /documents/{id} { id, title, folder, updatedAt, body }.
//   쓰기 HTTP 는 없다(S37). 오류 document_not_found·document_invalid. 스킬(momso_skill_read)·사전(momso_dictionary_search)은 AI 쪽 도구뿐이라 화면에서 고르는 길이 없다.
//   수업·수련생 자료 쓰기(momso_crm_upsert·momso_crm_mark_read)는 승인 kind crm_write(S23 판 S23-2026-11-21.2, description = 도구 이름과 건수, options allow_once·deny) → 승인 카드(kind other).
// 로그인이 풀림: 응답이 다른 곳으로 보내기(opaqueredirect)·401 이거나 JSON 자리에 HTML 이 오면 한 번 알린다(onServerSignedOut).
//   몸소 입구(줄2 S26, hq/s26-done 7fa9efbc1 momso-anywhere-pwa-entry-20261121.md "화면 계약"): 로그인 전 API 는 401 { error: login_required }.
//   403 은 잘못된 출처·로그인 증명 같은 보안 거절일 수 있어 무조건 다시 로그인으로 보지 않는다: JSON 403 은 forbidden(문은 "닿지 못함"), 로그인 문의 HTML 403 만 풀림.
//   503 은 VM 통신 실패(닿지 못함). 로그아웃(POST /__momso/auth/logout)은 화면에 단추가 없어 부르지 않는다.
// 회사·모델: 이 단계는 코덱스 하나. chat_send 의 provider 가 codex 가 아니면 unavailable(말을 보내지 않음).
import type { InvokeArgs } from '@tauri-apps/api/core'
import type { EventCallback, EventName, UnlistenFn } from '@tauri-apps/api/event'
import { sha256 } from '@noble/hashes/sha256'
import { W } from './words'
import { createServerTransport, isSignedOut, readServerSse, ServerError } from './server-request'
export { ServerError, getServerConnectionState, onServerConnectionChange } from './server-request'

// ---- 로그인이 풀림 알림 ----
const SIGNED_OUT_EVENT = 'momso:server-signed-out'
let signedOut = false

function markSignedOut() {
  if (signedOut) return
  signedOut = true
  window.dispatchEvent(new CustomEvent(SIGNED_OUT_EVENT))
}

/** 로그인이 풀렸을 때 한 번 부른다. 멈추는 함수를 돌려준다. */
export function onServerSignedOut(listener: () => void) {
  window.addEventListener(SIGNED_OUT_EVENT, listener)
  return () => window.removeEventListener(SIGNED_OUT_EVENT, listener)
}

/** 같은 출처 요청: 읽기만 깨우기를 기다리고 쓰기는 자동 재전송하지 않는다. */
const { call, request } = createServerTransport({ signedOut: markSignedOut })

/** 화면을 열 때: GET /health. ready / 로그인이 풀림 / 닿지 못함(꺼짐·5xx·그 밖). */
export async function checkServerHealth(): Promise<'ready' | 'signed_out' | 'unreachable'> {
  try {
    const value = await request<{ state?: unknown }>('GET', '/health')
    return value?.state === 'ready' ? 'ready' : 'unreachable'
  } catch (error) {
    return isSignedOut(error) ? 'signed_out' : 'unreachable'
  }
}

// ---- 이벤트: 화면의 listen 을 받아 두고, 아래 스트림이 보낸다 ----
const handlers = new Map<string, Set<EventCallback<unknown>>>()
let eventSeq = 0

function emit(event: string, payload: unknown) {
  eventSeq += 1
  for (const handler of [...(handlers.get(event) ?? [])]) handler({ event: event as EventName, id: eventSeq, payload })
}

export async function networkListen<T>(event: EventName, handler: EventCallback<T>): Promise<UnlistenFn> {
  const set = handlers.get(event) ?? new Set()
  set.add(handler as EventCallback<unknown>)
  handlers.set(event, set)
  return () => { set.delete(handler as EventCallback<unknown>) }
}

const readSse = readServerSse

// ---- 채팅방(server 것) ----
type ServerChat = { id?: unknown; chatId?: unknown; title?: unknown; createdAt?: unknown; updatedAt?: unknown; messageCount?: unknown; pinned?: unknown; pinnedOrder?: unknown; archivedAt?: unknown; groupId?: unknown }
type ServerMessage = { id?: unknown; role?: unknown; text?: unknown; createdAt?: unknown; turnId?: unknown; turnState?: unknown; attachments?: unknown; skill?: unknown }
type ActiveTurn = { turnId?: unknown; state?: unknown }

const str = (value: unknown) => (typeof value === 'string' ? value : '')

function readEntry(chat: ServerChat) {
  const id = str(chat.id) || str(chat.chatId)
  return id ? {
    id,
    title: str(chat.title),
    createdAt: str(chat.createdAt),
    updatedAt: str(chat.updatedAt) || str(chat.createdAt),
    messageCount: typeof chat.messageCount === 'number' ? chat.messageCount : 0,
    // 고정(S75 POST /chats/{id}/pin): 고정한 방은 pinned 와 고정한 차례. 옛 server 는 값이 없어 고정하지 않은 것으로 본다.
    pinned: chat.pinned === true,
    pinnedOrder: typeof chat.pinnedOrder === 'number' ? chat.pinnedOrder : null,
    // 묶음(S75 둘째: PUT /chats/{id}/group). 묶이지 않았으면 null.
    groupId: str(chat.groupId) || null,
  } : null
}

async function listChats() {
  const value = await request<{ chats?: unknown }>('GET', '/chats')
  return (Array.isArray(value?.chats) ? (value.chats as ServerChat[]) : []).flatMap((chat) => { const entry = readEntry(chat); return entry ? [entry] : [] })
}

async function readDocument(id: string) {
  const value = await request<ServerChat & { messages?: unknown; activeTurn?: ActiveTurn | null }>('GET', `/chats/${encodeURIComponent(id)}`)
  const messages = (Array.isArray(value?.messages) ? (value.messages as ServerMessage[]) : []).flatMap((message, index) => {
    if (message?.role !== 'user' && message?.role !== 'assistant') return []
    const attachments = message.role === 'user' && Array.isArray(message.attachments)
      ? (message.attachments as Array<Record<string, unknown>>).flatMap((file) => (str(file?.assetId) ? [{ assetId: str(file.assetId), name: str(file.name) || str(file.assetId), mediaType: str(file.mediaType) || null }] : []))
      : undefined
    // 내 말에 고른 스킬(S75 둘째: 턴의 skill 을 server 가 말에 남긴다). 말풍선 위 스킬 표시에 쓴다.
    const skill = message.role === 'user' ? str(message.skill) : ''
    return [{ id: str(message.id) || `${id}-${index}`, role: message.role, text: str(message.text), createdAt: str(message.createdAt), ...(skill ? { skill } : {}), ...(attachments && attachments.length > 0 ? { attachments } : {}) }]
  })
  // 화면을 다시 불러왔거나 다른 기기에서 보낸 말의 답이 아직 쓰이는 중이면(S32 activeTurn, 옛 server 는 turnState running) 그 턴을 처음부터 다시 따라간다:
  // 그 채팅방을 답을 쓰는 중으로 두어(chat-resumed) 답이 실시간으로 이어 보이고, 대기 중인 물음이 다시 뜬다.
  const raw = Array.isArray(value?.messages) ? (value.messages as ServerMessage[]) : []
  const activeId = value?.activeTurn && value.activeTurn.state === 'running' ? str(value.activeTurn.turnId) : ''
  const live = activeId
    ? raw.find((message) => message?.role === 'user' && str(message.turnId) === activeId) ?? null
    : [...raw].reverse().find((message) => message?.role === 'user' && message.turnState === 'running' && str(message.turnId)) ?? null
  const liveTurn = activeId || (live ? str(live.turnId) : '')
  if (liveTurn && running.get(id) !== liveTurn) {
    emit('chat-resumed', { chatId: id })
    void streamTurn(liveTurn, id, live ? str(live.id) || null : null)
  }
  return { id, title: str(value?.title), createdAt: str(value?.createdAt), updatedAt: str(value?.updatedAt) || str(value?.createdAt), messages }
}

async function createChat(title: string) {
  const made = await request<{ chatId?: unknown }>('POST', '/chats', title ? { title } : {})
  const id = str(made?.chatId)
  if (!id) throw new ServerError('invalid_response', null)
  const document = await readDocument(id).catch(() => null)
  const now = new Date().toISOString()
  return {
    id,
    title: document?.title ?? title,
    createdAt: document?.createdAt || now,
    updatedAt: document?.updatedAt || now,
    messageCount: document?.messages.length ?? 0,
  }
}

// ---- 답 받기 ----
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))
const encoder = new TextEncoder()

/** 방마다 지금 답을 쓰는 턴(멈추기에 쓴다). 실패한 내 말(userMessageId)마다 그 실패 턴(다시 보내기에 쓴다). */
const running = new Map<string, string>()
const failedTurns = new Map<string, string>()

/** server 의 실패 까닭 → 화면 실패 줄의 kind. */
function failureKind(code: string | null) {
  switch (code) {
    case 'cli_unavailable': return 'serverCliUnavailable'
    case 'provider_failed': return 'serverProviderFailed'
    case 'cli_disconnected': case 'cli_io_error': return 'serverCliStopped'
    case 'empty_response': return 'serverEmpty'
    case 'server_restarted': return 'serverRestarted'
    case 'storage_error': return 'serverStorage'
    case 'cli_timeout': return 'timeout'
    case 'auth_failed': return 'authentication'
    case 'unavailable': return 'serviceUnavailable'
    default: return 'unknown'
  }
}

function finishTurn(turnId: string, chatId: string, state: unknown, error: unknown, userMessageId: string | null, detail: unknown = null) {
  if (running.get(chatId) === turnId) running.delete(chatId)
  closeApprovals(turnId, 'provider_cancelled')
  for (const [requestId, open] of [...openInputs]) if (open.turnId === turnId) closeInput(requestId, 'provider_cancelled')
  if (state === 'completed') { emit('chat-done', { chatId }); return }
  const code = typeof error === 'string' ? error : null
  // 멈추기: 실패 줄 없이 멈춘다(화면은 "중단" 이 든 까닭을 조용히 끝낸다).
  if (code === 'cancelled') { emit('chat-error', { chatId, reason: '요청을 중단했습니다' }); return }
  if (userMessageId) failedTurns.set(userMessageId, turnId)
  // 고속 모드를 거부당함(S62): 데스크톱과 같은 status 로(그 모델의 고속 단추를 끄고 한 줄 알림).
  if (code === 'fast_unavailable') { emit('chat-error', { chatId, status: 'fast_unavailable', userMessageId }); return }
  emit('chat-error', { chatId, reason: code, userMessageId, error: { kind: failureKind(code), actions: ['retry'], code, retryable: code === 'stream_interrupted' || code === 'server_unreachable', detail } })
  // AI 를 못 쓴 실패면 연결 상태를 다시 읽어, 연결돼 있지 않으면 연결 화면으로 이끈다(실패 줄과 다시 보내기는 그대로 남는다).
  if (code === 'cli_unavailable' || code === 'provider_failed' || code === 'auth_failed') {
    void aiConnection().then((value) => { if (value === 'needed') window.dispatchEvent(new CustomEvent(AI_NEEDED_EVENT)) })
  }
}

// ---- 승인 물음(S23) ----
type ServerApproval = { requestId?: unknown; chatId?: unknown; turnId?: unknown; provider?: unknown; kind?: unknown; description?: unknown; options?: unknown }
const CHOICES = ['allow_once', 'allow_session', 'deny', 'cancel']
/** 열려 있는 물음(requestId → 그 턴·채팅방). */
const openApprovals = new Map<string, { turnId: string; chatId: string | null; answering?: boolean }>()
/** 이미 알린 사람이 할 일(턴:번호). */
const shownHuman = new Set<string>()

/** S23 물음 → 데스크톱 agent-approval-request 모양(같은 카드·같은 말). */
function approvalRequest(value: ServerApproval) {
  const requestId = str(value.requestId)
  if (!requestId) return null
  const description = str(value.description)
  const base = {
    requestId,
    chatId: str(value.chatId) || null,
    provider: str(value.provider) || 'codex',
    reason: null as string | null,
    options: (Array.isArray(value.options) ? value.options : []).filter((choice): choice is string => typeof choice === 'string' && CHOICES.includes(choice)),
    change: null,
    targets: [] as Array<{ path: string; change: null }>,
  }
  switch (str(value.kind)) {
    case 'browser_site': return { ...base, kind: 'browser_action', reason: 'site_access', description: W.approvalCard.browser.siteQuestion, targets: [{ path: description, change: null }] }
    case 'browser_login': return { ...base, kind: 'browser_action', reason: 'saved_login', description: W.savedLogins.title, targets: [{ path: description, change: null }] }
    case 'command': return { ...base, kind: 'command', description }
    case 'file_change': return { ...base, kind: 'file_change', description }
    // 수업·수련생 자료 쓰기(S34 crm_write): 하려는 일 = 도구 이름과 건수(이름·연락처·본문은 오지 않는다).
    case 'crm_write': return { ...base, kind: 'other', description }
    default: return { ...base, kind: 'other', description }
  }
}

function showApproval(value: ServerApproval) {
  const request = approvalRequest(value)
  if (!request || openApprovals.has(request.requestId)) return
  openApprovals.set(request.requestId, { turnId: str(value.turnId), chatId: request.chatId })
  emit('agent-approval-request', request)
}

function closeApprovals(turnId: string, reason: string | null) {
  for (const [requestId, open] of [...openApprovals]) {
    if (open.turnId !== turnId) continue
    openApprovals.delete(requestId)
    emit('agent-approval-closed', { requestId, chatId: open.chatId, reason })
  }
}

/** server 가 닫은 승인(S32 agent-approval-closed, 또는 대기 목록에 없음). 이 화면이 답한 것은 이미 닫았다. */
function closeApproval(requestId: string, reason: string | null) {
  const open = openApprovals.get(requestId)
  if (!open) return
  openApprovals.delete(requestId)
  // 다른 곳에서 답함(responded)·server 다시 시작은 화면에 "닫혔습니다"(까닭 없음).
  emit('agent-approval-closed', { requestId, chatId: open.chatId, reason: reason === 'timeout' || reason === 'provider_cancelled' ? reason : null })
}

// ---- 입력 물음(S32): 인증 번호·로그인 저장·로그인 고르기 ----
type ServerInput = { requestId?: unknown; chatId?: unknown; turnId?: unknown; kind?: unknown; origin?: unknown; expiresAt?: unknown; fields?: unknown; choices?: unknown }
const openInputs = new Map<string, { turnId: string; chatId: string | null; kind: string; expiresAt: number; answered: string | null }>()

function showInput(value: ServerInput, turnId: string) {
  const requestId = str(value.requestId)
  if (!requestId || openInputs.has(requestId)) return
  const kind = str(value.kind)
  const chatId = str(value.chatId) || null
  const origin = str(value.origin)
  const entry = { turnId: str(value.turnId) || turnId, chatId, kind, expiresAt: Date.parse(str(value.expiresAt)) || 0, answered: null }
  if (kind === 'select_saved_login') {
    const entries = (Array.isArray(value.choices) ? value.choices : []).flatMap((choice) => {
      const item = (choice ?? {}) as Record<string, unknown>
      return str(item.id) ? [{ id: str(item.id), origin, label: str(item.label), username: str(item.username) }] : []
    })
    if (!entries.length) return
    openInputs.set(requestId, entry)
    emit('saved-login-selection-request', { requestId, chatId, entries })
    return
  }
  if (kind !== 'one_time_code' && kind !== 'saved_login') return
  const fields = (Array.isArray(value.fields) ? value.fields : []).flatMap((field) => {
    const item = (field ?? {}) as Record<string, unknown>
    return str(item.id) ? [{ id: str(item.id), label: str(item.id), secret: item.secret === true }] : []
  })
  openInputs.set(requestId, entry)
  emit('agent-setup-secret-request', { requestId, chatId, service: '', kind, origin, fields })
}

function closeInput(requestId: string, fallback: string | null) {
  const open = openInputs.get(requestId)
  if (!open) return
  openInputs.delete(requestId)
  const reason = open.answered ?? fallback ?? (open.expiresAt && Date.now() >= open.expiresAt ? 'timeout' : null)
  if (open.kind === 'select_saved_login') {
    emit('saved-login-selection-closed', { requestId, reason: reason === 'submitted' ? 'selected' : reason === 'user_cancelled' ? 'cancelled' : reason })
  } else {
    emit('agent-setup-secret-closed', { requestId, chatId: open.chatId, reason })
  }
}

/** 입력 물음에 답한다. 값은 넘기기만 한다. 잘못된 입력이면 물음이 남는다(오류를 그대로 던진다). */
async function answerInput(requestId: string, body: Record<string, string>) {
  const open = openInputs.get(requestId)
  if (open) open.answered = 'submitted'
  try {
    await request('POST', `/input-requests/${encodeURIComponent(requestId)}/respond`, body)
  } catch (error) {
    if (open) open.answered = null
    if (error instanceof ServerError && error.code === 'request_closed') closeInput(requestId, null)
    throw error
  }
  closeInput(requestId, null)
}

async function cancelInput(requestId: string) {
  const open = openInputs.get(requestId)
  if (open) open.answered = 'user_cancelled'
  await request('POST', `/input-requests/${encodeURIComponent(requestId)}/cancel`)
  closeInput(requestId, null)
}

function showHuman(value: { id?: unknown; chatId?: unknown; turnId?: unknown; reason?: unknown }, turnId: string) {
  const key = `${str(value.turnId) || turnId}:${String(value.id ?? '')}`
  if (shownHuman.has(key) || !str(value.chatId)) return
  shownHuman.add(key)
  emit('browser-human-required', { chatId: str(value.chatId), reason: str(value.reason) || 'verification' })
}

/** 다시 붙거나 조회로 따라갈 때: 대기 목록(S32 /requests)을 정본으로 열린 물음을 맞추고, 놓친 사람 할 일을 읽는다. */
async function catchUpTurn(turnId: string, withBrowser: boolean) {
  const path = `/turns/${encodeURIComponent(turnId)}`
  let value: { approval?: ServerApproval | null; inputs?: unknown } | null = null
  try {
    value = await request('GET', `${path}/requests`)
  } catch (error) {
    // 옛 server(S23): 승인만.
    if (error instanceof ServerError && error.status === 404 && error.code !== 'turn_not_found') {
      try { value = { approval: (await request<{ approval?: ServerApproval | null }>('GET', `${path}/approval`))?.approval ?? null } } catch { value = null }
    }
  }
  if (value) {
    const approvalId = value.approval ? str(value.approval.requestId) : ''
    for (const [requestId, open] of [...openApprovals]) if (open.turnId === turnId && requestId !== approvalId && !open.answering) closeApproval(requestId, null)
    if (value.approval) showApproval(value.approval)
    if (Array.isArray(value.inputs)) {
      const ids = new Set(value.inputs.map((input) => str((input ?? {}).requestId)))
      for (const [requestId, open] of [...openInputs]) if (open.turnId === turnId && !ids.has(requestId) && !open.answered) closeInput(requestId, null)
      for (const input of value.inputs) showInput((input ?? {}) as ServerInput, turnId)
    }
  }
  if (!withBrowser) return
  try {
    const value = await request<{ events?: unknown }>('GET', `/turns/${encodeURIComponent(turnId)}/browser-events`)
    for (const event of Array.isArray(value?.events) ? value.events : []) showHuman(event as Record<string, unknown>, turnId)
  } catch { /* 없음 */ }
}

// ---- 권한 단계(S23 permissionMode). 코덱스의 세 단계는 데스크톱 뒤쪽(permission_modes.rs available)과 같은 값·원문. ----
const CODEX_PERMISSION_MODES = [
  { id: 'Ask for approval', label: 'Ask for approval', description: 'ask to edit external files and use the internet', available: true, isDefault: true },
  { id: 'Approve for me', label: 'Approve for me', description: 'actions detected as potentially unsafe', available: true, isDefault: false },
  { id: 'Full access', label: 'Full access', description: 'internet and any file on your computer', available: true, isDefault: false },
]
const PERMISSION_KEY = 'momso:permission-mode:codex'
function codexPermission() {
  try {
    const value = localStorage.getItem(PERMISSION_KEY)
    if (value && CODEX_PERMISSION_MODES.some((mode) => mode.id === value)) return value
  } catch { /* 저장소를 못 쓰면 기본 */ }
  return 'Ask for approval'
}

// ---- AI 연결 ----
const AI_NEEDED_EVENT = 'momso:server-ai-needed'

/**
 * 휴대폰 설정의 AI 연결에서 "코덱스에서 로그인"을 누름(S75 셋째): 로그인 코드 카드가 있는 휴대폰의 AI 연결 화면(ServerAiConnect)을 부른다.
 * 데스크톱 설정 줄은 코드 카드를 앱 전체 자리에 띄우는데 휴대폰에는 그 자리가 없어, 같은 일을 하는 연결 화면으로 보낸다.
 */
export function askServerAiConnect() {
  window.dispatchEvent(new CustomEvent(AI_NEEDED_EVENT))
}

/** 채팅이 AI 를 못 써 실패했고 AI 가 연결돼 있지 않을 때. 멈추는 함수를 돌려준다. */
export function onServerAiNeeded(listener: () => void) {
  window.addEventListener(AI_NEEDED_EVENT, listener)
  return () => window.removeEventListener(AI_NEEDED_EVENT, listener)
}

type ServerProvider = { provider?: unknown; state?: unknown; accountLabel?: unknown }
type NetProvider = { provider: string; installed: boolean; runnable: boolean; state: string; accountLabel: string | null; planLabel: null; apiKeySaved: boolean; keyVerification: string | null; methods: string[]; active: null }

async function providerStatus() {
  const value = await request<{ providers?: unknown }>('GET', '/providers')
  return (Array.isArray(value?.providers) ? (value.providers as ServerProvider[]) : []).flatMap((item): NetProvider[] => {
    const provider = str(item.provider)
    // not_supported(옛 server 의 클로드·제미나이)는 뺀다.
    if (!provider || item.state === 'not_supported') return []
    if (provider === 'claude' || provider === 'gemini') {
      // API 키만(S55). credential_saved 는 보관했다는 뜻이지 맞는 키라는 뜻이 아니다(확인 결과가 없어 keyVerification 은 null).
      const saved = item.state === 'credential_saved'
      const keyState = saved ? 'apiKey' : item.state === 'not_connected' ? 'signedOut' : 'unavailable'
      // keyVerification notSupported: 이 server 는 키를 확인하지 않는다(사실 그대로). 보낼 수 있는지는 S62 모델 목록의 state 가 정한다(chat_send 묻기).
      return [{ provider, installed: true, runnable: keyState !== 'unavailable', state: keyState, accountLabel: null, planLabel: null, apiKeySaved: saved, keyVerification: saved ? 'notSupported' : null, methods: ['apiKey'], active: null }]
    }
    const state = item.state === 'connected' ? 'account' : item.state === 'not_connected' ? 'signedOut' : 'unavailable'
    // 데스크톱 provider_auth_status 와 같은 모양(이 길에는 설치·API 키가 없다: 계정 로그인만).
    // installed: cli_unavailable 은 안 깔림만 뜻하지 않아(실행·상태 확인 실패도) 설치되지 않았다고 적지 않는다. 쓸 수 없음은 runnable 로.
    return [{ provider, installed: true, runnable: state !== 'unavailable', state, accountLabel: str(item.accountLabel) || null, planLabel: null, apiKeySaved: false, keyVerification: null, methods: ['account'], active: null }]
  })
}

// ---- 모델 목록(S62) ----
type ServerModel = { id?: unknown; label?: unknown; isDefault?: unknown; defaultEffort?: unknown; efforts?: unknown; fast?: { supported?: unknown; state?: unknown } | null }
type ServerModelList = { provider?: unknown; state?: unknown; models?: unknown }

async function readModelLists() {
  try {
    const value = await request<{ providers?: unknown }>('GET', '/providers/models')
    return Array.isArray(value?.providers) ? (value.providers as ServerModelList[]) : []
  } catch (error) {
    // 목록이 없는 옛 server: 데스크톱의 "명령 없음"과 같게(모델 고르기 없이 코덱스로).
    if (error instanceof ServerError && error.status === 404) return null
    throw error
  }
}

async function modelLists() {
  const lists = await readModelLists()
  if (lists === null) throw new Error('command ai_models not found')
  return lists.flatMap((list) => {
    const provider = str(list.provider)
    if (provider !== 'codex' && provider !== 'claude' && provider !== 'gemini') return []
    const state = list.state === 'connected' ? 'ok' : list.state === 'not_connected' ? 'not_connected' : 'unavailable'
    const models = (Array.isArray(list.models) ? (list.models as ServerModel[]) : []).flatMap((model) => {
      const id = str(model?.id)
      if (!id) return []
      const efforts = Array.isArray(model.efforts)
        ? (model.efforts as unknown[]).flatMap((item): Array<string | { id: string; label: string; description: null; special: boolean; available: boolean }> => {
            if (typeof item === 'string') return item ? [item] : []
            const entry = (item ?? {}) as { id?: unknown; label?: unknown }
            return str(entry.id) ? [{ id: str(entry.id), label: str(entry.label) || str(entry.id), description: null, special: false, available: true }] : []
          })
        : []
      const fastSupported = model.fast?.supported === true
      return [{
        id,
        label: str(model.label) || null,
        ...(model.isDefault === true ? { isDefault: true } : {}),
        defaultEffort: str(model.defaultEffort) || null,
        efforts,
        fast: fastSupported ? { supported: true, state: model.fast?.state === 'on' || model.fast?.state === 'off' ? model.fast.state : 'unavailable' } : null,
      }]
    })
    return [{ provider, state, reason: state === 'ok' ? null : str(list.state) || null, models }]
  })
}

/** 그 회사로 지금 보낼 수 있는가(S62 목록의 state). 목록이 없는 옛 server 는 코덱스만. */
async function providerSendable(provider: string) {
  const lists = await readModelLists().catch(() => null)
  if (lists === null) return provider === 'codex'
  return lists.some((list) => list.provider === provider && list.state === 'connected')
}

/** 클로드·제미나이 API 키 넣기(S55). 키는 이 함수 안에서 요청 본문으로 한 번 넘기고 끝낸다(남기지 않는다). */
async function saveProviderKey(provider: string, apiKey: string): Promise<'keySaved' | 'invalidKey' | 'storageUnavailable' | 'unavailable'> {
  if (provider !== 'claude' && provider !== 'gemini') return 'unavailable'
  if (!apiKey.trim()) return 'invalidKey'
  const keyPath = `/providers/${provider}/key/request`
  let requestId = ''
  try {
    // 다시 열 때는 대기 중인 요청을 쓴다(계약). 없으면 새로 만든다.
    const pending = await request<{ inputs?: unknown }>('GET', keyPath).catch(() => null)
    const first = Array.isArray(pending?.inputs) ? (pending.inputs as Array<{ requestId?: unknown }>).find((item) => str(item?.requestId)) : undefined
    requestId = str(first?.requestId)
    if (!requestId) requestId = str((await request<{ requestId?: unknown }>('POST', keyPath))?.requestId)
    if (!requestId) return 'unavailable'
    const result = await request<{ state?: unknown }>('POST', `/input-requests/${encodeURIComponent(requestId)}/respond`, { apiKey })
    return result?.state === 'credential_saved' ? 'keySaved' : 'unavailable'
  } catch (error) {
    if (requestId) void request('POST', `/input-requests/${encodeURIComponent(requestId)}/cancel`).catch(() => {})
    if (error instanceof ServerError && (error.code === 'invalid_key' || error.code === 'invalid_input')) return 'invalidKey'
    if (error instanceof ServerError && error.code === 'storage_unavailable') return 'storageUnavailable'
    throw error
  }
}

// ---- 첨부 올리기(S39) ----
export type UploadProgress = { uploadId: string; received: number; total: number }
const UPLOAD_CHUNK = 4 * 1024 * 1024
const uploadHashHex = (bytes: Uint8Array) => Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('')

// S167: while a backup or another large write holds the server's disk reservation, a chunk or the final
// copy is refused with 409 reservation_busy before anything is written. Sending the same request later is safe.
const UPLOAD_BUSY_RETRIES = 30
async function retryWhileReservationBusy<T>(send: () => Promise<T>): Promise<T> {
  let delay = 2000
  for (let attempt = 0; ; attempt++) {
    try {
      return await send()
    } catch (error) {
      if (!(error instanceof ServerError) || error.code !== 'reservation_busy' || attempt >= UPLOAD_BUSY_RETRIES) throw error
      await new Promise<void>((resolve) => setTimeout(resolve, delay))
      delay = Math.min(delay * 2, 30000)
    }
  }
}

async function putChunk(uploadId: string, offset: number, blob: Blob) {
  const chunkHash = uploadHashHex(sha256(new Uint8Array(await blob.arrayBuffer())))
  const response = await call(`/uploads/${encodeURIComponent(uploadId)}/chunks?offset=${offset}`, { method: 'PUT', headers: { 'Content-Type': 'application/octet-stream', 'X-Momso-Chunk-Sha256': chunkHash }, body: blob })
  let value: Record<string, unknown> | null = null
  try { value = await response.json() } catch { value = null }
  if (!response.ok) throw new ServerError(str(value?.error) || `http_${response.status}`, response.status, response.status >= 500, value?.detail ?? null)
  return typeof value?.received === 'number' ? value.received : offset + blob.size
}

/** 휴대폰에서 고른 파일 하나를 올리고 원자료 번호를 돌려준다. 끊기면 server 가 받은 곳부터 이어서(연달아 3번까지). */
export async function uploadAttachment(file: File, onProgress?: (progress: UploadProgress) => void, resumeUploadId?: string) {
  const hash = sha256.create()
  for (let offset = 0; offset < file.size; offset += UPLOAD_CHUNK) hash.update(new Uint8Array(await file.slice(offset, offset + UPLOAD_CHUNK).arrayBuffer()))
  const fileHash = uploadHashHex(hash.digest())
  const begun = resumeUploadId
    ? await request<{ uploadId?: unknown; received?: unknown; total?: unknown; sha256?: unknown }>('GET', `/uploads/${encodeURIComponent(resumeUploadId)}`)
    : await request<{ uploadId?: unknown; received?: unknown; total?: unknown; sha256?: unknown }>('POST', '/uploads', { fileName: file.name, mediaType: file.type || 'application/octet-stream', byteLength: file.size, sha256: fileHash })
  if (resumeUploadId && (begun?.total !== file.size || begun?.sha256 !== fileHash)) throw new ServerError('upload_file_mismatch', null)
  const uploadId = str(begun?.uploadId)
  if (!uploadId) throw new ServerError('invalid_response', null)
  let received = typeof begun?.received === 'number' ? begun.received : 0
  onProgress?.({ uploadId, received, total: file.size })
  let misses = 0
  while (received < file.size) {
    try {
      const from = received
      received = await retryWhileReservationBusy(() => putChunk(uploadId, from, file.slice(from, Math.min(file.size, from + UPLOAD_CHUNK))))
      misses = 0
    } catch (error) {
      const resumable = error instanceof ServerError && (error.code === 'server_unreachable' || error.code === 'offset_mismatch' || (error.status !== null && error.status >= 500))
      misses += 1
      if (!resumable || misses > 3) throw error
      const progress = await request<{ received?: unknown }>('GET', `/uploads/${encodeURIComponent(uploadId)}`)
      if (typeof progress?.received !== 'number') throw error
      received = progress.received
    }
    onProgress?.({ uploadId, received, total: file.size })
  }
  const done = await retryWhileReservationBusy(() => request<{ id?: unknown; originalFileName?: unknown; mediaType?: unknown }>('POST', `/uploads/${encodeURIComponent(uploadId)}/finish`))
  const assetId = str(done?.id)
  if (!assetId) throw new ServerError('invalid_response', null)
  return { assetId, name: str(done?.originalFileName) || file.name, mediaType: str(done?.mediaType) || file.type || null }
}

/** 코덱스에 로그인해야 하는가. server 에 AI 연결이 없거나(없는 길·읽지 못함) 로그인으로 풀 수 없으면(CLI 없음) unknown: 묻지 않고 채팅으로 간다. */
export async function aiConnection(): Promise<'connected' | 'needed' | 'unknown'> {
  try {
    const codex = (await providerStatus()).find((item) => item.provider === 'codex')
    return codex?.state === 'account' ? 'connected' : codex?.state === 'signedOut' ? 'needed' : 'unknown'
  } catch {
    return 'unknown'
  }
}

// ---- AI 로그인(기기 코드). 이 탭이 시작한 로그인 하나만 지켜본다. 세션 값은 여기 메모리에만. ----
type OwnLogin = { provider: string; session: string; shown: string; stop: () => void }
let ownLogin: OwnLogin | null = null
const loginPath = (provider: string) => `/providers/${encodeURIComponent(provider)}/login`
const sessionHeader = (login: OwnLogin) => ({ 'X-Momso-Login-Session': login.session })
type LoginView = { state?: unknown; url?: unknown; code?: unknown; expiresInSeconds?: unknown }
const loginRunning = (state: unknown) => state === 'starting' || state === 'waiting'

/** 끝난 로그인: 카드를 거두고 연결 상태가 바뀌었다고 알린다(화면은 상태를 다시 읽고, 끝난 까닭으로 한 줄 알린다). */
function endLogin(login: OwnLogin, state: string) {
  if (ownLogin !== login) return
  login.stop()
  ownLogin = null
  emit('provider-login-device-closed', { provider: login.provider, state })
  emit('provider-auth-changed', { provider: login.provider, state: state === 'connected' ? 'account' : 'signedOut', reason: state })
}

/** SSE 이벤트나 조회 결과 하나를 화면 이벤트로. 같은 코드는 한 번만 보낸다. */
function applyLogin(login: OwnLogin, view: LoginView) {
  if (ownLogin !== login) return
  const state = str(view.state)
  if (loginRunning(state)) {
    const url = str(view.url)
    const code = str(view.code)
    if (!url || !code || login.shown === code) return
    login.shown = code
    emit('provider-login-device', { provider: login.provider, url, code, expiresInSeconds: typeof view.expiresInSeconds === 'number' ? view.expiresInSeconds : null })
    return
  }
  endLogin(login, state || 'failed')
}

/** 로그인 세션 하나를 따라간다: SSE 로 받고, 끊기면 2초마다 조회. 휴대폰이 뒤로 갔다 오면(SSE 가 끊겼을 수 있다) 바로 조회. */
function followLogin(provider: string, session: string) {
  const controller = new AbortController()
  let timer = 0
  let stopped = false
  const login: OwnLogin = { provider, session, shown: '', stop: () => {} }
  const check = async () => {
    if (stopped) return
    try {
      applyLogin(login, await request<LoginView>('GET', loginPath(provider), undefined, sessionHeader(login)))
    } catch (error) {
      // 이 세션을 server 가 모른다(다시 시작됨·다른 로그인으로 바뀜): 끝난 것으로. 로그인이 풀림은 문이 알리고, 닿지 못함은 다음에 다시 본다.
      if (error instanceof ServerError && error.code === 'login_not_found') endLogin(login, 'login_not_found')
    }
  }
  const onBack = () => { if (document.visibilityState === 'visible') void check() }
  login.stop = () => {
    stopped = true
    controller.abort()
    window.clearInterval(timer)
    document.removeEventListener('visibilitychange', onBack)
    window.removeEventListener('focus', onBack)
  }
  ownLogin = login
  document.addEventListener('visibilitychange', onBack)
  window.addEventListener('focus', onBack)
  void (async () => {
    try {
      const response = await call(`${loginPath(provider)}/events`, { headers: { Accept: 'text/event-stream', ...sessionHeader(login) }, signal: controller.signal })
      if (response.ok && response.body) {
        await readSse(response.body, (name, payload) => {
          if (stopped) return true
          if (name === 'provider-login-device-closed') { endLogin(login, str(payload.state) || 'failed'); return true }
          if (name === 'provider-login-device' || name === 'provider-login-state') applyLogin(login, payload)
        })
      }
    } catch {
      // 끊김·멈춤: 아래 조회로 이어 간다.
    }
    if (stopped) return
    void check()
    timer = window.setInterval(() => void check(), 2000)
  })()
}

/** SSE 를 다시 붙이지 못할 때: GET /turns/{id} 를 1초마다 읽어 새로 온 만큼만 잇는다. */
async function followTurn(turnId: string, chatId: string, got: string, userMessageId: string | null) {
  let text = got
  for (let attempt = 0; attempt < 900; attempt += 1) {
    await catchUpTurn(turnId, attempt % 3 === 0)
    try {
      const turn = await request<{ state?: unknown; text?: unknown; error?: unknown }>('GET', `/turns/${encodeURIComponent(turnId)}`)
      if (typeof turn?.text === 'string' && turn.text.startsWith(text) && turn.text.length > text.length) {
        emit('chat-stream', { chatId, delta: turn.text.slice(text.length) })
        text = turn.text
      }
      if (turn?.state === 'completed' || turn?.state === 'failed') { finishTurn(turnId, chatId, turn.state, turn.error, userMessageId); return }
    } catch (error) {
      if (error instanceof ServerError && error.code === 'waking_timeout') {
        finishTurn(turnId, chatId, 'failed', 'stream_interrupted', userMessageId, { cause: 'waking_timeout' })
        return
      }
      // 로그인이 풀렸거나(문이 알림) server 가 이 답을 모르면 그만둔다. 닿지 못함·5xx 는 다시 해 본다.
      if (error instanceof ServerError && (isSignedOut(error) || (error.status !== null && error.status < 500))) {
        finishTurn(turnId, chatId, 'failed', error.code, userMessageId)
        return
      }
    }
    await wait(1000)
  }
  finishTurn(turnId, chatId, 'failed', 'stream_interrupted', userMessageId)
}

/** SSE 를 읽는다. 끊기면 받은 UTF-8 바이트를 after 로 주어 다시 붙는다(최대 5번). 그래도 안 되면 GET /turns 로. */
async function streamTurn(turnId: string, chatId: string, userMessageId: string | null) {
  running.set(chatId, turnId)
  let got = ''
  let bytes = 0
  for (let attempt = 0; attempt < 6; attempt += 1) {
    if (attempt > 0) await wait(Math.min(400 * attempt, 2000))
    try {
      const response = await call(`/turns/${encodeURIComponent(turnId)}/events?after=${bytes}`, { headers: { Accept: 'text/event-stream' } })
      if (!response.ok || !response.body) {
        // invalid_offset 등 이 길로는 이어 붙일 수 없다: 조회로 넘어간다.
        if (response.status >= 400 && response.status < 500) break
        continue
      }
      // 다시 붙었다: 끊긴 사이 닫히거나 새로 온 물음을 대기 목록으로 맞춘다(S32).
      if (attempt > 0) void catchUpTurn(turnId, true)
      const finished = await readSse(response.body, (name, payload) => {
        if (name === 'chat-delta' && typeof payload.delta === 'string' && payload.delta) {
          got += payload.delta
          bytes = typeof payload.offset === 'number' ? payload.offset : bytes + encoder.encode(payload.delta).length
          emit('chat-stream', { chatId, delta: payload.delta })
        } else if (name === 'chat-done') {
          finishTurn(turnId, chatId, payload.state, payload.error, userMessageId)
          return true
        } else if (name === 'agent-approval-request') {
          showApproval(payload)
        } else if (name === 'browser-human-required') {
          showHuman(payload, turnId)
        } else if (name === 'agent-approval-closed') {
          closeApproval(str(payload.requestId), str(payload.reason) || null)
        } else if (name === 'agent-input-request') {
          showInput(payload, turnId)
        } else if (name === 'agent-input-closed') {
          closeInput(str(payload.requestId), null)
        }
      })
      if (finished) return
    } catch (error) {
      if (isSignedOut(error)) { finishTurn(turnId, chatId, 'failed', error instanceof ServerError ? error.code : 'signed_out', userMessageId); return }
      if (error instanceof ServerError && error.code === 'waking_timeout') {
        finishTurn(turnId, chatId, 'failed', 'stream_interrupted', userMessageId, { cause: 'waking_timeout' })
        return
      }
      // 끊겼다: after 로 다시 붙어 본다.
    }
  }
  await followTurn(turnId, chatId, got, userMessageId)
}

// ---- 명령 ----
/** 사진과 동영상을 한 번에 받는 수(server 최대 100). */
const MEDIA_PAGE = 100

/** 사진·동영상 원본 주소(S75 셋째: GET /media/{id}/download, 같은 출처라 로그인 쿠키가 함께 간다). 갤러리의 그림·영상과 내려받기에 쓴다. */
export function serverMediaDownloadPath(assetId: string) {
  return `/media/${encodeURIComponent(assetId)}/download`
}

/** 삭제한 원자료를 한 번에 받는 수(데스크톱과 같은 40개). */
const TRASH_PAGE = 40

/** 다른 AI 채팅 가져오기의 진행 수를 읽는 간격(S83 GET /chat-imports/{id}/progress). */
const IMPORT_PROGRESS_MS = 700

/** 휴지통 항목의 사용처 수(S83 references: { chats, documents }). 문서 수를 모르면(null) 전체도 모른다. */
function referenceTotal(value: unknown) {
  const refs = value && typeof value === 'object' ? value as { chats?: unknown; documents?: unknown } : null
  if (!refs || typeof refs.chats !== 'number' || typeof refs.documents !== 'number') return null
  return refs.chats + refs.documents
}

/** 원자료 내려받기 주소(S75 둘째: GET /assets/{id}/download, 같은 출처라 로그인 쿠키가 함께 간다). */
export function serverAssetDownloadPath(assetId: string) {
  return `/assets/${encodeURIComponent(assetId)}/download`
}

async function readServerDocument(id: string) {
  return request<Record<string, unknown>>('GET', `/documents/${encodeURIComponent(id)}`)
}

/** server 문서(또는 그 판)를 데스크톱 글 머리 꼴로. 판에 없는 값은 fallback(지금 문서)에서. */
function zetHead(value: Record<string, unknown> | null, fallback?: Record<string, unknown> | null) {
  const pick = (key: string) => (value?.[key] !== undefined ? value?.[key] : fallback?.[key])
  const list = (key: string) => { const item = pick(key); return Array.isArray(item) ? item : [] }
  return {
    id: str(pick('id')), title: str(pick('title')), summary: str(pick('summary')),
    kind: str(pick('kind')) || 'note', status: str(pick('status')) || 'draft',
    created_at: str(pick('createdAt')), updated_at: str(pick('updatedAt')),
    assets: list('assets'), edges: list('edges'),
  }
}

/**
 * 스킬 목록(GET /skills): state 는 enabled|disabled|unavailable(스킬 파일이나 회사별 사본이 충돌). 데스크톱 목록 꼴로:
 * enabled 는 사용자가 켰는지(unavailable 도 끄지 않은 것), available·unavailableReason·syncState 는 충돌을 알린다. 고친 때와 회사 목록은 server 가 주지 않는다.
 */
async function listSkills() {
  const value = await request<{ skills?: unknown }>('GET', '/skills')
  return (Array.isArray(value?.skills) ? (value.skills as Array<Record<string, unknown>>) : []).flatMap((item) => {
    const name = str(item?.name)
    if (!name) return []
    const state = str(item.state)
    return [{
      name,
      title: str(item.title) || name,
      description: str(item.description),
      enabled: state !== 'disabled',
      builtin: item.builtin === true,
      updatedAt: null,
      providers: null,
      syncState: state === 'unavailable' ? 'conflict' : state === 'disabled' ? 'disabled' : 'ok',
      available: state === 'enabled',
      unavailableReason: state === 'unavailable' ? 'skill_conflict' : state === 'disabled' ? 'disabled' : null,
    }]
  })
}

/** 스킬 쓰기. server 의 skill_conflict 는 일마다 뜻이 달라(이미 있음·그사이 바뀜·기본 스킬) 데스크톱 오류 값으로 바꿔 던진다. */
async function skillCall(codes: Record<string, string>, method: string, path: string, body?: unknown) {
  try {
    await request(method, path, body)
  } catch (error) {
    if (error instanceof ServerError && error.code === 'skill_not_found') throw new ServerError('not_found', error.status)
    if (error instanceof ServerError && codes[error.code]) throw new ServerError(codes[error.code], error.status)
    throw error
  }
}

/** 고정·보관·되돌리기: 데스크톱 명령처럼 { state } 로 돌려준다(없는 방은 not_found). */
async function keepChat(chatId: string, action: 'pin' | 'archive', body: Record<string, boolean>) {
  try {
    await request('POST', `/chats/${encodeURIComponent(chatId)}/${action}`, body)
    return { state: 'ok' }
  } catch (error) {
    if (error instanceof ServerError && error.code === 'chat_not_found') return { state: 'not_found' }
    throw error
  }
}

// ---- GitHub 백업(휴대폰 설정 "연결과 백업", 계약 S17 GitHub 부분·github_bridge.rs). 데스크톱 archive_remote_* 의 꼴로 바꿔 준다. ----
/** 연결한 저장소와 백업 형편(GET /github/repository)을 데스크톱 archive_remote_status 꼴로. 저장소가 없으면 not_connected. */
async function githubStatus() {
  const value = await request<{ repository?: { nameWithOwner?: unknown } | null; account?: unknown; lastSavedAt?: unknown; backupStatus?: unknown; backupReason?: unknown }>('GET', '/github/repository')
  const repo = str(value?.repository?.nameWithOwner)
  const failed = value?.backupStatus === 'failed'
  return {
    state: repo ? 'connected' : 'not_connected',
    reason: repo && failed ? str(value?.backupReason) || 'unavailable' : null,
    // S98: GitHub 계정 이름(로그인했으면 저장소가 없어도 온다, 계정 조회가 실패하면 null)과 마지막으로 성공한 저장 때.
    repo: repo || null, visibility: repo ? 'private' : null, lastPushedAt: str(value?.lastSavedAt) || null, ahead: null, behind: null, signedInAs: str(value?.account) || null,
  }
}

/**
 * 원자료(고객 프로젝트 저장소의 암호화 백업)를 데스크톱 object_store_status 꼴로(GET /activity 의 objectBackup*, S97·S98).
 * 설치가 정한 Google Cloud 저장소라 회사는 gcp, 버킷 이름·마지막 백업 때·남은 수는 server 가 알려 주지 않는다(null).
 */
async function objectBackup() {
  const value = await request<{ objectBackupEnabled?: unknown; objectBackupStatus?: unknown; objectBackupReason?: unknown }>('GET', '/activity')
  return {
    enabled: value?.objectBackupEnabled === true,
    status: str(value?.objectBackupStatus),
    reason: str(value?.objectBackupReason) || null,
  }
}
/**
 * 백업 형편을 데스크톱 자동 백업 줄(archive_auto_backup_get)의 꼴로. server 는 늘 자동으로 저장한다(끌 수 없음).
 * 실패(failed)는 다시 시도할 때를 알려 주지 않아 이 줄에 적지 않고, 상태의 까닭 줄 하나로만 보인다(같은 말을 두 번 두지 않게).
 */
async function githubAutoBackup() {
  const value = await request<{ backupStatus?: unknown }>('GET', '/github/repository')
  const state = value?.backupStatus === 'syncing' ? 'running'
    : value?.backupStatus === 'pending' || value?.backupStatus === 'queued' ? 'pending' : 'idle'
  return { enabled: true, state, consecutiveFailures: 0, reason: null, retryInSeconds: null }
}

/** 이 탭이 시작한 GitHub 기기 로그인 하나(세션 값은 메모리에만). */
let githubLogin: { session: string; finish: (state: string) => void } | null = null
/**
 * GitHub 기기 로그인(POST /github/login → GET /github/login 을 2초마다, X-Momso-Login-Session). 코덱스와 같은 화면 이벤트
 * provider-login-device { provider: 'github', url, code, expiresInSeconds } 를 보내 설정의 코드 카드가 뜬다.
 * 연결되면 끝나고, 취소면 cancelled, 시간이 지나거나 실패하면 signed_out 오류로 끝난다("GitHub에 로그인해 주세요").
 */
async function githubDeviceLogin() {
  githubLogin?.finish('cancelled')
  const started = await request<{ sessionId?: unknown }>('POST', '/github/login')
  const session = str(started?.sessionId)
  if (!session) throw new ServerError('unavailable', null)
  return new Promise<void>((resolve, reject) => {
    let shown = ''
    let done = false
    const header = { 'X-Momso-Login-Session': session }
    const finish = (state: string) => {
      if (done) return
      done = true
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', onBack)
      if (githubLogin?.session === session) githubLogin = null
      emit('provider-login-device-closed', { provider: 'github', state })
      if (state === 'connected') resolve()
      else reject(new ServerError(state === 'cancelled' ? 'cancelled' : 'signed_out', null))
    }
    const check = async () => {
      if (done) return
      try {
        const view = await request<{ state?: unknown; url?: unknown; code?: unknown; expiresInSeconds?: unknown }>('GET', '/github/login', undefined, header)
        const state = str(view?.state)
        if (state !== 'pending') { finish(state || 'failed'); return }
        const url = str(view?.url)
        const code = str(view?.code)
        if (url && code && code !== shown) {
          shown = code
          emit('provider-login-device', { provider: 'github', url, code, expiresInSeconds: typeof view?.expiresInSeconds === 'number' ? view.expiresInSeconds : null })
        }
      } catch (error) {
        if (error instanceof ServerError && error.status === 404) finish('failed')
      }
    }
    // 휴대폰에서 GitHub 페이지에 다녀오면 바로 다시 본다.
    const onBack = () => { if (document.visibilityState === 'visible') void check() }
    const timer = window.setInterval(() => void check(), 2000)
    document.addEventListener('visibilitychange', onBack)
    githubLogin = { session, finish }
    void check()
  })
}

function missing(cmd: string): never {
  // Tauri 의 없는 명령과 같은 글(화면의 isMissingCommand 가 그 기능이 없는 뒤쪽으로 본다).
  throw new Error(`command ${cmd} not found`)
}

export async function networkInvoke<T>(cmd: string, args?: InvokeArgs): Promise<T> {
  const input = (args && typeof args === 'object' && !Array.isArray(args) && !(args instanceof ArrayBuffer) && !(args instanceof Uint8Array) ? args : {}) as Record<string, unknown>
  switch (cmd) {
    case 'chat_list':
      return (await listChats()) as T
    case 'chat_list_page':
      return { items: typeof input.offset === 'number' && input.offset > 0 ? [] : await listChats(), hasMore: false, nextOffset: null } as T
    case 'chat_create':
      return (await createChat('')) as T
    case 'chat_create_with_message': {
      const chat = await createChat(Array.from(str(input.text).trim()).slice(0, 40).join(''))
      // 첫 말은 아직 server 에 없다. 화면은 이 번호로 chat_send(retryMessageId) 를 부르고, 그때 turns 로 보낸다.
      return { chat, messageId: `local-${chat.id}` } as T
    }
    case 'chat_read':
      return (await readDocument(str(input.id))) as T
    case 'chat_read_page':
      return { ...(await readDocument(str(input.id))), nextCursor: null } as T
    // ---- 채팅방 이름·찾기·고정·보관(S75: server 에 있는 길을 데스크톱 명령과 같은 꼴로) ----
    case 'chat_rename':
      await request('PATCH', `/chats/${encodeURIComponent(str(input.id))}`, { title: str(input.title) })
      return null as T
    case 'chat_search': {
      // 제목·말에 그 글이 든 방, 최근 고친 순 100개까지. 말 둘레 글(snippet)은 server 가 주지 않는다.
      const value = await request<{ chats?: unknown }>('GET', `/chats/search?q=${encodeURIComponent(str(input.query))}`)
      return (Array.isArray(value?.chats) ? (value.chats as ServerChat[]) : []).flatMap((chat) => { const entry = readEntry(chat); return entry ? [{ ...entry, snippet: null }] : [] }) as T
    }
    case 'chat_pin':
      return (await keepChat(str(input.chatId), 'pin', { pinned: input.pinned === true })) as T
    case 'chat_archive':
      return (await keepChat(str(input.chatId), 'archive', { archived: true })) as T
    case 'chat_restore':
      return (await keepChat(str(input.chatId), 'archive', { archived: false })) as T
    // 채팅방 지우기(S75 둘째: DELETE /chats/{id}). 답하는 중이면 server 가 409 turn_in_progress → 데스크톱처럼 busy.
    case 'chat_delete':
      try {
        await request('DELETE', `/chats/${encodeURIComponent(str(input.chatId))}`)
        return { state: 'deleted' } as T
      } catch (error) {
        if (error instanceof ServerError && error.code === 'turn_in_progress') return { state: 'busy' } as T
        if (error instanceof ServerError && error.code === 'chat_not_found') return { state: 'not_found' } as T
        throw error
      }
    // ---- 채팅방 묶음(S75 둘째: /chat-groups, PUT /chats/{id}/group). 데스크톱 chat_group_* 와 같은 꼴 ----
    case 'chat_group_list': {
      const value = await request<{ groups?: unknown }>('GET', '/chat-groups')
      return (Array.isArray(value?.groups) ? (value.groups as Array<Record<string, unknown>>) : []).flatMap((group) => (str(group?.id)
        ? [{ id: str(group.id), name: str(group.name), order: typeof group.order === 'number' ? group.order : 0, chatCount: typeof group.chatCount === 'number' ? group.chatCount : 0 }]
        : [])) as T
    }
    case 'chat_group_create': {
      const made = await request<{ id?: unknown; name?: unknown }>('POST', '/chat-groups', { name: str(input.name) })
      return { id: str(made?.id), name: str(made?.name), order: 0, chatCount: 0 } as T
    }
    case 'chat_group_rename':
      await request('PATCH', `/chat-groups/${encodeURIComponent(str(input.id))}`, { name: str(input.name) })
      return null as T
    case 'chat_group_reorder':
      await request('POST', '/chat-groups/reorder', { ids: Array.isArray(input.ids) ? input.ids.filter((id): id is string => typeof id === 'string') : [] })
      return null as T
    case 'chat_group_remove':
      await request('DELETE', `/chat-groups/${encodeURIComponent(str(input.id))}`)
      return null as T
    case 'chat_set_group':
      await request('PUT', `/chats/${encodeURIComponent(str(input.chatId))}/group`, { groupId: str(input.groupId) || null })
      return null as T
    case 'chat_archived_list': {
      const value = await request<{ chats?: unknown }>('GET', '/chats/archived')
      return (Array.isArray(value?.chats) ? (value.chats as ServerChat[]) : []).flatMap((chat) => {
        const entry = readEntry(chat)
        return entry ? [{ id: entry.id, title: entry.title, groupId: entry.groupId, updatedAt: entry.updatedAt, archivedAt: str(chat.archivedAt) || null, provider: null }] : []
      }) as T
    }
    case 'chat_send': {
      const chatId = str(input.chatId)
      const provider = str(input.provider) || 'codex'
      if (provider !== 'codex' && provider !== 'claude' && provider !== 'gemini') return { status: 'unavailable' } as T
      // 보낼 수 있는 곳 미리 알기(빈 방 번호): 연결된 회사면 데스크톱 뒤쪽처럼 저장에서 실패한다(아무것도 보내지 않는다).
      if (!chatId) {
        if (!(await providerSendable(provider))) return { status: 'unavailable' } as T
        throw new ServerError('invalid_chat', null)
      }
      const again = str(input.retryMessageId)
      let sent: { turnId?: unknown; userMessageId?: unknown }
      if (again && !again.startsWith('local-')) {
        // 다시 보내기: 실패한 그 말의 턴을 server 가 다시 돌린다(같은 말이 두 번 남지 않음). 모르는 말이면 다시 보낼 수 없다.
        const failed = failedTurns.get(again)
        if (!failed) return { status: 'retry_unavailable' } as T
        try {
          sent = await request('POST', `/turns/${encodeURIComponent(failed)}/retry`)
        } catch (error) {
          if (error instanceof ServerError && (error.code === 'retry_unavailable' || error.code === 'turn_not_failed' || error.code === 'turn_not_found')) return { status: 'retry_unavailable' } as T
          throw error
        }
        failedTurns.delete(again)
      } else {
        const attachments = Array.isArray(input.attachments) ? input.attachments.filter((id): id is string => typeof id === 'string' && Boolean(id)).map((assetId) => ({ assetId })) : []
        const model = str(input.model)
        const effort = str(input.effort)
        const skill = str(input.skill)
        // 사진과 동영상의 "다시 만들기"(S75 셋째): 같은 채팅방의 생성물을 바탕으로 새 판. 클로드·제미나이는 server 가 media_generation_unavailable.
        const regenerateOf = str(input.regenerateOf)
        try {
          sent = await request('POST', `/chats/${encodeURIComponent(chatId)}/turns`, {
            text: str(input.text),
            permissionMode: codexPermission(),
            provider,
            ...(model ? { model } : {}),
            ...(effort ? { effort } : {}),
            ...(typeof input.fast === 'boolean' ? { fast: input.fast } : {}),
            ...(attachments.length > 0 ? { attachments } : {}),
            // 입력 칸에서 고른 스킬(S75 둘째). 꺼졌거나 없는 스킬이면 server 가 409 skill_unavailable(말을 저장하지 않음) → 데스크톱과 같은 값.
            ...(skill ? { skill } : {}),
            ...(regenerateOf ? { regenerateOf } : {}),
          })
        } catch (error) {
          if (error instanceof ServerError && error.code === 'fast_unavailable') return { status: 'fast_unavailable' } as T
          if (error instanceof ServerError && error.code === 'skill_unavailable') return { status: 'skill_unavailable' } as T
          throw error
        }
      }
      const turnId = str(sent?.turnId)
      if (!turnId) throw new ServerError('invalid_response', null)
      const userMessageId = str(sent?.userMessageId) || null
      void streamTurn(turnId, chatId, userMessageId)
      return { status: 'started', userMessageId } as T
    }
    case 'provider_auth_status':
      return (await providerStatus()) as T
    case 'ai_models':
      return (await modelLists()) as T
    case 'provider_api_key_set':
      return (await saveProviderKey(str(input.provider), str(input.apiKey))) as T
    case 'provider_login_start': {
      const provider = str(input.provider)
      // 이 단계의 server 는 코덱스 로그인만 있다.
      if (provider !== 'codex') return 'unavailable' as T
      // 지켜보던 로그인이 있으면(코드 다시 받기) 놓는다. 이전 로그인은 새로 시작할 때 server 가 취소한다.
      ownLogin?.stop()
      ownLogin = null
      const started = await request<{ sessionId?: unknown }>('POST', loginPath(provider))
      const session = str(started?.sessionId)
      if (!session) return 'unavailable' as T
      followLogin(provider, session)
      return 'inProgress' as T
    }
    case 'provider_login_cancel': {
      const provider = str(input.provider)
      if (provider === 'github') {
        const login = githubLogin
        if (login) await request('POST', '/github/login/cancel', undefined, { 'X-Momso-Login-Session': login.session }).catch(() => {})
        login?.finish('cancelled')
        return 'cancelled' as T
      }
      const login = ownLogin
      if (login && login.provider === provider) {
        login.stop()
        ownLogin = null
        await request('POST', `${loginPath(provider)}/cancel`, undefined, sessionHeader(login)).catch(() => {})
      }
      emit('provider-login-device-closed', { provider, state: 'cancelled' })
      return 'cancelled' as T
    }
    case 'permission_modes_available':
      if (input.provider !== 'codex') return missing(cmd)
      return CODEX_PERMISSION_MODES as T
    case 'permission_mode_get':
      if (input.provider !== 'codex') return missing(cmd)
      return codexPermission() as T
    case 'permission_mode_set': {
      const mode = str(input.mode)
      if (input.provider !== 'codex' || !CODEX_PERMISSION_MODES.some((item) => item.id === mode)) throw new ServerError('invalid_permission_mode', null)
      try { localStorage.setItem(PERMISSION_KEY, mode) } catch { throw new ServerError('storage_error', null) }
      return null as T
    }
    case 'agent_approval_respond': {
      const requestId = str(input.requestId)
      const open = openApprovals.get(requestId)
      if (open) open.answering = true
      try {
        await request('POST', `/approvals/${encodeURIComponent(requestId)}/respond`, { choice: str(input.choice) })
      } catch (error) {
        if (open) open.answering = false
        // 그사이 닫혔다(5분 지남·멈춤·server 다시 시작): 화면은 그 자리에 "닫혔습니다" 한 줄.
        if (error instanceof ServerError && error.code === 'approval_closed') {
          openApprovals.delete(requestId)
          emit('agent-approval-closed', { requestId, chatId: open?.chatId ?? null, reason: null })
          return null as T
        }
        throw error
      }
      openApprovals.delete(requestId)
      emit('agent-approval-closed', { requestId, chatId: open?.chatId ?? null, reason: 'responded' })
      return null as T
    }
    case 'setup_secret_submit': {
      // 휴대폰 몸소의 입력 물음(S32). 값은 넘기기만 하고 어디에도 담지 않는다.
      const requestId = str(input.requestId)
      const values = (input.input && typeof input.input === 'object' ? input.input : {}) as Record<string, unknown>
      const kind = openInputs.get(requestId)?.kind
      await answerInput(requestId, kind === 'one_time_code' ? { code: str(values.code) } : { label: str(values.label), username: str(values.username), password: str(values.password) })
      return null as T
    }
    case 'agent_setup_secret_cancel':
      await cancelInput(str(input.requestId))
      return null as T
    case 'saved_login_select':
      try {
        await answerInput(str(input.requestId), { selectionId: str(input.loginId) })
      } catch (error) {
        // 이미 닫힘: 데스크톱 고르기와 같은 값(화면은 "닫혔습니다" 한 줄).
        if (error instanceof ServerError && error.code === 'request_closed') throw new ServerError('unavailable', 409)
        throw error
      }
      return null as T
    case 'saved_login_cancel':
      await cancelInput(str(input.requestId))
      return null as T
    case 'server_documents_list': {
      const value = await request<{ documents?: unknown }>('GET', '/documents')
      return (Array.isArray(value?.documents) ? value.documents : []).flatMap((item) => {
        const p = (item ?? {}) as Record<string, unknown>
        return str(p.id) ? [{ id: str(p.id), title: str(p.title), folder: str(p.folder), updatedAt: str(p.updatedAt) }] : []
      }) as T
    }
    case 'server_document_read': {
      const value = await request<Record<string, unknown>>('GET', `/documents/${encodeURIComponent(str(input.id))}`)
      return { id: str(value?.id), title: str(value?.title), folder: str(value?.folder), updatedAt: str(value?.updatedAt), summary: str(value?.summary), body: str(value?.body), revision: str(value?.revision) } as T
    }
    // 문서 고치기(S37·S75): 읽을 때 받은 revision 을 보내고, 그사이 바뀌었으면 server 가 409 conflict(화면은 데스크톱처럼 묻는다).
    case 'server_document_save': {
      const saved = await request<{ id?: unknown; revision?: unknown }>('PUT', `/documents/${encodeURIComponent(str(input.id))}`, {
        title: str(input.title), summary: str(input.summary), body: str(input.body), folder: str(input.folder), revision: str(input.revision),
      })
      return { id: str(saved?.id), revision: str(saved?.revision) } as T
    }
    // 문서 폴더(S75): 데스크톱 폴더 정본의 모든 경로(빈 폴더·위 폴더 포함)와 만들기(전체 경로).
    case 'server_document_folders': {
      const value = await request<{ folders?: unknown }>('GET', '/document-folders')
      return (Array.isArray(value?.folders) ? value.folders : []).filter((path): path is string => typeof path === 'string' && Boolean(path)) as T
    }
    case 'server_document_folder_create':
      await request('POST', '/document-folders', { folder: str(input.folder) })
      return null as T
    // 폴더 이름 바꾸기(S75 둘째: POST /document-folders/rename { from, to }, 전체 경로).
    case 'server_document_folder_rename':
      await request('POST', '/document-folders/rename', { from: str(input.from), to: str(input.to) })
      return null as T
    // ---- 문서 읽기·이전 판·되돌리기(S75 둘째): 데스크톱 zet_read·zet_history·zet_read_version·zet_restore_version 과 같은 꼴(데스크톱 "이전 버전" 화면을 그대로 쓴다) ----
    // ---- 찾기 창의 문서·원자료 탭(S75 둘째): 데스크톱 zet_list·source_search 꼴로 ----
    case 'zet_list': {
      // server 목록에는 요약·종류·상태·만든 때가 없다(찾기는 제목으로만 맞는다).
      const value = await request<{ documents?: unknown }>('GET', '/documents')
      return (Array.isArray(value?.documents) ? (value.documents as Array<Record<string, unknown>>) : []).flatMap((item) => (str(item?.id)
        ? [{ id: str(item.id), title: str(item.title), summary: '', kind: 'note', status: 'draft', createdAt: '', updatedAt: str(item.updatedAt), folder: str(item.folder) }]
        : [])) as T
    }
    case 'source_search': {
      // 원자료 이름 찾기(GET /assets/search?q=, 서버에 올린 원본의 이름만). 이어진 문서는 server 가 주지 않는다.
      const value = await request<{ items?: unknown }>('GET', `/assets/search?q=${encodeURIComponent(str(input.query))}`)
      return (Array.isArray(value?.items) ? (value.items as Array<Record<string, unknown>>) : []).flatMap((item) => (str(item?.id)
        ? [{ id: str(item.id), originalFileName: str(item.originalFileName) || str(item.id), mediaType: str(item.mediaType) || null, size: typeof item.byteLength === 'number' ? item.byteLength : 0, createdAt: '', linkedDocumentIds: [] }]
        : [])) as T
    }
    case 'zet_read': {
      const value = await readServerDocument(str(input.id))
      return { head: zetHead(value), body: str(value?.body), contentHash: str(value?.revision) } as T
    }
    case 'zet_history': {
      // server 는 Git 에 저장된 판의 번호와 때만 준다(그때의 제목·같이 저장된 문서 수는 없음).
      const value = await request<{ versions?: unknown }>('GET', `/documents/${encodeURIComponent(str(input.id))}/history`)
      return (Array.isArray(value?.versions) ? (value.versions as Array<Record<string, unknown>>) : []).flatMap((item) => (str(item?.revision)
        ? [{ revision: str(item.revision), time: str(item.time), title: null, otherZets: 0 }]
        : [])) as T
    }
    case 'zet_read_version': {
      const id = str(input.id)
      const [value, current] = await Promise.all([
        request<Record<string, unknown>>('GET', `/documents/${encodeURIComponent(id)}/versions/${encodeURIComponent(str(input.revision))}`),
        readServerDocument(id),
      ])
      // 판에는 종류·상태·때가 없어 지금 문서의 값으로 채운다(데스크톱이 "저장하지 않은 내용"을 견줄 때 제목·요약·본문·연결만 다르게 보이도록).
      return { head: zetHead(value, current), body: str(value?.body), headUnreadable: false } as T
    }
    case 'zet_restore_version': {
      const id = str(input.id)
      const current = await readServerDocument(id)
      try {
        await request('POST', `/documents/${encodeURIComponent(id)}/restore`, { revision: str(input.revision), expectedRevision: str(current?.revision) })
      } catch (error) {
        // 저장하지 않은 내용(409 unsaved_changes): server 에는 그 내용을 버리고 되돌리는 길이 없어, 데스크톱의 "그래도 되돌리기" 물음 대신
        // 데스크톱 뒤쪽과 같은 글을 되돌리기 창에 그대로 보인다(Error 로 던져 isUnsavedError 의 물음으로 가지 않게).
        if (error instanceof ServerError && error.code === 'unsaved_changes') throw new Error('저장하지 않은 내용이 있습니다')
        throw error
      }
      return null as T
    }
    // ---- 용어 사전·사람 이름(S75): 데스크톱 명령과 같은 입력·목록 꼴. revision 이 낡았으면 server 가 409 dictionary_changed ----
    case 'yoga_terms_list':
      return (await request<unknown>('GET', '/dictionary')) as T
    case 'yoga_term_edit':
      return str((await request<{ id?: unknown }>('POST', '/dictionary/terms', input.input ?? {}))?.id) as T
    case 'yoga_person_edit':
      return str((await request<{ id?: unknown }>('POST', '/dictionary/people', input.input ?? {}))?.id) as T
    case 'yoga_term_delete':
      try {
        return str((await request<{ result?: unknown }>('DELETE', '/dictionary/terms', { id: str(input.id), expectedRevision: str(input.expectedRevision) }))?.result) as T
      } catch (error) {
        // 기본 용어는 지울 수 없다(데스크톱 명령처럼 'base_term' 을 돌려준다. 화면은 끄기만 권한다).
        if (error instanceof ServerError && error.code === 'base_term') return 'base_term' as T
        throw error
      }
    case 'yoga_person_delete':
      await request('DELETE', '/dictionary/people', { id: str(input.id), expectedRevision: str(input.expectedRevision) })
      return null as T
    // ---- 스킬(S75): 데스크톱 skill_* 명령과 같은 꼴. server 의 오류 값을 데스크톱 오류 값으로 바꿔 화면의 글이 같게 ----
    case 'skill_list':
      return (await listSkills()) as T
    case 'skill_read': {
      const name = encodeURIComponent(str(input.name))
      const [doc, list] = await Promise.all([request<Record<string, unknown>>('GET', `/skills/${name}`), listSkills()])
      const entry = list.find((item) => item.name === str(doc?.name))
      if (!entry) throw new ServerError('not_found', 404)
      return { ...entry, title: str(doc?.title), description: str(doc?.description), body: str(doc?.body), contentHash: str(doc?.revision) } as T
    }
    case 'skill_create':
      await skillCall({ skill_conflict: 'already_exists', invalid_skill: 'invalid_format' }, 'POST', '/skills', { name: str(input.name), title: str(input.title), description: str(input.description), body: str(input.body) })
      return { status: 'ok' } as T
    case 'skill_update':
      await skillCall({ skill_conflict: 'conflict', invalid_skill: 'invalid_format' }, 'PUT', `/skills/${encodeURIComponent(str(input.name))}`, { revision: str(input.expectedHash), changes: input.changes ?? {} })
      return { status: 'ok' } as T
    case 'skill_set_enabled':
      await skillCall({}, 'POST', `/skills/${encodeURIComponent(str(input.name))}/enabled`, { enabled: input.enabled === true })
      return { status: 'ok' } as T
    case 'skill_remove':
      await skillCall({ skill_conflict: 'builtin' }, 'DELETE', `/skills/${encodeURIComponent(str(input.name))}`)
      return { status: 'ok' } as T
    case 'saved_login_list': {
      const value = await request<{ logins?: unknown }>('GET', '/saved-logins')
      return (Array.isArray(value?.logins) ? value.logins : []) as T
    }
    case 'saved_login_delete':
      await request('DELETE', `/saved-logins/${encodeURIComponent(str(input.id))}`)
      return null as T
    case 'saved_login_set': {
      // 새로 저장(POST) 또는 고치기(PUT, 바꿀 값만. 비밀번호를 빼면 그대로). 비밀번호는 넘기기만 하고 어디에도 담지 않는다.
      const login = (input.input && typeof input.input === 'object' ? input.input : {}) as Record<string, unknown>
      if (str(login.id)) {
        const change: Record<string, unknown> = {}
        for (const key of ['siteUrl', 'label', 'username', 'password', 'loginPath']) if (typeof login[key] === 'string') change[key] = login[key]
        for (const key of ['askBeforeUse', 'defaultForPath', 'favorite']) if (typeof login[key] === 'boolean') change[key] = login[key]
        if (change.password === '') delete change.password
        const saved = await request<{ login?: unknown }>('PUT', `/saved-logins/${encodeURIComponent(str(login.id))}`, change)
        return (saved?.login ?? null) as T
      }
      const body: Record<string, unknown> = {
        siteUrl: str(login.siteUrl) || str(login.origin),
        label: str(login.label),
        username: str(login.username),
        password: str(login.password),
      }
      if (typeof login.askBeforeUse === 'boolean') body.askBeforeUse = login.askBeforeUse
      if (str(login.loginPath)) body.loginPath = str(login.loginPath)
      if (typeof login.defaultForPath === 'boolean') body.defaultForPath = login.defaultForPath
      const saved = await request<{ login?: unknown }>('POST', '/saved-logins', body)
      return (saved?.login ?? null) as T
    }
    // ---- S75 셋째: 사용자 정보·AI 연결 해제·다른 AI 채팅 가져오기 ----
    case 'user_profile_get': {
      const value = await request<{ name?: unknown; about?: unknown; style?: unknown; updatedAt?: unknown }>('GET', '/profile')
      return { name: str(value?.name), about: str(value?.about), style: str(value?.style), updatedAt: str(value?.updatedAt) || null } as T
    }
    case 'user_profile_set':
      // 규칙 파일을 따로 고쳤으면 409 profile_conflict(덮지 않음) → 화면은 저장하지 못함 한 줄.
      await request('PUT', '/profile', { name: str(input.name), about: str(input.about), style: str(input.style) })
      return null as T
    case 'provider_logout':
    case 'provider_api_key_clear': {
      // 연결 해제(POST /providers/{provider}/disconnect → { state: not_connected }). 코덱스는 서버 계정 로그아웃, 클로드·제미나이는 보관한 키만 지운다.
      // 답하는 중(turn_in_progress)·코덱스 로그인 중(login_in_progress)이면 409 → 화면의 "로그아웃하지 못했습니다"·"키를 삭제하지 못했습니다".
      const provider = str(input.provider)
      await request('POST', `/providers/${encodeURIComponent(provider)}/disconnect`)
      emit('provider-auth-changed', { provider, state: 'signedOut', reason: null })
      return (cmd === 'provider_logout' ? 'signedOut' : 'keyCleared') as T
    }
    // ---- GitHub 백업(휴대폰 설정): 상태·연결·업로드·자동 백업 형편. 저장소 만들기·다운로드·연결 해제는 server 에 없다(화면이 단추를 두지 않음). ----
    case 'archive_remote_status':
      return (await githubStatus()) as T
    case 'archive_remote_connect': {
      // 저장소를 고른다. GitHub 에 로그인하지 않았으면(not_connected) 기기 로그인을 마친 뒤 다시 고른다.
      const body = { nameWithOwner: str(input.repo) }
      // 없거나 비공개가 아닌 저장소(repository_not_found)는 데스크톱의 같은 까닭 값으로("저장소를 찾지 못했습니다").
      const choose = () => request('POST', '/github/repository', body).catch((error) => {
        throw error instanceof ServerError && error.code === 'repository_not_found' ? new ServerError('repository_unavailable', error.status) : error
      })
      try {
        await choose()
      } catch (error) {
        if (!(error instanceof ServerError) || error.code !== 'not_connected') throw error
        await githubDeviceLogin()
        await choose()
      }
      return (await githubStatus()) as T
    }
    case 'archive_remote_create': {
      // S97: GitHub 로그인이 승인되면 server 가 momso-<계정> 비공개 저장소를 만들거나 찾아 연결한다. 데스크톱 "저장소 만들기"와 같은 뜻.
      // 로그인 전이면(not_connected) 기기 로그인부터. 만들기는 같은 경로를 다시 해도 이미 고른 저장소를 그대로 준다.
      const make = () => request<{ repository?: { nameWithOwner?: unknown } | null }>('POST', '/github/repository/create')
      let made: { repository?: { nameWithOwner?: unknown } | null } | null
      try {
        try {
          made = await make()
        } catch (error) {
          if (!(error instanceof ServerError) || error.code !== 'not_connected') throw error
          await githubDeviceLogin()
          made = await make()
        }
      } catch (error) {
        // 데스크톱 저장소 만들기의 결과 값으로(같은 글): 같은 이름의 공개 저장소, 만들지 못함, 비공개인지 확인하지 못함.
        const known: Record<string, string> = { repository_not_private: 'exists_public', repository_create_failed: 'create_failed', repository_create_unverified: 'created_unverified' }
        if (error instanceof ServerError && known[error.code]) return { state: known[error.code], repo: null } as T
        throw error
      }
      return { state: 'created', repo: str(made?.repository?.nameWithOwner) || null } as T
    }
    case 'archive_remote_disconnect':
      // S98: server 에 저장된 GitHub 연결과 자격 증명을 지운다(GitHub 의 저장소와 자료는 그대로). 로그인 중이면 409 login_in_progress.
      await request('POST', '/github/disconnect')
      return (await githubStatus()) as T
    case 'object_store_status': {
      const value = await objectBackup()
      return { state: value.enabled ? 'connected' : 'not_connected', provider: value.enabled ? 'gcp' : null, bucket: null, lastBackupAt: null, pendingCount: null, pendingBytes: null } as T
    }
    case 'object_store_auto_upload_get': {
      // 원자료 백업은 server 가 늘 자동으로 한다(끌 수 없음). 실패하면 server 가 30초 안에 다시 하므로 데스크톱의 "곧 다시 시도" 줄과 같다.
      const value = await objectBackup()
      const state = !value.enabled ? 'not_connected' : value.status === 'syncing' ? 'running' : value.status === 'pending' ? 'pending' : value.status === 'failed' ? 'retry_pending' : 'idle'
      return { enabled: true, state, consecutiveFailures: state === 'retry_pending' ? 1 : 0, reason: state === 'retry_pending' ? value.reason : null, retryInSeconds: null } as T
    }
    case 'archive_remote_push':
      // GitHub 로그인이 풀렸으면(not_connected, 갱신 토큰도 만료) 먼저 기기 로그인을 다시 한 뒤 저장을 예약한다.
      try {
        await request('GET', '/github/repositories')
      } catch (error) {
        if (!(error instanceof ServerError) || error.code !== 'not_connected') throw error
        await githubDeviceLogin()
      }
      await request('POST', '/github/sync')
      return (await githubStatus()) as T
    case 'archive_auto_backup_get':
      return (await githubAutoBackup()) as T
    case 'chat_import_export_preview':
      // path 자리에 휴대폰에서 올린 파일의 원자료 번호(ChatImportSettings). 결과는 데스크톱과 같은 판독기의 같은 꼴.
      return (await request('POST', '/chat-imports/preview', { assetId: str(input.path), provider: str(input.provider) })) as T
    case 'chat_import_export_progress': {
      // 끝나면 센 수, 그만두면 409 cancelled. 가져오는 동안 GET /chat-imports/{id}/progress 의 processed 를 데스크톱처럼
      // chat-import-progress 로 보낸다(S83). 진행값은 server 를 다시 켜면 없어지므로 읽지 못하면 그냥 넘어간다.
      const requestId = str(input.requestId)
      let done = false
      const watch = (async () => {
        while (!done) {
          await new Promise((resolve) => setTimeout(resolve, IMPORT_PROGRESS_MS))
          if (done) break
          const value = await request<{ processed?: unknown }>('GET', `/chat-imports/${encodeURIComponent(requestId)}/progress`).catch(() => null)
          if (!done && typeof value?.processed === 'number') emit('chat-import-progress', { requestId, processed: value.processed })
        }
      })()
      try {
        return (await request('POST', '/chat-imports', { assetId: str(input.path), provider: str(input.provider), requestId })) as T
      } finally {
        done = true
        void watch
      }
    }
    case 'chat_import_export_cancel':
      await request('POST', `/chat-imports/${encodeURIComponent(str(input.requestId))}/cancel`)
      return null as T
    // ---- S75 셋째·S83: 원자료 휴지통·되돌리기·완전히 삭제 ----
    case 'source_asset_deleted_list': {
      // 데스크톱은 { before } 로 이어 받는다. 여기서는 그 자리에 다음 offset 을 담아 둔다(화면은 받은 값을 그대로 돌려준다).
      const before = input.before && typeof input.before === 'object' ? input.before as Record<string, unknown> : null
      const offset = typeof before?.offset === 'number' ? before.offset : 0
      const value = await request<{ items?: unknown }>('GET', `/assets/trash?offset=${offset}&limit=${TRASH_PAGE}`)
      const rows = (Array.isArray(value?.items) ? value.items : []) as Array<Record<string, unknown>>
      const items = rows.filter((row) => str(row.assetId)).map((row) => ({
        id: str(row.assetId), originalFileName: str(row.name) || null, deletedAt: str(row.deletedAt) || null,
        deletionState: 'trashed', canRestore: true, canPurge: row.canPurge === true,
        // 사용처 = 채팅방 수 + 문서 수(S83 references). 문서를 읽지 못하면 documents 가 null 이라 수를 모른다(null).
        referenceCount: referenceTotal(row.references),
        backupCopiesRemain: row.backupCopiesRemain === true,
      }))
      const last = items[items.length - 1]
      const more = rows.length >= TRASH_PAGE && Boolean(last)
      return { items, hasMore: more, nextCursor: more ? { deletedAt: last.deletedAt ?? '', assetId: last.id, offset: offset + rows.length } : null } as T
    }
    case 'source_asset_delete_preview':
      // server 는 사용처를 미리 알려 주지 않는다. 묻는 창은 사용처 없이 "되돌릴 수 있습니다"만(usesUnknown).
      return { assetId: str(input.assetId), originalFileName: null, state: 'active', documents: [], receipts: [], chats: [], recordVersions: [], planDigest: '', usesUnknown: true } as T
    case 'source_asset_trash':
      await request('POST', `/assets/${encodeURIComponent(str(input.assetId))}/trash`)
      return { state: 'trashed', assetId: str(input.assetId), retainUntil: null } as T
    case 'source_asset_restore_trashed':
      await request('POST', `/assets/${encodeURIComponent(str(input.assetId))}/restore`)
      return { state: 'active', assetId: str(input.assetId), retainUntil: null } as T
    case 'source_asset_purge': {
      // S83: 몸소 서버의 원본을 지우고 다음 백업·복원에서 뺀다. 이미 올라간 예전 백업 세대에는 남을 수 있다(backupCopiesRemain).
      // 사용 중이면 409 asset_in_use, 백업 작업자가 없으면 backup_purge_unavailable(화면은 "완전히 삭제하지 못했습니다").
      const value = await request<{ backupCopiesRemain?: unknown }>('POST', `/assets/${encodeURIComponent(str(input.assetId))}/purge`)
      const remain = value?.backupCopiesRemain === true
      return { state: remain ? 'partial' : 'purged', localRemaining: [], remoteRemaining: [], removedRemoteKeys: [], backupCopiesRemain: remain } as T
    }
    // ---- S75 셋째: 사진과 동영상(생성 기록과 판) ----
    case 'media_list': {
      // 데스크톱 media_list 는 전부를 한 번에 준다. server 는 쪽마다 100개라 끝까지(10쪽까지) 이어 받는다. 같은 묶음은 최신 판 하나와 판 수만 온다.
      const rows: Array<Record<string, unknown>> = []
      for (let offset = 0; offset < MEDIA_PAGE * 10; offset += MEDIA_PAGE) {
        const value = await request<{ items?: unknown }>('GET', `/media?offset=${offset}&limit=${MEDIA_PAGE}`)
        const page = (Array.isArray(value?.items) ? value.items : []) as Array<Record<string, unknown>>
        rows.push(...page)
        if (page.length < MEDIA_PAGE) break
      }
      const titles = new Map((await listChats().catch(() => [])).map((chat) => [chat.id, chat.title]))
      const count = (value: unknown) => (typeof value === 'number' ? value : null)
      return rows.flatMap((row) => {
        const id = str(row.assetId)
        const kind = row.kind === 'image' || row.kind === 'video' ? row.kind : null
        // 사람이 올린 사진·동영상도 날짜가 온다(S83: 올린 때, 옛 파일은 파일 수정 시각). 그래도 날짜를 읽지 못한 것(null)은 날짜별 갤러리에 놓을 자리가 없어 뺀다.
        const createdAt = str(row.createdAt)
        if (!id || !kind || !createdAt) return []
        const chatId = row.chatAvailable === true ? str(row.chatId) : ''
        const how = row.versionHow === 'regenerated' || row.versionHow === 'edited' ? row.versionHow : null
        // 이름: 데스크톱처럼 올린 파일은 파일 이름. 생성물은 지금처럼 비워 요청한 말이 보이게 한다.
        const uploaded = !str(row.lineageId)
        // 동영상의 첫 장면 그림(S83 thumbnailUrl, 서버가 처음 볼 때 만든다).
        const poster = kind === 'video' && row.thumbnailState === 'on_demand' ? str(row.thumbnailUrl) : ''
        return [{
          id, kind, title: uploaded ? str(row.name) || null : null, createdAt, width: null, height: null, durationSec: null,
          path: serverMediaDownloadPath(id), posterPath: poster.startsWith('/') ? poster : null,
          chatId: chatId || null, chats: chatId ? [{ id: chatId, title: titles.get(chatId) || null }] : null,
          by: null, how: null, at: null,
          prompt: str(row.prompt) || null, lineageId: str(row.lineageId) || null, version: count(row.version),
          versionOf: str(row.versionOf) || null, versionHow: how, versionCount: count(row.versionCount),
          provider: str(row.provider) || null, model: str(row.model) || null,
        }]
      }) as T
    }
    case 'media_versions': {
      const value = await request<{ versions?: unknown }>('GET', `/media/lineages/${encodeURIComponent(str(input.versionGroupId))}/versions`)
      const rows = (Array.isArray(value?.versions) ? value.versions : []) as Array<Record<string, unknown>>
      return rows.filter((row) => str(row.assetId)).map((row) => ({
        assetId: str(row.assetId), prompt: str(row.prompt) || null, provider: str(row.provider) || null, model: str(row.model) || null,
        createdAt: str(row.createdAt) || null, chatId: str(row.chatId) || null, version: typeof row.version === 'number' ? row.version : null,
        // 휴지통에 든 판(deleted)은 열 수 없어 path 를 비운다(화면은 그 판을 고를 수 없게 그린다).
        path: row.deleted === true ? null : serverMediaDownloadPath(str(row.assetId)),
        lineageId: str(row.lineageId) || null, versionOf: str(row.versionOf) || null,
        versionHow: row.versionHow === 'regenerated' || row.versionHow === 'edited' ? row.versionHow : null,
      })) as T
    }
    case 'chat_cancel': {
      const turnId = running.get(str(input.chatId))
      if (turnId) await request('POST', `/turns/${encodeURIComponent(turnId)}/cancel`)
      return null as T
    }
    default:
      return missing(cmd)
  }
}
