// momso server 를 흉내 내는 시험용 가짜 서버(2026-11-21, 화면의 네트워크 길 확인용).
//   API: momso-server-s17-contract.md(hq/s17-done 6ec22f472) — GET /chats, POST /chats, GET /chats/{id}(제목·때·말의 때),
//        POST /chats/{id}/turns → { turnId, userMessageId }, GET /turns/{id}, SSE /turns/{id}/events?after=N(조각마다 offset = UTF-8 바이트),
//        POST /turns/{id}/cancel, POST /turns/{id}/retry(실패한 턴의 그 말을 다시, 같은 말이 두 번 남지 않음), GET /health, 실패 { error: 까닭 값 }.
//   연결: momso-mobile-connection-s19.md S19b — 화면 파일도 같은 출처로 내주고, 브라우저는 토큰을 보내지 않는다(앞단 로그인 문과 VM 안 프록시가 맡음).
//   그래서 이 가짜는 "로그인 문 + 프록시 + server"를 한 몸으로 흉내 낸다: 토큰을 묻지 않고, 로그인이 풀린 상태면 모든 요청을 로그인 문처럼 다른 곳으로 보낸다(302).
//
// 쓰는 법(apps/alpha/desktop 에서):
//   VITE_MOMSO_NETWORK=1 npx vite build --outDir <폴더>    (화면을 네트워크 길로 빌드)
//   FAKE_WEB_DIR=<그 폴더> node tests/ui-check/fake-server.mjs [포트=8799]
//   그다음 브라우저로 http://localhost:8799/ 를 연다(주소 하나로 화면과 API).
// 시험 손잡이(화면은 모르는 길, 시험하는 쪽이 부른다):
//   POST /__fake/sign-out, POST /__fake/sign-in   로그인이 풀림/돌아옴
//   POST /__fake/fail?code=<까닭 값>               다음 턴 하나를 그 까닭으로 실패시킨다(provider_failed, cli_unavailable …). code 를 비우면 끔.
//   FAKE_DROP=1 이면 턴마다 첫 SSE 를 6조각 뒤 끊는다(화면이 after 로 다시 붙는지 본다). FAKE_HEALTH=down 이면 /health 가 503.
//   FAKE_SLOW=1 이면 답을 천천히(멈추기 시험).
// AI 연결: 줄3 S25 확정 판(S17-2026-11-21.2, hq/s17-contract a4a62e34b):
//   GET /providers → { providers: [{ provider: codex, state: connected|not_connected|cli_unavailable, accountLabel: 메일|null }, claude·gemini not_supported] }
//   POST /providers/codex/login → { sessionId }. 진행 중인 로그인이 있으면 그것을 취소하고 새로 시작한다(이전 sessionId 는 404).
//   아래는 X-Momso-Login-Session 머리가 그 값이어야 한다(없거나 다르면 404 login_not_found):
//   GET /providers/codex/login → { provider, state: starting|waiting|connected|failed|cancelled|expired, url, code, expiresInSeconds, expiresAt },
//   GET /providers/codex/login/events → SSE provider-login-state(starting) · provider-login-device { state: waiting, url, code, … }
//     · provider-login-device-closed { provider, state: connected|failed|cancelled|expired|cli_unavailable } 뒤 닫음,
//   POST /providers/codex/login/cancel. 모든 응답 Cache-Control: no-store. 코드는 시작 0.4초 뒤에 나온다. 채팅의 로그인 풀림은 auth_failed.
//   FAKE_AI=account(기본)·signedOut·nocli(CLI 없음)·none(AI 연결 길이 없는 server: /providers 가 404). FAKE_AI_EXPIRE=<초>(코드 유효 시간, 기본 900).
//   FAKE_LOGIN_SSE=drop 이면 이벤트 길을 바로 끊는다(화면이 조회로 따라잡는지 본다).
//   코덱스가 연결돼 있지 않으면 채팅은 cli_unavailable 로 실패한다. 손잡이: POST /__fake/approve(로그인 승인), POST /__fake/login-fail(진행 중인 로그인을 failed 로),
//   POST /__fake/ai?state=signedOut|account, GET /__fake/device(새 탭에 열리는 가짜 로그인 페이지).
// 승인·브라우저·로그인 저장(줄3 S23 hq/s23-done 20d07ec04 와 같은 꼴):
//   POST /chats/{id}/turns 의 permissionMode(Ask for approval 기본·Approve for me·Full access, 그 밖은 400 invalid_permission_mode). 재시도는 원래 단계.
//   턴 SSE 에 agent-approval-request { requestId, chatId, turnId, provider, kind, description, options }(다시 붙으면 대기 중인 것을 다시 보냄),
//   browser-human-required { id, chatId, turnId, reason, origin }. GET /turns/{id}/approval → { approval|null }, GET /turns/{id}/browser-events → { events },
//   POST /approvals/{id}/respond { choice } → { requestId, state: answered }(닫혔으면 409 approval_closed, 없는 답은 400 invalid_choice).
//   POST /saved-logins { siteUrl, label, username, password, askBeforeUse?, loginPath?, defaultForPath? } → { login }(비밀번호 없음), 빠진 값은 400 invalid_input.
//   손잡이: POST /__fake/ask?kind=browser_site|browser_login|command&description=… 다음 턴이 8조각 뒤 그 물음을 내고 답할 때까지 멈춘다
//     (Full access 면 브라우저 물음은 건너뜀. 거절이면 답 끝에 "(거절됨)"). POST /__fake/approval-expire 대기 중인 물음을 닫는다(5분 지남처럼).
//     POST /__fake/human?reason=verification 쓰는 중인 턴에 사람 할 일. GET /__fake/saved-logins 저장된 것(비밀번호 없이).
// 입력 물음·닫힘·다시 잇기(줄3 S32 hq/s32-done 0bcbb8c9c, human_prompt.rs 와 같은 꼴):
//   agent-input-request { requestId, chatId, turnId, kind, origin, expiresAt, fields: [{ id, secret }], choices }(다시 붙으면 대기 중인 것을 다시 보냄), 끝나면 agent-input-closed.
//   승인이 닫히면 agent-approval-closed { requestId, chatId, turnId, reason, decision }. GET /turns/{id}/requests → { approval, inputs }.
//   POST /input-requests/{id}/respond(one_time_code { code }: 영숫자·- 32자까지, saved_login { label, username, password }: 아이디·비밀번호 필수, select_saved_login { selectionId })
//     → { requestId, state: answered }, 잘못되면 400 invalid_input(물음은 남음), 닫혔으면 409 request_closed. POST /input-requests/{id}/cancel → { requestId, state: cancelled }.
//   GET /chats/{id} 에 activeTurn { turnId, state: running, offset }. GET /saved-logins { logins }, PUT /saved-logins/{id}, DELETE /saved-logins/{id} → { state: deleted }.
//   손잡이: POST /__fake/input?kind=one_time_code|saved_login|select_saved_login 쓰는 중인 턴에 물음(답할 때까지 턴이 멈춤), POST /__fake/input-expire 대기 중인 입력을 닫음.
//     /__fake/approval-expire 는 승인을 5분 지남으로 닫고(agent-approval-closed timeout) 턴은 이어진다.
// 몸소 입구(줄2 S26 momso-anywhere-pwa-entry-20261121.md "화면 계약"): FAKE_GATE=entry 이면 로그인이 풀린 상태에서 API 는 401 { error: login_required },
//   문서 탐색(Accept text/html)은 200 로그인 HTML, 로그인 전에도 매니페스트·아이콘 여섯 파일은 낸다. 손잡이 POST /__fake/forbid 다음 API 하나를 403 JSON 으로(보안 거절).
// 시작하기 페이지 시험: FAKE_WEB_DIR 에 VITE_MOMSO_START=1 빌드 + tests/ui-check/fake-start/ 의 config.json·momso_anywhere_start.mjs 를 함께 둔다.
// 문서 읽기(줄3 S34 hq/s34-done 087247e4a): GET /documents { documents: [{ id, title, folder, updatedAt }] }, GET /documents/{id} { id, title, folder, updatedAt, body }(없으면 404 document_not_found).
//   FAKE_DOCS=none 이면 문서 0개, FAKE_DOCS=fail 이면 목록 503. 수업 자료 쓰기 승인은 /__fake/ask?kind=crm_write&description=…
// 클로드·제미나이 API 키(S55, hq/merge-s57-done momso-server-s17-contract.md "S55"): GET /providers 의 claude·gemini 는 not_connected|credential_saved|storage_unavailable.
//   FAKE_KEYS=claude:credential_saved,gemini:storage_unavailable 처럼 처음 값을 정한다(기본 둘 다 not_connected, FAKE_KEYS=old 면 옛 server 의 not_supported).
//   POST /providers/{claude|gemini}/key/request → provider_api_key 입력 요청 { requestId, kind, provider, expiresAt, fields: [{ id: apiKey, secret: true }] }(5분),
//   GET 같은 주소 → { inputs }. POST /input-requests/{id}/respond { apiKey } → { requestId, state: credential_saved }, 빈 값·줄바꿈·4096자 넘음은 400 invalid_key,
//   보관소를 열 수 없으면 503 storage_unavailable. 키 값은 어디에도 두지 않고 길이만 기억한다(GET /__fake/keys).
// 첨부(S39): POST /uploads { fileName, mediaType, byteLength } → { uploadId, received, total }, PUT /uploads/{id}/chunks?offset=N(원본 바이트, 8 MiB까지, 위치가 다르면 409 offset_mismatch),
//   GET /uploads/{id} → 진행 값, POST /uploads/{id}/finish → { id, sha256, byteLength, originalFileName, mediaType, kind }(덜 받았으면 409 incomplete).
//   POST /chats/{id}/turns 의 attachments: [{ assetId }] → GET /chats/{id} 의 내 말 attachments: [{ assetId, name, mediaType, byteLength }](없으면 빈 배열).
//   손잡이 POST /__fake/upload-drop: 다음 조각 하나를 받지 않고 연결을 끊는다(이어 올리기 시험). FAKE_UPLOAD_SLOW=<ms> 조각마다 늦춤.
// 모델 목록(S62): GET /providers/models → { providers: [{ provider, state, models: [{ id, label, isDefault, defaultEffort, efforts, fast: { supported, state } }] }] }.
//   코덱스는 코덱스 연결 상태대로, 클로드·제미나이는 키가 저장(credential_saved)됐을 때 connected. FAKE_MODELS=none 이면 404(목록이 없는 옛 server).
//   POST /chats/{id}/turns 의 provider·model·effort·fast 를 받는다: 목록에 없는 모델은 400 invalid_model, 그 모델에 없는 effort 는 400 model_setting_unavailable,
//   고속을 받지 않는 모델에 fast:true 는 400 fast_unavailable. 받은 값은 GET /__fake/last-turn(글은 길이만). 턴 실패는 기존 /__fake/fail?code=fast_unavailable.
// 채팅방 이름·찾기·보관·고정(S75, momso-chat-claude main.rs rename_chat·search_chats·list_archived_chats·archive_chat·pin_chat 와 같은 꼴):
//   PATCH /chats/{id} { title } → { chatId, title }(빈 이름·200자 넘음·제어 문자는 400 invalid_title), GET /chats/search?q= → { chats: [{ id, title, updatedAt }] }(제목·말에 든 방, 100개까지,
//   빈 말은 400 invalid_query), POST /chats/{id}/archive { archived } → { chatId, archivedAt }, GET /chats/archived → { chats: [{ id, title, createdAt, updatedAt, archivedAt, pinned, pinnedOrder }] },
//   POST /chats/{id}/pin { pinned } → { chatId, pinned }. GET /chats 는 보관한 방을 빼고 고정한 방(고정한 차례) 먼저, pinned·pinnedOrder 를 준다.
//   FAKE_CHATS=seed 이면 채팅방 몇 개(말 포함, 하나는 고정, 하나는 보관)로 시작한다.
// 문서 고치기·폴더(S37·S75, main.rs read_document·update_document·list_document_folders·create_document_folder 와 같은 꼴):
//   GET /documents/{id} 에 summary·revision(내용 지문). PUT /documents/{id} { title, summary, body, folder, revision } → { id, revision }(revision 이 다르면 409 conflict,
//   빠지면 400 revision_required). GET /document-folders → { folders: [모든 경로(위 폴더 포함)] }, POST /document-folders { folder } → { folder }(잘못된 이름은 400 invalid_folder).
//   손잡이 POST /__fake/doc-touch?id=… 그 문서를 다른 기기에서 고친 것처럼 바꾼다(그사이 바뀜 시험).
// 용어 사전(S75, main.rs list_dictionary·edit_dictionary_term·delete_dictionary_term·edit_dictionary_person·delete_dictionary_person 과 같은 꼴):
//   GET /dictionary → 데스크톱 yoga_terms_list 와 같은 객체(terms·people·peopleStatus·userRevision·peopleRevision). POST /dictionary/terms(yoga_term_edit 입력) → { id },
//   POST /dictionary/people(yoga_person_edit 입력) → { id }, DELETE 같은 주소 { id, expectedRevision } → { result }. revision 이 낡았으면 409 dictionary_changed,
//   기본 용어 지우기는 409 base_term, 빈 표기·이름은 400 invalid_input. 손잡이 POST /__fake/dict-touch 다른 기기에서 고친 것처럼 revision 만 올린다.
// 스킬(S75, main.rs list_skills·read_skill·create_skill·update_skill·enable_skill·delete_skill 과 같은 꼴):
//   GET /skills → { skills: [{ name, title, description, state: enabled|disabled|unavailable, builtin }] }, GET /skills/{name} → { name, title, description, body, revision },
//   POST /skills { name, title, description, body } → { name }(이름 규칙이 틀리면 400 invalid_skill, 이미 있으면 409 skill_conflict),
//   PUT /skills/{name} { revision, changes } → { name }(revision 이 다르면 409 skill_conflict), POST /skills/{name}/enabled { enabled }, DELETE /skills/{name}(기본 스킬은 409 skill_conflict).
//   없는 스킬은 404 skill_not_found. 손잡이 POST /__fake/skill-touch?name=… 다른 기기에서 고친 것처럼 본문을 바꾼다.
// 채팅방 지우기·묶음(S75 둘째, main.rs delete_chat·chat_groups.rs 와 같은 꼴): DELETE /chats/{id} → { chatId, deletedAt }(답하는 중이면 409 turn_in_progress),
//   GET /chat-groups → { groups: [{ id, name, order, createdAt, chatCount }] }, POST /chat-groups { name } → { id, name, createdAt }(빈 이름 400 invalid_group_name),
//   PATCH /chat-groups/{id} { name }, DELETE /chat-groups/{id}(안의 방은 묶음 밖으로), POST /chat-groups/reorder { ids }, PUT /chats/{id}/group { groupId|null }.
//   채팅방 목록·보관함에 groupId. FAKE_CHATS=seed 면 "수업 준비" 묶음 하나(12월 수업 공지 문구가 그 안).
// 문서 이전 판·되돌리기·폴더 이름 바꾸기(S75 둘째, document_history.rs·rename_document_folder 와 같은 꼴):
//   GET /documents/{id}/history → { versions: [{ revision(40자), time }] }(최근 것 먼저), GET /documents/{id}/versions/{revision} → { id, title, summary, folder, body, assets, edges },
//   POST /documents/{id}/restore { revision, expectedRevision } → { id, revision }(저장하지 않은 내용이 있으면 409 unsaved_changes, revision 이 다르면 409 conflict).
//   POST /document-folders/rename { from, to } → { from, to }(아래 폴더·문서도 함께, 없으면 404 folder_not_found). 문서를 저장하면 판이 하나 생긴다.
//   손잡이 POST /__fake/doc-dirty?id=… 그 문서에 저장하지 않은 내용이 있는 것처럼(되돌리기 409).
// 원자료 찾기·내려받기(S75 둘째, attachment_bridge.rs search_assets·asset_download 와 같은 꼴): GET /assets/search?q= → { items: [{ id, sha256, byteLength, originalFileName, mediaType, kind }], offset, limit }
//   (이름에 그 글이 든 것, 빈 말은 400 invalid_arguments), GET /assets/{id}/download → 원본(application/octet-stream, Content-Disposition: attachment). 원자료 셋으로 시작한다.
// 턴의 스킬(S75 둘째): POST /chats/{id}/turns 의 skill(켜진 스킬 이름). 없거나 꺼진 스킬이면 409 skill_unavailable(말을 남기지 않음). 내 말에 skill 을 남긴다.
// 사용자 정보(S75 셋째): GET /profile → { name, about, style, updatedAt }, PUT /profile { name, about, style }(같은 꼴). FAKE_PROFILE=conflict 면 PUT 이 409 profile_conflict.
// AI 연결 해제: POST /providers/{codex|claude|gemini}/disconnect → { state: not_connected }. 답하는 중이면 409 turn_in_progress, 코덱스 로그인 중이면(코덱스만) 409 login_in_progress.
// 다른 AI 채팅 가져오기: POST /chat-imports/preview { assetId, provider } → { conversations, alreadyImported, unreadable, oldest, newest }(저장 안 함),
//   POST /chat-imports { assetId, provider, requestId? } → { imported, duplicate, updated, … }(1.5초 걸림), POST /chat-imports/{requestId}/cancel → 그 가져오기가 409 cancelled.
//   올린 파일 이름이 zip·json·html 이 아니면 400 unsupported_format. JSON 배열이면 그 항목의 title 로, 아니면 정해 둔 셋으로 채팅방을 만든다(importedFrom). 같은 것은 duplicate.
// 원자료 휴지통(S75 셋째, attachment_bridge.rs trash_list·trash·restore_from_trash 와 같은 꼴): GET /assets/trash?offset&limit → { items: [{ assetId, name, mediaType, byteLength, deletedAt }], offset, limit },
//   POST /assets/{id}/trash · /restore → { assetId, deleted, changed, recordStatus }. 휴지통의 것은 찾기·사진과 동영상에서 빠지고 내려받기는 409 asset_trashed.
//   "지난 학기 출석표.xlsx" 하나가 휴지통에 든 채로 시작한다.
// 사진과 동영상(S75 셋째, attachment_bridge.rs media_list·media_versions·media_download 와 같은 꼴): GET /media?kind&chatId&offset&limit → { items, offset, limit, pendingGenerations },
//   항목 { assetId, kind, name, mediaType, byteLength, prompt, promptSource, provider, model, createdAt, chatId, messageId, lineageId, version, versionOf, versionHow, versionCount, chatAvailable }
//   (사람이 올린 것은 생성 값이 null 이고 createdAt 이 없다). 같은 묶음은 최신 판 하나만, 휴지통의 것은 빠진다.
//   GET /media/lineages/{id}/versions → { lineageId, versions: [생성 기록 + deleted] }, GET /media/{id}/download → 원본(octet-stream). 그림은 src/dev-samples 의 jpg·mp4.
//   POST /chats/{id}/turns 의 regenerateOf: 그 채팅방의 생성물이면 같은 묶음에 새 판(regenerated)을 더한다. 아니면 400 media_generation_unavailable.
import { createHash } from 'node:crypto'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { readFileSync } from 'node:fs'
import { extname, join, normalize, resolve, sep } from 'node:path'

const port = Number(process.argv[2] ?? 8799)
const webDir = process.env.FAKE_WEB_DIR ? resolve(process.env.FAKE_WEB_DIR) : null
const chats = new Map()
if (process.env.FAKE_CHATS === 'seed') {
  const seed = [
    ['chat-s1', '수요일 저녁 빈야사 순서 정리', '2026-11-21T20:10:00Z', 0, null, ['수요일 저녁 빈야사 수업 순서를 짜 줘', '전사 자세로 시작해서 비둘기 자세로 마무리하는 흐름입니다.']],
    ['chat-s2', '어깨가 굳은 수련생 상담', '2026-11-21T09:00:00Z', null, null, ['왼쪽 어깨가 잘 안 올라간다는 수련생에게 무엇을 권할까?', '팔을 낮춘 변형부터 권해 보세요.']],
    ['chat-s3', '12월 수업 공지 문구', '2026-11-20T11:30:00Z', null, null, ['12월 휴강 공지 문구를 다듬어 줘', '12월 24일과 31일은 쉽니다.']],
    ['chat-s4', '호흡 수업 자료 찾기', '2026-11-19T15:00:00Z', null, null, ['우짜이 호흡을 처음 배우는 사람에게 설명해 줘', '목 뒤를 살짝 좁혀 바다 소리처럼 내쉽니다.']],
    ['chat-s5', '지난달 정산 메모', '2026-11-02T10:00:00Z', null, '2026-11-15T08:00:00Z', ['10월 수업료 정산을 정리해 줘', '정리했습니다.']],
  ]
  for (const [id, title, at, pinnedOrder, archivedAt, [ask, answer]] of seed) {
    chats.set(id, { title, createdAt: at, updatedAt: at, pinnedOrder, archivedAt, messages: [
      { id: `${id}-m1`, role: 'user', text: ask, createdAt: at, attachments: [] },
      { id: `${id}-m2`, role: 'assistant', text: answer, createdAt: at },
    ] })
  }
}
const chatGroups = []
let groupSeq = 0
if (process.env.FAKE_CHATS === 'seed') {
  chatGroups.push({ id: 'group-1', name: '수업 준비', order: 0, createdAt: '2026-11-20T00:00:00Z' })
  chats.get('chat-s3').groupId = 'group-1'
  groupSeq = 1
}
const chatEntry = (id, c) => ({ id, title: c.title, createdAt: c.createdAt, updatedAt: c.updatedAt, messageCount: c.messages.length, pinned: c.pinnedOrder != null, pinnedOrder: c.pinnedOrder ?? null, groupId: c.groupId ?? null })
const badText = (text) => !text || [...text].length > 200 || /[\u0000-\u001f\u007f]/.test(text)
const turns = new Map()
let seq = 0
let signedIn = true
let failNext = null
let forbidNext = false
const PUBLIC_FILES = ['/manifest.webmanifest', '/app-icons/apple-touch-icon.png', '/app-icons/favicon-32.png', '/app-icons/icon-192.png', '/app-icons/icon-512.png', '/app-icons/icon-512-maskable.png']
let askNext = null
const approvals = new Map()
const inputs = new Map()
const providerKeys = { claude: 'not_connected', gemini: 'not_connected' }
for (const part of (process.env.FAKE_KEYS ?? '').split(',')) { const [name, value] = part.split(':'); if (name in providerKeys && value) providerKeys[name] = value }
const keyLengths = {}
const uploads = new Map()
const assets = new Map()
const assetBytes = new Map()
for (const [name, mediaType, kind, text] of [
  ['수강생 명단 11월.csv', 'text/csv', 'text', '이름,반\n김하늘,수요일 저녁\n박소이,토요일 오전\n'],
  ['11월 12일 수업 녹음.m4a', 'audio/mp4', 'audio', 'fake audio'],
  ['빈야사 순서 메모.txt', 'text/plain', 'text', '다운독 5호흡, 전사 자세 II, 삼각 자세\n'],
  ['지난 학기 출석표.xlsx', 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet', 'other', 'fake xlsx'],
]) {
  const bytes = Buffer.from(text, 'utf8')
  const sha256 = createHash('sha256').update(bytes).digest('hex')
  const id = `asset-${sha256.slice(0, 12)}`
  assets.set(id, { id, sha256, byteLength: bytes.length, originalFileName: name, mediaType, kind })
  assetBytes.set(id, bytes)
}
const generations = new Map()
{
  const sample = (name) => readFileSync(new URL(`../../src/dev-samples/${name}`, import.meta.url))
  const add = (file, kind, mediaType, generation) => {
    const data = sample(file)
    const id = `asset-${createHash('sha256').update(data).update(file).digest('hex').slice(0, 12)}`
    assets.set(id, { id, sha256: createHash('sha256').update(data).digest('hex'), byteLength: data.length, originalFileName: file, mediaType, kind })
    assetBytes.set(id, data)
    if (generation) generations.set(id, { assetId: id, promptSource: 'reported', model: 'gpt-image-1', messageId: `${generation.chatId}-m2`, versionOf: null, versionHow: null, ...generation })
    return id
  }
  const sky1 = add('sky-1-poster.jpg', 'image', 'image/jpeg', { prompt: '천천히 밝아지는 아침 하늘, 수업 안내 첫 장 배경으로 쓸 가로 사진', provider: 'codex', createdAt: '2026-11-20T09:10:00Z', chatId: 'chat-s3', lineageId: 'lineage-sky', version: 1 })
  add('sky-2-poster.jpg', 'image', 'image/jpeg', { prompt: '천천히 밝아지는 아침 하늘, 수업 안내 첫 장 배경으로 쓸 가로 사진', provider: 'codex', createdAt: '2026-11-20T09:20:00Z', chatId: 'chat-s3', lineageId: 'lineage-sky', version: 2, versionOf: sky1, versionHow: 'regenerated' })
  add('waves-1-poster.jpg', 'image', 'image/jpeg', { prompt: '잔잔한 물결 무늬, 차분한 파란색, 수업 시간표 배경', provider: 'codex', createdAt: '2026-11-21T10:00:00Z', chatId: 'chat-s1', lineageId: 'lineage-waves', version: 1 })
  add('waves-1.mp4', 'video', 'video/mp4', { prompt: '물결이 잔잔히 움직이는 4초 세로 영상(릴스용)', provider: 'codex', model: 'sora-2', createdAt: '2026-11-21T11:00:00Z', chatId: 'chat-s1', lineageId: 'lineage-reel', version: 1 })
  add('sky-2.mp4', 'video', 'video/mp4', null)
}
const trashed = new Map([[[...assets.values()].find((a) => a.originalFileName === '지난 학기 출석표.xlsx').id, '2026-11-18T10:20:00Z']])
let dropNextChunk = false
const profile = { name: '김하늘', about: '서울에서 작은 요가원을 운영합니다. 수요일 저녁반과 토요일 오전반을 가르칩니다.', style: '', updatedAt: '2026-11-20T09:00:00Z' }
const importCancels = new Map()
const importedKeys = new Set()
let lastTurn = null
const FAKE_MODEL_LISTS = {
  codex: [
    { id: 'gpt-5.5-codex', label: 'GPT-5.5-Codex', isDefault: true, defaultEffort: 'medium', efforts: [{ id: 'low', label: 'low' }, { id: 'medium', label: 'medium' }, { id: 'high', label: 'high' }, { id: 'xhigh', label: 'xhigh' }], fast: { supported: true, state: 'unknown' } },
    { id: 'gpt-5.5-mini', label: 'GPT-5.5-mini', isDefault: false, defaultEffort: 'low', efforts: [{ id: 'low', label: 'low' }, { id: 'medium', label: 'medium' }], fast: { supported: false, state: 'unavailable' } },
  ],
  claude: [
    { id: 'claude-opus-5-5', label: 'Claude Opus 5.5', isDefault: null, defaultEffort: 'high', efforts: ['low', 'medium', 'high', 'max'], fast: { supported: true, state: 'unknown' } },
    { id: 'claude-sonnet-5-5', label: 'Claude Sonnet 5.5', isDefault: null, defaultEffort: 'medium', efforts: ['low', 'medium', 'high'], fast: { supported: false, state: 'unavailable' } },
    { id: 'claude-haiku-5-5', label: 'Claude Haiku 5.5', isDefault: null, defaultEffort: null, efforts: [], fast: { supported: false, state: 'unavailable' } },
  ],
  gemini: [
    { id: 'gemini-3-pro', label: 'Gemini 3 Pro', isDefault: null, defaultEffort: 'high', efforts: ['low', 'high'], fast: { supported: false, state: 'unavailable' } },
    { id: 'gemini-3-flash', label: 'Gemini 3 Flash', isDefault: null, defaultEffort: null, efforts: [], fast: { supported: false, state: 'unavailable' } },
  ],
}
function modelState(provider) {
  if (provider === 'codex') return codex.state
  return providerKeys[provider] === 'credential_saved' ? 'connected' : providerKeys[provider]
}
const documents = process.env.FAKE_DOCS === 'none' ? [] : [
  {"id": "zet_20261120_093000", "title": "11월 셋째 주 빈야사 순서", "summary": "다운독에서 시작해 삼각 자세로 마무리하는 60분 흐름", "folder": "", "updatedAt": "2026-11-20T09:30:00+09:00", "body": "## 순서\n\n1. 아도 무카 스바나사나 5호흡\n2. 전사 자세 II\n3. **삼각 자세**로 마무리\n\n[참고 영상](https://example.com/flow)"},
  {"id": "zet_20261118_200000", "title": "왼쪽 어깨가 걸린다고 한 수련생", "summary": "어깨 높이를 확인하고 팔 각도를 낮춘 변형을 권함", "folder": "수련생 기록", "updatedAt": "2026-11-18T20:00:00+09:00", "body": "어깨 높이를 먼저 확인하고 팔 각도를 낮춰 보기."},
  {"id": "zet_20261112_190000", "title": "11월 12일 저녁 하타 수업 돌아보기", "summary": "", "folder": "수업 기록/2026-11", "updatedAt": "2026-11-12T19:00:00+09:00", "body": ""},
]
const docRevision = (d) => createHash('sha256').update(JSON.stringify([d.title, d.summary, d.folder, d.body])).digest('hex').slice(0, 16)
const docVersions = new Map()
const dirtyDocs = new Set()
const snapshot = (d) => ({ title: d.title, summary: d.summary, folder: d.folder, body: d.body })
const commitDoc = (d, time = now()) => {
  const list = docVersions.get(d.id) ?? []
  list.unshift({ revision: createHash('sha1').update(`${d.id}${list.length}${time}${d.body}`).digest('hex'), time, ...snapshot(d) })
  docVersions.set(d.id, list)
}
for (const d of documents) {
  if (d.id === 'zet_20261120_093000') {
    commitDoc({ ...d, body: '## 순서\n\n1. 아도 무카 스바나사나 5호흡\n2. 전사 자세 II' }, '2026-11-19T21:10:00+09:00')
  }
  commitDoc(d, d.updatedAt)
}
const extraFolders = new Set()
const allFolders = () => {
  const out = new Set(extraFolders)
  for (const path of [...documents.map((d) => d.folder), ...extraFolders]) {
    const parts = path ? path.split('/') : []
    for (let i = 1; i <= parts.length; i += 1) out.add(parts.slice(0, i).join('/'))
  }
  return [...out].sort()
}
const dictionary = {
  terms: [
    { id: 'adho-mukha-svanasana', enabled: true, sanskrit: 'Adho Mukha Śvānāsana', roman: 'adho mukha svanasana', korean: ['아도 무카 스바나사나'], koreanNames: ['다운독', '견상 자세'], meaning: '얼굴을 아래로 한 개 자세', category: '자세', variants: [{ text: '아도무카', origin: 'default' }], searchOnly: [{ text: '아도 무카 스와나사나', origin: 'default' }], origin: 'default', userKind: null, canDelete: false },
    { id: 'virabhadrasana-2', enabled: true, sanskrit: 'Vīrabhadrāsana II', roman: 'virabhadrasana ii', korean: ['비라바드라사나 2'], koreanNames: ['전사 자세 2'], meaning: '전사 자세의 두 번째 변형', category: '자세', variants: [{ text: '워리어 2', origin: 'user' }], searchOnly: [], origin: 'default', userKind: 'modified', canDelete: false },
    { id: 'ujjayi', enabled: true, sanskrit: 'Ujjāyī', roman: 'ujjayi', korean: ['우짜이'], koreanNames: ['승리 호흡'], meaning: '목 뒤를 좁혀 소리 나게 쉬는 호흡', category: '호흡', variants: [], searchOnly: [{ text: '웃자이', origin: 'default' }], origin: 'default', userKind: null, canDelete: false },
    { id: 'savasana', enabled: false, sanskrit: 'Śavāsana', roman: 'savasana', korean: ['사바사나'], koreanNames: ['송장 자세'], meaning: '누워서 쉬는 마무리 자세', category: '자세', variants: [], searchOnly: [], origin: 'default', userKind: null, canDelete: false },
    { id: 'user-1', enabled: true, sanskrit: '', roman: '', korean: ['벽 다운독'], koreanNames: [], meaning: '벽에 손을 짚고 하는 다운독', category: '자세', variants: [], searchOnly: [], origin: 'user', userKind: 'added', canDelete: true },
  ],
  people: [
    { id: 'person-1', name: '김하늘', aliases: ['하늘 선생님'], confirmed: true, origin: 'user' },
    { id: 'person-2', name: '박소이', aliases: [], confirmed: false, origin: 'user' },
  ],
  peopleStatus: 'ok',
  userRevision: 'u1',
  peopleRevision: 'p1',
}
let dictSeq = 1
const bumpTerms = () => { dictionary.userRevision = `u${Number(dictionary.userRevision.slice(1)) + 1}` }
const bumpPeople = () => { dictionary.peopleRevision = `p${Number(dictionary.peopleRevision.slice(1)) + 1}` }
const skills = [
  { name: 'class-notice', title: '수업 공지 쓰기', description: '휴강·보강 공지를 수련생에게 보낼 글로 다듬습니다', body: '공지는 날짜, 시간, 바뀐 점 순서로 짧게 씁니다.', state: 'enabled', builtin: true },
  { name: 'sequence-review', title: '수업 순서 점검', description: '자세 순서가 몸에 무리가 없는지 살핍니다', body: '앞으로 굽히기 뒤에는 반대 동작을 넣습니다.', state: 'disabled', builtin: true },
  { name: 'student-followup', title: '수련생 후속 연락', description: '상담한 수련생에게 보낼 안부 글을 씁니다', body: '지난 상담에서 들은 몸 상태를 먼저 묻습니다.', state: 'enabled', builtin: false },
  { name: 'crm-sync', title: '예약 사이트 정리', description: '예약 사이트의 수업 기록을 정리합니다', body: '', state: 'unavailable', builtin: false },
]
const skillRevision = (s) => createHash('sha256').update(JSON.stringify([s.title, s.description, s.body])).digest('hex').slice(0, 16)
const SKILL_NAME = /^(?=.{1,64}$)[a-z0-9]+(?:-[a-z0-9]+)*$/
const savedLogins = []
const aiMode = process.env.FAKE_AI || 'account'
const codex = { state: aiMode === 'signedOut' ? 'not_connected' : aiMode === 'nocli' ? 'cli_unavailable' : 'connected' }
let deviceLogin = null
const active = () => deviceLogin && (deviceLogin.state === 'starting' || deviceLogin.state === 'waiting')
const settle = () => { if (deviceLogin?.state === 'waiting' && Date.now() > deviceLogin.expiresAt) { deviceLogin.state = 'expired'; deviceLogin.code = null } }
const loginView = () => {
  settle()
  const l = deviceLogin
  return { provider: 'codex', state: l.state, url: l.code ? `http://localhost:${port}/__fake/device` : null, code: l.code, expiresInSeconds: l.code ? Math.max(0, Math.round((l.expiresAt - Date.now()) / 1000)) : null, expiresAt: l.code ? new Date(l.expiresAt).toISOString() : null }
}
const ownLogin = (req) => { const s = req.headers['x-momso-login-session']; return deviceLogin && s && s === deviceLogin.session ? deviceLogin : null }

const TYPES = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.png': 'image/png', '.svg': 'image/svg+xml', '.jpg': 'image/jpeg', '.mp4': 'video/mp4', '.woff2': 'font/woff2', '.json': 'application/json', '.webmanifest': 'application/manifest+json', '.mjs': 'text/javascript; charset=utf-8' }
const now = () => new Date().toISOString()
const bytes = (text) => Buffer.byteLength(text, 'utf8')

function send(res, status, value) {
  res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store' })
  res.end(JSON.stringify(value))
}

function rawBody(req) {
  return new Promise((done) => {
    const parts = []
    req.on('data', (chunk) => { parts.push(chunk) })
    req.on('end', () => done(Buffer.concat(parts)))
  })
}

function body(req) {
  return new Promise((done) => {
    let text = ''
    req.on('data', (chunk) => { text += chunk })
    req.on('end', () => { try { done(text ? JSON.parse(text) : {}) } catch { done(null) } })
  })
}

/** 화면 파일(같은 출처). 없는 경로는 index.html(한 쪽짜리 화면). */
async function serveWeb(res, pathname) {
  if (!webDir) { send(res, 404, { error: 'not_found' }); return }
  const wanted = normalize(join(webDir, decodeURIComponent(pathname)))
  const inside = wanted === webDir || wanted.startsWith(webDir + sep)
  for (const file of inside && pathname !== '/' ? [wanted, join(webDir, 'index.html')] : [join(webDir, 'index.html')]) {
    try {
      const data = await readFile(file)
      res.writeHead(200, { 'Content-Type': TYPES[extname(file)] ?? 'application/octet-stream' })
      res.end(data)
      return
    } catch { /* 다음 */ }
  }
  send(res, 404, { error: 'not_found' })
}

/** 답을 조금씩 만든다(글자 네 개씩 60ms 마다, FAKE_SLOW 면 400ms). 실패 까닭이 정해져 있으면 몇 조각 뒤 그 까닭으로 끝낸다. */
function runTurn(turn, prompt) {
  const chat = chats.get(turn.chatId)
  // 연결돼 있지 않으면 코덱스가 unauthorized 를 주므로 auth_failed, CLI 를 못 쓰면 cli_unavailable.
  const fail = failNext ?? (codex.state === 'not_connected' ? 'auth_failed' : codex.state === 'cli_unavailable' ? 'cli_unavailable' : null)
  failNext = null
  const ask = askNext && !(turn.permissionMode === 'Full access' && askNext.kind.startsWith('browser_')) ? askNext : null
  askNext = null
  const reply = `「${prompt}」 잘 받았습니다. 가짜 서버가 코덱스 대신 조금씩 답하고 있습니다. 수업 등록 여부 같은 짧은 확인을 휴대폰에서 보게 될 화면입니다.`
  const parts = Array.from(reply)
  let at = 0
  const end = (state, error) => {
    clearInterval(turn.timer)
    turn.state = state
    turn.error = error
    if (state === 'completed') { chat.messages.push({ id: `m${++seq}`, role: 'assistant', text: turn.text, createdAt: now() }); chat.updatedAt = now() }
    for (const listener of turn.listeners) listener({ type: 'done' })
  }
  turn.end = end
  turn.timer = setInterval(() => {
    if (fail && at >= 8) { end('failed', fail); return }
    if (ask && at >= 8 && !turn.approval) {
      const options = ask.kind.startsWith('browser_') || ask.kind === 'crm_write' ? ['allow_once', 'deny'] : ['allow_once', 'allow_session', 'deny', 'cancel']
      turn.approval = { requestId: `ap-${++seq}`, chatId: turn.chatId, turnId: turn.turnId, provider: 'codex', kind: ask.kind, description: ask.description, options, state: 'pending' }
      approvals.set(turn.approval.requestId, turn.approval)
      for (const listener of turn.listeners) listener({ type: 'approval' })
    }
    if (turn.approval?.state === 'pending') return
    if (turn.inputs.some((item) => item.state === 'pending')) return
    if (turn.approval && !turn.approvalNoted) {
      turn.approvalNoted = true
      if (turn.approval.state !== 'allow_once' && turn.approval.state !== 'allow_session') parts.push(...Array.from(' (거절됨)'))
    }
    const piece = parts.slice(at, at + 4).join('')
    at += 4
    turn.text += piece
    for (const listener of turn.listeners) listener({ type: 'delta', delta: piece })
    if (at >= parts.length) end('completed', null)
  }, process.env.FAKE_SLOW === '1' ? 400 : 60)
}

function inputView(item) {
  const fields = item.kind === 'one_time_code' ? [{ id: 'code', secret: true }] : item.kind === 'saved_login' ? [{ id: 'label', secret: false }, { id: 'username', secret: false }, { id: 'password', secret: true }] : [{ id: 'selectionId', secret: false }]
  return { requestId: item.requestId, chatId: item.chatId, turnId: item.turnId, kind: item.kind, origin: item.origin, expiresAt: item.expiresAt, fields, choices: item.choices }
}

function startTurn(chatId, userMessageId, prompt, retryOf = null, permissionMode = 'Ask for approval') {
  const turnId = `turn-${++seq}`
  const turn = { turnId, chatId, userMessageId, prompt, state: 'running', text: '', error: null, listeners: new Set(), drops: 0, retryOf, retried: null, permissionMode, approval: null, approvalNoted: false, browserEvents: [], inputs: [] }
  turns.set(turnId, turn)
  runTurn(turn, prompt)
  return turn
}

createServer(async (req, res) => {
  const url = new URL(req.url, `http://localhost:${port}`)
  // 시험 손잡이
  if (req.method === 'POST' && url.pathname === '/__fake/sign-out') { signedIn = false; send(res, 200, { signedIn }); return }
  if (req.method === 'POST' && url.pathname === '/__fake/sign-in') { signedIn = true; send(res, 200, { signedIn }); return }
  if (req.method === 'POST' && url.pathname === '/__fake/ask') { askNext = { kind: url.searchParams.get('kind') || 'browser_site', description: url.searchParams.get('description') || 'crm.example.com' }; send(res, 200, askNext); return }
  if (req.method === 'POST' && url.pathname === '/__fake/approval-expire') {
    for (const a of approvals.values()) if (a.state === 'pending') { a.state = 'cancel'; const t = turns.get(a.turnId); for (const listener of t?.listeners ?? []) listener({ type: 'approval-closed', reason: 'timeout', decision: 'cancel', approval: a }) }
    send(res, 200, {})
    return
  }
  if (req.method === 'POST' && url.pathname === '/__fake/input') {
    const turn = [...turns.values()].find((t) => t.state === 'running')
    const kind = url.searchParams.get('kind') || 'one_time_code'
    if (turn) {
      const choices = kind === 'select_saved_login' ? [{ id: 'login-a', label: '관리자', username: 'admin@example.com' }, { id: 'login-b', label: '강사', username: 'teacher@example.com' }] : []
      const item = { requestId: `in-${++seq}`, chatId: turn.chatId, turnId: turn.turnId, kind, origin: 'https://crm.example.com', expiresAt: new Date(Date.now() + 300000).toISOString(), choices, state: 'pending' }
      inputs.set(item.requestId, item)
      turn.inputs.push(item)
      for (const listener of turn.listeners) listener({ type: 'input', item })
    }
    send(res, 200, { turn: turn?.turnId ?? null })
    return
  }
  if (req.method === 'POST' && url.pathname === '/__fake/input-expire') {
    for (const item of inputs.values()) if (item.state === 'pending') { item.state = 'expired'; for (const listener of turns.get(item.turnId)?.listeners ?? []) listener({ type: 'input-closed', item }) }
    send(res, 200, {})
    return
  }
  if (req.method === 'POST' && url.pathname === '/__fake/human') {
    const turn = [...turns.values()].find((t) => t.state === 'running')
    if (turn) { const event = { id: ++seq, chatId: turn.chatId, turnId: turn.turnId, reason: url.searchParams.get('reason') || 'verification', origin: 'https://crm.example.com' }; turn.browserEvents.push(event); for (const listener of turn.listeners) listener({ type: 'human', event }) }
    send(res, 200, { turn: turn?.turnId ?? null })
    return
  }
  if (req.method === 'GET' && url.pathname === '/__fake/saved-logins') { send(res, 200, { logins: savedLogins }); return }
  if (req.method === 'POST' && url.pathname === '/__fake/fail') { failNext = url.searchParams.get('code') || null; send(res, 200, { failNext }); return }
  if (req.method === 'POST' && url.pathname === '/__fake/approve') {
    settle()
    if (deviceLogin?.state === 'waiting') { deviceLogin.state = 'connected'; deviceLogin.code = null; codex.state = 'connected' }
    send(res, 200, { state: deviceLogin?.state ?? null })
    return
  }
  if (req.method === 'POST' && url.pathname === '/__fake/login-fail') { if (active()) { deviceLogin.state = 'failed'; deviceLogin.code = null } send(res, 200, { state: deviceLogin?.state ?? null }); return }
  if (req.method === 'POST' && url.pathname === '/__fake/ai') { codex.state = url.searchParams.get('state') === 'signedOut' ? 'not_connected' : 'connected'; send(res, 200, codex); return }
  if (req.method === 'GET' && url.pathname === '/__fake/device') {
    res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' })
    res.end('<!doctype html><meta name="viewport" content="width=device-width"><title>가짜 코덱스 로그인</title><body style="font:16px sans-serif;padding:24px"><h1>가짜 코덱스 로그인</h1><p>코드를 붙여 넣고 승인을 누르세요.</p><input placeholder="코드"><button onclick="fetch(\'/__fake/approve\',{method:\'POST\'}).then(()=>document.body.append(\' 승인했습니다\'))">승인</button></body>')
    return
  }
  // 로그인이 풀렸으면 로그인 문처럼 다른 출처의 로그인 화면으로 보낸다(화면 파일·API 모두).
  if (req.method === 'POST' && url.pathname === '/__fake/upload-drop') { dropNextChunk = true; send(res, 200, { dropNextChunk }); return }
  if (req.method === 'GET' && url.pathname === '/__fake/last-turn') { send(res, 200, { lastTurn }); return }
  if (req.method === 'GET' && url.pathname === '/__fake/keys') { send(res, 200, { providerKeys, keyLengths }); return }
  if (req.method === 'POST' && url.pathname === '/__fake/doc-dirty') { dirtyDocs.add(url.searchParams.get('id')); send(res, 200, { dirty: [...dirtyDocs] }); return }
  if (req.method === 'POST' && url.pathname === '/__fake/doc-touch') {
    const doc = documents.find((d) => d.id === url.searchParams.get('id'))
    if (doc) { doc.body += '\n\n(다른 기기에서 더한 줄)'; doc.updatedAt = now() }
    send(res, 200, { touched: Boolean(doc) })
    return
  }
  if (req.method === 'POST' && url.pathname === '/__fake/dict-touch') { bumpTerms(); bumpPeople(); send(res, 200, { userRevision: dictionary.userRevision }); return }
  if (req.method === 'POST' && url.pathname === '/__fake/skill-touch') {
    const skill = skills.find((s) => s.name === url.searchParams.get('name'))
    if (skill) skill.body += ' (다른 기기에서 고침)'
    send(res, 200, { touched: Boolean(skill) })
    return
  }
  if (req.method === 'POST' && url.pathname === '/__fake/forbid') { forbidNext = true; send(res, 200, { forbidNext }); return }
  if (!signedIn) {
    if (process.env.FAKE_GATE === 'entry') {
      if (req.method === 'GET' && PUBLIC_FILES.includes(url.pathname)) { await serveWeb(res, url.pathname); return }
      if ((req.headers.accept ?? '').includes('text/html')) {
        res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' })
        res.end('<!doctype html><meta name="viewport" content="width=device-width"><link rel="manifest" href="/manifest.webmanifest" crossorigin="use-credentials"><title>momso</title><h1>momso</h1><p>가짜 입구 로그인</p>')
        return
      }
      send(res, 401, { error: 'login_required' })
      return
    }
    res.writeHead(302, { Location: 'https://momso-test.cloudflareaccess.com/cdn-cgi/access/login/fake', 'Cache-Control': 'no-store' })
    res.end()
    return
  }
  if (forbidNext && !url.pathname.startsWith('/__fake') && !(req.headers.accept ?? '').includes('text/html')) { forbidNext = false; send(res, 403, { error: 'forbidden' }); return }
  // 브라우저는 토큰을 보내지 않는다(S19b). 보내면 계약 위반이라 거절한다.
  if (req.headers.authorization) { send(res, 400, { error: 'browser_must_not_send_token' }); return }
  const path = url.pathname.split('/').filter(Boolean)

  if (req.method === 'GET' && url.pathname === '/health') {
    if (process.env.FAKE_HEALTH === 'down') { send(res, 503, { error: 'unavailable' }); return }
    send(res, 200, { state: 'ready' })
    return
  }

  if (url.pathname === '/providers' || url.pathname.startsWith('/providers/')) {
    if (aiMode === 'none') { send(res, 404, { error: 'not_found' }); return }
    if (req.method === 'GET' && url.pathname === '/providers') {
      const old = process.env.FAKE_KEYS === 'old'
      const providers = [{ provider: 'codex', state: codex.state, accountLabel: codex.state === 'connected' ? 'fake@momso.test' : null }, { provider: 'claude', state: old ? 'not_supported' : providerKeys.claude, accountLabel: null }, { provider: 'gemini', state: old ? 'not_supported' : providerKeys.gemini, accountLabel: null }]
      send(res, 200, { providers })
      return
    }
    if (req.method === 'GET' && url.pathname === '/providers/models') {
      if (process.env.FAKE_MODELS === 'none') { send(res, 404, { error: 'not_found' }); return }
      const providers = ['codex', 'claude', 'gemini'].map((provider) => { const state = modelState(provider); return { provider, state, models: state === 'connected' ? FAKE_MODEL_LISTS[provider] : [] } })
      send(res, 200, { providers })
      return
    }
    const offMatch = url.pathname.match(/^\/providers\/(codex|claude|gemini)\/disconnect$/)
    if (offMatch && req.method === 'POST') {
      const provider = offMatch[1]
      if ([...turns.values()].some((t) => t.state === 'running')) { send(res, 409, { error: 'turn_in_progress' }); return }
      if (provider === 'codex' && active()) { send(res, 409, { error: 'login_in_progress' }); return }
      if (provider === 'codex') codex.state = 'not_connected'
      else providerKeys[provider] = 'not_connected'
      send(res, 200, { state: 'not_connected' })
      return
    }
    const keyMatch = url.pathname.match(/^\/providers\/(claude|gemini)\/key\/request$/)
    if (keyMatch) {
      const provider = keyMatch[1]
      const view = (item) => ({ requestId: item.requestId, chatId: '', turnId: '', kind: 'provider_api_key', origin: null, provider, expiresAt: item.expiresAt, fields: [{ id: 'apiKey', secret: true }], choices: [] })
      const live = (item) => item.kind === 'provider_api_key' && item.provider === provider && item.state === 'pending' && Date.parse(item.expiresAt) > Date.now()
      if (req.method === 'GET') { send(res, 200, { inputs: [...inputs.values()].filter(live).map(view) }); return }
      if (req.method === 'POST') {
        const item = { requestId: `key-${++seq}`, kind: 'provider_api_key', provider, state: 'pending', turnId: '', expiresAt: new Date(Date.now() + 300_000).toISOString() }
        inputs.set(item.requestId, item)
        send(res, 200, view(item))
        return
      }
    }
    if (req.method === 'POST' && url.pathname === '/providers/codex/login') {
      settle()
      // 진행 중인 이전 로그인은 취소하고 새로 시작한다(이전 sessionId 는 더 쓸 수 없다).
      if (active()) { deviceLogin.state = 'cancelled'; deviceLogin.code = null }
      const seconds = Number(process.env.FAKE_AI_EXPIRE || 900)
      const login = { session: `s${Date.now()}${Math.random().toString(16).slice(2)}`, state: 'starting', code: null, expiresAt: 0 }
      deviceLogin = login
      // 코덱스 CLI 가 코드를 내놓기까지 조금 걸린다.
      setTimeout(() => { if (deviceLogin === login && login.state === 'starting') { login.state = 'waiting'; login.code = `KQ7M-${String(++seq).padStart(4, '0')}`; login.expiresAt = Date.now() + seconds * 1000 } }, 400)
      send(res, 200, { sessionId: login.session })
      return
    }
    if (url.pathname.startsWith('/providers/codex/login')) {
      const login = ownLogin(req)
      if (!login) { send(res, 404, { error: 'login_not_found' }); return }
      if (req.method === 'GET' && url.pathname === '/providers/codex/login') { send(res, 200, loginView()); return }
      if (req.method === 'POST' && url.pathname === '/providers/codex/login/cancel') { if (active()) { login.state = 'cancelled'; login.code = null } send(res, 200, { provider: 'codex', state: 'cancelling' }); return }
      if (req.method === 'GET' && url.pathname === '/providers/codex/login/events') {
        res.writeHead(200, { 'Content-Type': 'text/event-stream; charset=utf-8', 'Cache-Control': 'no-store', Connection: 'keep-alive' })
        if (process.env.FAKE_LOGIN_SSE === 'drop') { res.destroy(); return }
        let last = null
        const tick = () => {
          if (deviceLogin !== login) { clearInterval(timer); res.end(); return }
          const view = loginView()
          const key = `${view.state}:${view.code ?? ''}`
          if (key === last) return
          last = key
          const terminal = view.state !== 'starting' && view.state !== 'waiting'
          const kind = terminal ? 'provider-login-device-closed' : view.code ? 'provider-login-device' : 'provider-login-state'
          res.write(`event: ${kind}\ndata: ${JSON.stringify(terminal ? { provider: 'codex', state: view.state } : view)}\n\n`)
          if (terminal) { clearInterval(timer); res.end() }
        }
        const timer = setInterval(tick, 200)
        tick()
        req.on('close', () => clearInterval(timer))
        return
      }
    }
    send(res, 404, { error: 'not_found' })
    return
  }

  if (req.method === 'POST' && url.pathname === '/saved-logins') {
    const input = await body(req)
    const text = (v, max) => typeof v === 'string' && v.trim() && v.length <= max
    if (!input || !text(input.siteUrl, 2048) || !text(input.label, 100) || !text(input.username, 256) || !text(input.password, 1024) || (input.loginPath !== undefined && !(typeof input.loginPath === 'string' && input.loginPath.startsWith('/')))) { send(res, 400, { error: 'invalid_input' }); return }
    let origin
    try { const u = new URL(input.siteUrl); if (u.protocol !== 'https:') throw new Error(); origin = u.origin } catch { send(res, 400, { error: 'invalid_input' }); return }
    const login = { id: `login-${++seq}`, origin, label: input.label, username: input.username, askBeforeUse: input.askBeforeUse === true, loginPath: input.loginPath ?? null, defaultForPath: input.defaultForPath === true, siteUrl: input.siteUrl }
    savedLogins.push(login)
    send(res, 200, { login })
    return
  }
  if (path[0] === 'approvals' && path[1] && path[2] === 'respond' && req.method === 'POST') {
    const input = await body(req)
    const approval = approvals.get(path[1])
    if (!input || !['allow_once', 'allow_session', 'deny', 'cancel'].includes(input.choice)) { send(res, 400, { error: 'invalid_choice' }); return }
    if (!approval || approval.state !== 'pending') { send(res, 409, { error: 'approval_closed' }); return }
    if (!approval.options.includes(input.choice)) { send(res, 400, { error: 'invalid_choice' }); return }
    approval.state = input.choice
    for (const listener of turns.get(approval.turnId)?.listeners ?? []) listener({ type: 'approval-closed', reason: 'responded', decision: input.choice, approval })
    send(res, 200, { requestId: approval.requestId, state: 'answered' })
    return
  }
  if (path[0] === 'input-requests' && path[1] && req.method === 'POST' && (path[2] === 'respond' || path[2] === 'cancel')) {
    const item = inputs.get(path[1])
    if (path[2] === 'cancel') {
      if (item?.state === 'pending') { item.state = 'cancelled'; for (const listener of turns.get(item.turnId)?.listeners ?? []) listener({ type: 'input-closed', item }) }
      send(res, 200, { requestId: path[1], state: 'cancelled' })
      return
    }
    const input = await body(req)
    if (!item || item.state !== 'pending') { send(res, 409, { error: 'request_closed' }); return }
    if (!input) { send(res, 400, { error: 'invalid_input' }); return }
    if (item.kind === 'provider_api_key') {
      if (Date.parse(item.expiresAt) <= Date.now()) { item.state = 'expired'; send(res, 409, { error: 'request_closed' }); return }
      const key = typeof input.apiKey === 'string' ? input.apiKey : null
      if (key === null) { send(res, 400, { error: 'invalid_input' }); return }
      if (!key || key.length > 4096 || /[\r\n\0]/.test(key)) { send(res, 400, { error: 'invalid_key' }); return }
      if (providerKeys[item.provider] === 'storage_unavailable') { send(res, 503, { error: 'storage_unavailable' }); return }
      providerKeys[item.provider] = 'credential_saved'
      keyLengths[item.provider] = key.length
      item.state = 'answered'
      send(res, 200, { requestId: item.requestId, state: 'credential_saved' })
      return
    }
    if (item.kind === 'one_time_code' && !(typeof input.code === 'string' && /^[A-Za-z0-9-]{1,32}$/.test(input.code))) { send(res, 400, { error: 'invalid_input' }); return }
    if (item.kind === 'saved_login' && !(typeof input.username === 'string' && input.username && typeof input.password === 'string' && input.password)) { send(res, 400, { error: 'invalid_input' }); return }
    if (item.kind === 'select_saved_login' && !item.choices.some((c) => c.id === input.selectionId)) { send(res, 400, { error: 'invalid_input' }); return }
    if (item.kind === 'saved_login') savedLogins.push({ id: `login-${++seq}`, origin: item.origin, label: input.label || '', username: input.username, askBeforeUse: false, loginPath: null, defaultForPath: false, siteUrl: item.origin })
    item.state = 'answered'
    item.answer = item.kind === 'select_saved_login' ? input.selectionId : item.kind === 'one_time_code' ? `code(${input.code.length})` : `login(${input.username})`
    for (const listener of turns.get(item.turnId)?.listeners ?? []) listener({ type: 'input-closed', item })
    send(res, 200, { requestId: item.requestId, state: 'answered' })
    return
  }
  if (path[0] === 'uploads') {
    const progress = (u) => ({ uploadId: u.uploadId, received: u.received, total: u.total })
    if (req.method === 'POST' && path.length === 1) {
      const input = await body(req)
      const ok = input && typeof input.fileName === 'string' && input.fileName && input.fileName.length <= 255 && typeof input.mediaType === 'string' && Number.isInteger(input.byteLength) && input.byteLength >= 0 && input.byteLength <= 2 * 1024 ** 3
      if (!ok) { send(res, 400, { error: 'invalid_arguments' }); return }
      const u = { uploadId: `up-${++seq}`, fileName: input.fileName, mediaType: input.mediaType, total: input.byteLength, received: 0, parts: [], done: null }
      uploads.set(u.uploadId, u)
      send(res, 200, progress(u))
      return
    }
    const u = uploads.get(path[1])
    if (!u) { send(res, 404, { error: 'not_found' }); return }
    if (req.method === 'GET' && path.length === 2) { send(res, 200, progress(u)); return }
    if (req.method === 'PUT' && path[2] === 'chunks') {
      const chunk = await rawBody(req)
      if (dropNextChunk) { dropNextChunk = false; req.socket.destroy(); return }
      if (u.done) { send(res, 409, { error: 'already_complete' }); return }
      if (Number(url.searchParams.get('offset')) !== u.received) { send(res, 409, { error: 'offset_mismatch' }); return }
      if (chunk.length === 0 || chunk.length > 8 * 1024 * 1024 || u.received + chunk.length > u.total) { send(res, 400, { error: 'invalid_chunk' }); return }
      const slow = Number(process.env.FAKE_UPLOAD_SLOW || 0)
      if (slow) await new Promise((r) => setTimeout(r, slow))
      u.parts.push(chunk)
      u.received += chunk.length
      send(res, 200, progress(u))
      return
    }
    if (req.method === 'POST' && path[2] === 'finish') {
      if (u.done) { send(res, 200, u.done); return }
      if (u.received < u.total) { send(res, 409, { error: 'incomplete' }); return }
      const all = Buffer.concat(u.parts)
      const sha256 = createHash('sha256').update(all).digest('hex')
      const kind = ['image', 'audio', 'video', 'text'].find((k) => u.mediaType.startsWith(`${k}/`)) ?? 'other'
      u.done = { id: `asset-${sha256.slice(0, 12)}`, sha256, byteLength: all.length, originalFileName: u.fileName, mediaType: u.mediaType, kind }
      if (!assets.has(u.done.id)) { assets.set(u.done.id, u.done); assetBytes.set(u.done.id, all) }
      u.parts = []
      send(res, 200, u.done)
      return
    }
  }
  if (url.pathname === '/documents' && req.method === 'GET') {
    if (process.env.FAKE_DOCS === 'fail') { send(res, 503, { error: 'unavailable' }); return }
    send(res, 200, { documents: documents.map(({ body, ...rest }) => rest) })
    return
  }
  if (url.pathname === '/skills') {
    if (req.method === 'GET') { send(res, 200, { skills: [...skills].sort((a, b) => a.name.localeCompare(b.name)).map(({ body, ...rest }) => rest) }); return }
    if (req.method === 'POST') {
      const input = await body(req)
      if (!SKILL_NAME.test(input?.name ?? '') || typeof input.title !== 'string' || !input.title.trim()) { send(res, 400, { error: 'invalid_skill' }); return }
      if (skills.some((s) => s.name === input.name)) { send(res, 409, { error: 'skill_conflict' }); return }
      skills.push({ name: input.name, title: input.title, description: String(input.description ?? ''), body: String(input.body ?? ''), state: 'enabled', builtin: false })
      send(res, 200, { name: input.name })
      return
    }
  }
  if (path[0] === 'skills' && path[1]) {
    const name = decodeURIComponent(path[1])
    const skill = skills.find((s) => s.name === name)
    if (!skill) { send(res, 404, { error: 'skill_not_found' }); return }
    if (req.method === 'GET' && path.length === 2) { send(res, 200, { name, title: skill.title, description: skill.description, body: skill.body, revision: skillRevision(skill) }); return }
    if (req.method === 'PUT' && path.length === 2) {
      const input = await body(req)
      if (input?.revision !== skillRevision(skill)) { send(res, 409, { error: 'skill_conflict' }); return }
      for (const key of ['title', 'description', 'body']) if (typeof input.changes?.[key] === 'string') skill[key] = input.changes[key]
      send(res, 200, { name })
      return
    }
    if (req.method === 'POST' && path[2] === 'enabled') {
      const input = await body(req)
      if (skill.state !== 'unavailable') skill.state = input?.enabled === true ? 'enabled' : 'disabled'
      send(res, 200, { name, enabled: input?.enabled === true })
      return
    }
    if (req.method === 'DELETE' && path.length === 2) {
      if (skill.builtin) { send(res, 409, { error: 'skill_conflict' }); return }
      skills.splice(skills.indexOf(skill), 1)
      send(res, 200, { name, result: 'deleted' })
      return
    }
  }
  if (url.pathname === '/dictionary' && req.method === 'GET') { send(res, 200, dictionary); return }
  if (url.pathname === '/dictionary/terms' && (req.method === 'POST' || req.method === 'DELETE')) {
    const input = await body(req)
    if (input?.expectedRevision !== dictionary.userRevision) { send(res, 409, { error: 'dictionary_changed' }); return }
    if (req.method === 'DELETE') {
      const at = dictionary.terms.findIndex((t) => t.id === input.id)
      if (at < 0) { send(res, 404, { error: 'not_found' }); return }
      if (dictionary.terms[at].origin === 'default') { send(res, 409, { error: 'base_term' }); return }
      dictionary.terms.splice(at, 1)
      bumpTerms()
      send(res, 200, { result: 'deleted' })
      return
    }
    const add = Array.isArray(input.addVariants) ? input.addVariants.map((v) => String(v).trim()).filter(Boolean) : []
    let term = input.id ? dictionary.terms.find((t) => t.id === input.id) : null
    if (input.id && !term) { send(res, 404, { error: 'not_found' }); return }
    if (!term) {
      if (add.length === 0) { send(res, 400, { error: 'invalid_input' }); return }
      term = { id: `user-${++dictSeq}`, enabled: true, sanskrit: '', roman: '', korean: [add.shift()], koreanNames: [], meaning: '', category: input.category || '사용자', variants: [], searchOnly: [], origin: 'user', userKind: 'added', canDelete: true }
      dictionary.terms.push(term)
    } else if (term.origin === 'default') term.userKind = 'modified'
    for (const text of add) term.variants.push({ text, origin: 'user' })
    if (Array.isArray(input.removeVariants)) term.variants = term.variants.filter((v) => !(v.origin === 'user' && input.removeVariants.includes(v.text)))
    if (typeof input.enabled === 'boolean') term.enabled = input.enabled
    if (typeof input.meaning === 'string') term.meaning = input.meaning
    bumpTerms()
    send(res, 200, { id: term.id })
    return
  }
  if (url.pathname === '/dictionary/people' && (req.method === 'POST' || req.method === 'DELETE')) {
    const input = await body(req)
    if (input?.expectedRevision !== dictionary.peopleRevision) { send(res, 409, { error: 'dictionary_changed' }); return }
    if (req.method === 'DELETE') {
      const at = dictionary.people.findIndex((p) => p.id === input.id)
      if (at < 0) { send(res, 404, { error: 'not_found' }); return }
      dictionary.people.splice(at, 1)
      bumpPeople()
      send(res, 200, { result: 'deleted' })
      return
    }
    const name = typeof input.name === 'string' ? input.name.trim() : ''
    if (!name) { send(res, 400, { error: 'invalid_input' }); return }
    let person = input.id ? dictionary.people.find((p) => p.id === input.id) : null
    if (input.id && !person) { send(res, 404, { error: 'not_found' }); return }
    if (!person) { person = { id: `person-${++dictSeq}`, name, aliases: [], confirmed: false, origin: 'user' }; dictionary.people.push(person) }
    person.name = name
    person.aliases = Array.isArray(input.aliases) ? input.aliases.map((a) => String(a).trim()).filter(Boolean) : person.aliases
    if (input.confirmedByUser === true) person.confirmed = true
    bumpPeople()
    send(res, 200, { id: person.id })
    return
  }
  if (url.pathname === '/profile') {
    if (req.method === 'GET') { send(res, 200, profile); return }
    if (req.method === 'PUT') {
      const input = (await body(req)) ?? {}
      if (process.env.FAKE_PROFILE === 'conflict') { send(res, 409, { error: 'profile_conflict' }); return }
      for (const key of ['name', 'about', 'style']) profile[key] = typeof input[key] === 'string' ? input[key] : ''
      profile.updatedAt = now()
      send(res, 200, profile)
      return
    }
  }
  if ((url.pathname === '/chat-imports/preview' || url.pathname === '/chat-imports') && req.method === 'POST') {
    const input = (await body(req)) ?? {}
    const asset = assets.get(input.assetId)
    if (!asset) { send(res, 404, { error: 'not_found' }); return }
    if (!['chatgpt', 'claude', 'gemini'].includes(input.provider)) { send(res, 400, { error: 'unsupported_provider' }); return }
    if (!/\.(zip|json|html)$/i.test(asset.originalFileName)) { send(res, 400, { error: 'unsupported_format' }); return }
    let titles = ['수업 홍보 문구 다듬기', '12월 시간표 정리', '어깨 통증 회원 상담 메모']
    try { const list = JSON.parse((assetBytes.get(asset.id) ?? Buffer.alloc(0)).toString('utf8')); if (Array.isArray(list)) titles = list.map((item, i) => (typeof item?.title === 'string' && item.title) || `가져온 대화 ${i + 1}`) } catch { /* zip 등 */ }
    const keys = titles.map((title) => `${input.provider}:${title}`)
    const already = keys.filter((key) => importedKeys.has(key)).length
    if (url.pathname === '/chat-imports/preview') {
      send(res, 200, { conversations: titles.length, alreadyImported: already, unreadable: 0, oldest: '2025-03-02T10:00:00Z', newest: '2026-10-28T21:30:00Z' })
      return
    }
    const requestId = typeof input.requestId === 'string' ? input.requestId : null
    if (requestId) importCancels.set(requestId, false)
    await new Promise((done) => setTimeout(done, 1500))
    if (requestId && importCancels.get(requestId)) { importCancels.delete(requestId); send(res, 409, { error: 'cancelled' }); return }
    if (requestId) importCancels.delete(requestId)
    let imported = 0
    titles.forEach((title, i) => {
      if (importedKeys.has(keys[i])) return
      importedKeys.add(keys[i])
      imported += 1
      const id = `chat-import-${++seq}`
      const at = new Date(Date.now() - (titles.length - i) * 86_400_000).toISOString()
      chats.set(id, { id, title, createdAt: at, updatedAt: at, importedFrom: input.provider, messages: [{ id: `${id}-m1`, role: 'user', text: `${title}에 대해 물어봅니다.`, createdAt: at, attachments: [] }, { id: `${id}-m2`, role: 'assistant', text: '가져온 답입니다.', createdAt: at, attachments: [] }] })
    })
    send(res, 200, { imported, duplicate: titles.length - imported, skipped: 0, updated: 0, addedMessages: 0, conflicts: 0, omittedAttachments: 0 })
    return
  }
  const cancelMatch = url.pathname.match(/^\/chat-imports\/([^/]+)\/cancel$/)
  if (cancelMatch && req.method === 'POST') {
    const requestId = decodeURIComponent(cancelMatch[1])
    if (importCancels.has(requestId)) importCancels.set(requestId, true)
    send(res, 200, { requestId })
    return
  }
  if (url.pathname === '/assets/search' && req.method === 'GET') {
    const term = (url.searchParams.get('q') ?? '').trim()
    if (badText(term)) { send(res, 400, { error: 'invalid_arguments' }); return }
    const items = [...assets.values()].filter((a) => !trashed.has(a.id) && a.originalFileName.toLowerCase().includes(term.toLowerCase())).reverse().slice(0, 50)
    send(res, 200, { items, offset: 0, limit: 50 })
    return
  }
  if (url.pathname === '/media' && req.method === 'GET') {
    const offset = Number(url.searchParams.get('offset') ?? 0)
    const limit = Number(url.searchParams.get('limit') ?? 50)
    if (!(limit >= 1 && limit <= 100)) { send(res, 400, { error: 'invalid_arguments' }); return }
    const kind = url.searchParams.get('kind')
    const latest = new Map()
    for (const g of generations.values()) { if (trashed.has(g.assetId)) continue; const cur = latest.get(g.lineageId); if (!cur || g.version > cur.version) latest.set(g.lineageId, g) }
    const list = [...assets.values()].filter((a) => (a.kind === 'image' || a.kind === 'video') && !trashed.has(a.id) && (!kind || a.kind === kind)).filter((a) => { const g = generations.get(a.id); return !g || latest.get(g.lineageId)?.assetId === a.id })
    const items = list.reverse().slice(offset, offset + limit).map((a) => {
      const g = generations.get(a.id)
      const base = { assetId: a.id, kind: a.kind, name: a.originalFileName, mediaType: a.mediaType, byteLength: a.byteLength, prompt: null, lineageId: null, version: null, versionOf: null, versionHow: null, versionCount: null, chatAvailable: false }
      if (!g) return base
      return { ...base, ...g, versionCount: [...generations.values()].filter((x) => x.lineageId === g.lineageId).length, chatAvailable: chats.has(g.chatId) }
    })
    send(res, 200, { items, offset, limit, pendingGenerations: 0 })
    return
  }
  const lineageMatch = url.pathname.match(/^\/media\/lineages\/([^/]+)\/versions$/)
  if (lineageMatch && req.method === 'GET') {
    const id = decodeURIComponent(lineageMatch[1])
    const versions = [...generations.values()].filter((g) => g.lineageId === id).sort((a, b) => a.version - b.version).map((g) => ({ ...g, deleted: trashed.has(g.assetId) }))
    send(res, 200, { lineageId: id, versions })
    return
  }
  if (path[0] === 'media' && path[1] && path[2] === 'download' && req.method === 'GET') {
    const asset = assets.get(decodeURIComponent(path[1]))
    if (!asset || (asset.kind !== 'image' && asset.kind !== 'video')) { send(res, 404, { error: 'not_found' }); return }
    if (trashed.has(asset.id)) { send(res, 409, { error: 'asset_trashed' }); return }
    const data = assetBytes.get(asset.id) ?? Buffer.alloc(0)
    res.writeHead(200, { 'Content-Type': 'application/octet-stream', 'Content-Disposition': `attachment; filename*=UTF-8''${encodeURIComponent(asset.originalFileName)}`, 'X-Content-Type-Options': 'nosniff', 'Content-Length': data.length })
    res.end(data)
    return
  }
  if (url.pathname === '/assets/trash' && req.method === 'GET') {
    const offset = Number(url.searchParams.get('offset') ?? 0)
    const limit = Number(url.searchParams.get('limit') ?? 50)
    const items = [...trashed.entries()].sort((a, b) => b[1].localeCompare(a[1])).slice(offset, offset + limit).map(([id, deletedAt]) => { const a = assets.get(id); return { assetId: id, name: a.originalFileName, mediaType: a.mediaType, byteLength: a.byteLength, deletedAt } })
    send(res, 200, { items, offset, limit })
    return
  }
  if (path[0] === 'assets' && path[1] && (path[2] === 'trash' || path[2] === 'restore') && !path[3] && req.method === 'POST') {
    const id = decodeURIComponent(path[1])
    if (!assets.has(id)) { send(res, 404, { error: 'asset_not_found' }); return }
    const toTrash = path[2] === 'trash'
    const changed = toTrash !== trashed.has(id)
    if (toTrash && changed) trashed.set(id, now())
    if (!toTrash) trashed.delete(id)
    send(res, 200, { assetId: id, deleted: toTrash, changed, recordStatus: changed ? 'recorded' : null })
    return
  }
  if (path[0] === 'assets' && path[1] && path[2] === 'download' && req.method === 'GET') {
    const asset = assets.get(decodeURIComponent(path[1]))
    if (!asset) { send(res, 404, { error: 'asset_not_found' }); return }
    if (trashed.has(asset.id)) { send(res, 409, { error: 'asset_trashed' }); return }
    const bytes = assetBytes.get(asset.id) ?? Buffer.alloc(0)
    res.writeHead(200, { 'Content-Type': 'application/octet-stream', 'Content-Disposition': `attachment; filename*=UTF-8''${encodeURIComponent(asset.originalFileName)}`, 'X-Content-Type-Options': 'nosniff', 'Content-Length': bytes.length })
    res.end(bytes)
    return
  }
  if (url.pathname === '/document-folders/rename' && req.method === 'POST') {
    const input = await body(req)
    const from = typeof input?.from === 'string' ? input.from : ''
    const to = typeof input?.to === 'string' ? input.to : ''
    const bad = (p) => !p || p.split('/').some((x) => !x || x !== x.trim() || x === '.' || x === '..' || x.includes('\\'))
    if (bad(from) || bad(to)) { send(res, 400, { error: 'invalid_folder' }); return }
    if (!allFolders().includes(from)) { send(res, 404, { error: 'folder_not_found' }); return }
    const move = (p) => (p === from ? to : p.startsWith(`${from}/`) ? to + p.slice(from.length) : p)
    for (const d of documents) { if (d.folder !== move(d.folder)) { d.folder = move(d.folder); commitDoc(d) } }
    for (const p of [...extraFolders]) { extraFolders.delete(p); extraFolders.add(move(p)) }
    send(res, 200, { from, to })
    return
  }
  if (url.pathname === '/document-folders') {
    if (req.method === 'GET') { send(res, 200, { folders: allFolders() }); return }
    if (req.method === 'POST') {
      const input = await body(req)
      const folder = typeof input?.folder === 'string' ? input.folder : ''
      const parts = folder.split('/')
      if (!folder || parts.some((p) => !p || p !== p.trim() || p === '.' || p === '..' || /[\\\u0000-\u001f]/.test(p))) { send(res, 400, { error: 'invalid_folder' }); return }
      extraFolders.add(folder)
      send(res, 200, { folder })
      return
    }
  }
  if (path[0] === 'documents' && path[1] && path[2]) {
    const doc = documents.find((d) => d.id === decodeURIComponent(path[1]))
    if (!doc) { send(res, 404, { error: 'document_not_found' }); return }
    const versions = docVersions.get(doc.id) ?? []
    if (req.method === 'GET' && path[2] === 'history') { send(res, 200, { versions: versions.map(({ revision, time }) => ({ revision, time })) }); return }
    if (req.method === 'GET' && path[2] === 'versions') {
      const v = versions.find((x) => x.revision === path[3])
      if (!v) { send(res, 404, { error: 'version_not_found' }); return }
      send(res, 200, { id: doc.id, title: v.title, summary: v.summary, folder: v.folder, body: v.body, assets: [], edges: [] })
      return
    }
    if (req.method === 'POST' && path[2] === 'restore') {
      const input = await body(req)
      if (dirtyDocs.has(doc.id)) { send(res, 409, { error: 'unsaved_changes' }); return }
      const v = versions.find((x) => x.revision === input?.revision)
      if (!v) { send(res, 404, { error: 'version_not_found' }); return }
      if (input.expectedRevision !== docRevision(doc)) { send(res, 409, { error: 'conflict' }); return }
      Object.assign(doc, { title: v.title, summary: v.summary, folder: v.folder, body: v.body, updatedAt: now() })
      commitDoc(doc)
      send(res, 200, { id: doc.id, revision: docRevision(doc) })
      return
    }
  }
  if (path[0] === 'documents' && path[1] && req.method === 'GET') {
    const doc = documents.find((d) => d.id === decodeURIComponent(path[1]))
    if (!doc) { send(res, 404, { error: 'document_not_found' }); return }
    send(res, 200, { ...doc, kind: 'note', status: 'draft', revision: docRevision(doc) })
    return
  }
  if (path[0] === 'documents' && path[1] && req.method === 'PUT') {
    const doc = documents.find((d) => d.id === decodeURIComponent(path[1]))
    if (!doc) { send(res, 404, { error: 'not_found' }); return }
    const input = await body(req)
    if (typeof input?.revision !== 'string') { send(res, 400, { error: 'revision_required' }); return }
    if (input.revision !== docRevision(doc)) { send(res, 409, { error: 'conflict' }); return }
    Object.assign(doc, { title: String(input.title ?? ''), summary: String(input.summary ?? ''), body: String(input.body ?? ''), folder: String(input.folder ?? ''), updatedAt: now() })
    commitDoc(doc)
    send(res, 200, { id: doc.id, revision: docRevision(doc) })
    return
  }
  if (url.pathname === '/saved-logins' && req.method === 'GET') { send(res, 200, { logins: savedLogins }); return }
  if (path[0] === 'saved-logins' && path[1]) {
    const at = savedLogins.findIndex((l) => l.id === path[1])
    if (at < 0) { send(res, 404, { error: 'not_found' }); return }
    if (req.method === 'DELETE') { savedLogins.splice(at, 1); send(res, 200, { state: 'deleted' }); return }
    if (req.method === 'PUT') {
      const input = await body(req)
      const { password, ...rest } = input ?? {}
      savedLogins[at] = { ...savedLogins[at], ...rest }
      send(res, 200, { login: savedLogins[at] })
      return
    }
  }

  if (url.pathname === '/chat-groups' || url.pathname.startsWith('/chat-groups/')) {
    if (req.method === 'GET' && url.pathname === '/chat-groups') {
      const live = [...chats.values()].filter((c) => !c.archivedAt)
      send(res, 200, { groups: [...chatGroups].sort((a, b) => a.order - b.order).map((g) => ({ ...g, chatCount: live.filter((c) => c.groupId === g.id).length })) })
      return
    }
    if (req.method === 'POST' && url.pathname === '/chat-groups') {
      const input = await body(req)
      const name = typeof input?.name === 'string' ? input.name.trim() : ''
      if (badText(name)) { send(res, 400, { error: 'invalid_group_name' }); return }
      const group = { id: `group-${++groupSeq}`, name, order: Math.max(-1, ...chatGroups.map((g) => g.order)) + 1, createdAt: now() }
      chatGroups.push(group)
      send(res, 200, { id: group.id, name, createdAt: group.createdAt })
      return
    }
    if (req.method === 'POST' && url.pathname === '/chat-groups/reorder') {
      const input = await body(req)
      const ids = Array.isArray(input?.ids) ? input.ids : []
      if (ids.length !== chatGroups.length || !chatGroups.every((g) => ids.includes(g.id))) { send(res, 409, { error: 'group_order_changed' }); return }
      ids.forEach((id, at) => { chatGroups.find((g) => g.id === id).order = at })
      send(res, 200, { ids })
      return
    }
    const group = chatGroups.find((g) => g.id === path[1])
    if (!group) { send(res, 404, { error: 'group_not_found' }); return }
    if (req.method === 'PATCH') {
      const input = await body(req)
      const name = typeof input?.name === 'string' ? input.name.trim() : ''
      if (badText(name)) { send(res, 400, { error: 'invalid_arguments' }); return }
      group.name = name
      send(res, 200, { id: group.id, name })
      return
    }
    if (req.method === 'DELETE') {
      chatGroups.splice(chatGroups.indexOf(group), 1)
      for (const c of chats.values()) if (c.groupId === group.id) c.groupId = null
      send(res, 200, { id: group.id, deletedAt: now() })
      return
    }
  }
  if (url.pathname === '/chats/search' && req.method === 'GET') {
    const term = (url.searchParams.get('q') ?? '').trim()
    if (badText(term)) { send(res, 400, { error: 'invalid_query' }); return }
    const low = term.toLowerCase()
    const found = [...chats.entries()].filter(([, c]) => c.title.toLowerCase().includes(low) || c.messages.some((m) => m.text.toLowerCase().includes(low)))
      .sort(([, a], [, b]) => b.updatedAt.localeCompare(a.updatedAt)).slice(0, 100).map(([id, c]) => ({ id, title: c.title, updatedAt: c.updatedAt }))
    send(res, 200, { chats: found })
    return
  }
  if (url.pathname === '/chats/archived' && req.method === 'GET') {
    const list = [...chats.entries()].filter(([, c]) => c.archivedAt).map(([id, c]) => ({ ...chatEntry(id, c), archivedAt: c.archivedAt }))
    list.sort((a, b) => b.archivedAt.localeCompare(a.archivedAt))
    send(res, 200, { chats: list.map(({ messageCount, ...rest }) => rest) })
    return
  }
  if (url.pathname === '/chats') {
    if (req.method === 'GET') {
      const list = [...chats.entries()].filter(([, c]) => !c.archivedAt).map(([id, c]) => chatEntry(id, c))
      list.sort((a, b) => (a.pinnedOrder ?? Infinity) - (b.pinnedOrder ?? Infinity) || b.updatedAt.localeCompare(a.updatedAt))
      send(res, 200, { chats: list })
      return
    }
    if (req.method === 'POST') {
      const input = await body(req)
      if (input === null) { send(res, 400, { error: 'invalid_json' }); return }
      const chatId = `chat-${++seq}`
      const at = now()
      chats.set(chatId, { title: typeof input.title === 'string' ? input.title : '', createdAt: at, updatedAt: at, messages: [] })
      send(res, 200, { chatId })
      return
    }
  }

  if (path[0] === 'chats' && path[1]) {
    const chat = chats.get(path[1])
    if (!chat) { send(res, 404, { error: 'chat_not_found' }); return }
    if (req.method === 'PATCH' && path.length === 2) {
      const input = await body(req)
      const title = typeof input?.title === 'string' ? input.title.trim() : ''
      if (badText(title)) { send(res, 400, { error: 'invalid_title' }); return }
      chat.title = title
      chat.updatedAt = now()
      send(res, 200, { chatId: path[1], title })
      return
    }
    if (req.method === 'POST' && path[2] === 'archive') {
      const input = await body(req)
      chat.archivedAt = input?.archived === true ? now() : null
      chat.updatedAt = now()
      send(res, 200, { chatId: path[1], archivedAt: chat.archivedAt })
      return
    }
    if (req.method === 'DELETE' && path.length === 2) {
      if ([...turns.values()].some((t) => t.chatId === path[1] && t.state === 'running')) { send(res, 409, { error: 'turn_in_progress' }); return }
      chats.delete(path[1])
      send(res, 200, { chatId: path[1], deletedAt: now() })
      return
    }
    if (req.method === 'PUT' && path[2] === 'group') {
      const input = await body(req)
      const groupId = typeof input?.groupId === 'string' ? input.groupId : null
      if (groupId && !chatGroups.some((g) => g.id === groupId)) { send(res, 404, { error: 'group_not_found' }); return }
      chat.groupId = groupId
      send(res, 200, { chatId: path[1], groupId })
      return
    }
    if (req.method === 'POST' && path[2] === 'pin') {
      const input = await body(req)
      if (input?.pinned === true) chat.pinnedOrder ??= Math.max(-1, ...[...chats.values()].map((c) => c.pinnedOrder ?? -1)) + 1
      else chat.pinnedOrder = null
      send(res, 200, { chatId: path[1], pinned: input?.pinned === true })
      return
    }
    if (req.method === 'GET' && path.length === 2) {
      // 사용자 말에 가장 최근 시도의 turnId·turnState·turnError(S17 "다시 연 채팅").
      const messages = chat.messages.map((m) => { if (m.role !== 'user') return m; const t = [...turns.values()].filter((x) => x.userMessageId === m.id).pop(); return { ...m, turnId: t?.turnId ?? null, turnState: t?.state ?? null, turnError: t?.error ?? null } })
      const live = [...turns.values()].find((t) => t.chatId === path[1] && t.state === 'running')
      send(res, 200, { chatId: path[1], title: chat.title, createdAt: chat.createdAt, updatedAt: chat.updatedAt, messages, activeTurn: live ? { turnId: live.turnId, state: 'running', offset: bytes(live.text) } : null })
      return
    }
    if (req.method === 'POST' && path[2] === 'turns') {
      const input = await body(req)
      const text = typeof input?.text === 'string' ? input.text.trim() : ''
      if (!text) { send(res, 400, { error: 'empty_message' }); return }
      if ([...turns.values()].some((t) => t.chatId === path[1] && t.state === 'running')) { send(res, 409, { error: 'turn_in_progress' }); return }
      const provider = input.provider ?? 'codex'
      if (!['codex', 'claude', 'gemini'].includes(provider)) { send(res, 400, { error: 'invalid_provider' }); return }
      if (process.env.FAKE_MODELS !== 'none') {
        const found = FAKE_MODEL_LISTS[provider].find((m) => m.id === input.model)
        if ((provider !== 'codex' || input.model !== undefined) && (!found || modelState(provider) !== 'connected')) { send(res, 400, { error: 'invalid_model' }); return }
        const levels = (found?.efforts ?? []).map((e) => (typeof e === 'string' ? e : e.id))
        if (input.effort !== undefined && !levels.includes(input.effort)) { send(res, 400, { error: 'model_setting_unavailable' }); return }
        if (input.fast === true && !found?.fast?.supported) { send(res, 400, { error: 'fast_unavailable' }); return }
      }
      if (input.skill !== undefined && !skills.some((s) => s.name === input.skill && s.state === 'enabled')) { send(res, 409, { error: 'skill_unavailable' }); return }
      const parent = typeof input.regenerateOf === 'string' ? generations.get(input.regenerateOf) : null
      if (input.regenerateOf !== undefined && (!parent || parent.chatId !== path[1] || provider !== 'codex')) { send(res, 400, { error: 'media_generation_unavailable' }); return }
      if (parent) {
        const versions = [...generations.values()].filter((g) => g.lineageId === parent.lineageId)
        const id = `asset-regen-${++seq}`
        const old = assets.get(parent.assetId)
        assets.set(id, { ...old, id, originalFileName: `regenerated-${seq}${old.originalFileName.slice(old.originalFileName.lastIndexOf('.'))}` })
        assetBytes.set(id, assetBytes.get(parent.assetId))
        generations.set(id, { ...parent, assetId: id, prompt: text, promptSource: 'userRequest', createdAt: now(), version: Math.max(...versions.map((g) => g.version)) + 1, versionOf: parent.assetId, versionHow: 'regenerated' })
      }
      lastTurn = { provider, model: input.model ?? null, effort: input.effort ?? null, fast: input.fast ?? null, skill: input.skill ?? null, regenerateOf: input.regenerateOf ?? null, textLength: text.length }
      const mode = input.permissionMode ?? 'Ask for approval'
      if (!['Ask for approval', 'Approve for me', 'Full access'].includes(mode)) { send(res, 400, { error: 'invalid_permission_mode' }); return }
      const userMessageId = `m${++seq}`
      const wanted = Array.isArray(input.attachments) ? input.attachments : []
      if (wanted.some((a) => !assets.has(a?.assetId))) { send(res, 400, { error: 'invalid_attachment' }); return }
      const attachments = wanted.map((a) => { const asset = assets.get(a.assetId); return { assetId: asset.id, name: asset.originalFileName, mediaType: asset.mediaType, byteLength: asset.byteLength } })
      chat.messages.push({ id: userMessageId, role: 'user', text, createdAt: now(), attachments, ...(typeof input.skill === 'string' ? { skill: input.skill } : {}) })
      chat.updatedAt = now()
      const turn = startTurn(path[1], userMessageId, text, null, mode)
      send(res, 200, { turnId: turn.turnId, userMessageId })
      return
    }
  }

  if (path[0] === 'turns' && path[1]) {
    const turn = turns.get(path[1])
    if (!turn) { send(res, 404, { error: 'turn_not_found' }); return }
    if (req.method === 'GET' && path.length === 2) {
      send(res, 200, { turnId: turn.turnId, chatId: turn.chatId, state: turn.state, text: turn.text, offset: bytes(turn.text), error: turn.error })
      return
    }
    if (req.method === 'GET' && path[2] === 'approval') {
      const a = turn.approval?.state === 'pending' ? turn.approval : null
      send(res, 200, { approval: a && { requestId: a.requestId, chatId: a.chatId, turnId: a.turnId, provider: a.provider, kind: a.kind, description: a.description, options: a.options } })
      return
    }
    if (req.method === 'GET' && path[2] === 'browser-events') { send(res, 200, { events: turn.browserEvents }); return }
    if (req.method === 'GET' && path[2] === 'requests') {
      const a = turn.approval?.state === 'pending' ? turn.approval : null
      send(res, 200, { approval: a && { requestId: a.requestId, chatId: a.chatId, turnId: a.turnId, provider: a.provider, kind: a.kind, description: a.description, options: a.options }, inputs: turn.inputs.filter((item) => item.state === 'pending').map(inputView) })
      return
    }
    if (req.method === 'POST' && path[2] === 'cancel') {
      if (turn.state === 'running') turn.end('failed', 'cancelled')
      send(res, 200, { turnId: turn.turnId, state: turn.state })
      return
    }
    if (req.method === 'POST' && path[2] === 'retry') {
      if (turn.state !== 'failed') { send(res, 409, { error: 'turn_not_failed' }); return }
      // 같은 실패 턴의 거듭된 요청에는 이미 만든 재시도 턴을 돌려준다. 내 말은 새로 남기지 않는다.
      if (turn.retried) { send(res, 200, { turnId: turn.retried, userMessageId: turn.userMessageId }); return }
      const again = startTurn(turn.chatId, turn.userMessageId, turn.prompt, turn.turnId, turn.permissionMode)
      turn.retried = again.turnId
      send(res, 200, { turnId: again.turnId, userMessageId: turn.userMessageId })
      return
    }
    if (req.method === 'GET' && path[2] === 'events') {
      const after = Number(url.searchParams.get('after') ?? 0)
      const whole = Buffer.from(turn.text, 'utf8')
      if (!Number.isInteger(after) || after < 0 || after > whole.length || (after < whole.length && (whole[after] & 0xc0) === 0x80)) { send(res, 400, { error: 'invalid_offset' }); return }
      res.writeHead(200, { 'Content-Type': 'text/event-stream; charset=utf-8', 'Cache-Control': 'no-store', Connection: 'keep-alive' })
      const write = (event, data) => res.write(`event: ${event}\ndata: ${JSON.stringify({ turnId: turn.turnId, chatId: turn.chatId, ...data })}\n\n`)
      const rest = whole.subarray(after).toString('utf8')
      if (rest) write('chat-delta', { delta: rest, offset: whole.length })
      if (turn.state !== 'running') { write('chat-done', { state: turn.state, error: turn.error }); res.end(); return }
      const approvalView = () => { const a = turn.approval; return { requestId: a.requestId, provider: a.provider, kind: a.kind, description: a.description, options: a.options } }
      // 다시 붙으면 대기 중인 물음과 사람 할 일을 다시 보낸다(server 와 같음).
      if (turn.approval?.state === 'pending') write('agent-approval-request', approvalView())
      for (const event of turn.browserEvents) write('browser-human-required', event)
      for (const item of turn.inputs) if (item.state === 'pending') write('agent-input-request', inputView(item))
      let sent = 0
      const dropThis = process.env.FAKE_DROP === '1' && turn.drops === 0
      const listener = (change) => {
        if (change.type === 'approval') { write('agent-approval-request', approvalView()); return }
        if (change.type === 'approval-closed') { write('agent-approval-closed', { requestId: change.approval.requestId, reason: change.reason, decision: change.decision }); return }
        if (change.type === 'input') { write('agent-input-request', inputView(change.item)); return }
        if (change.type === 'input-closed') { write('agent-input-closed', { requestId: change.item.requestId }); return }
        if (change.type === 'human') { write('browser-human-required', change.event); return }
        if (change.type === 'delta') {
          write('chat-delta', { delta: change.delta, offset: bytes(turn.text) })
          sent += 1
          if (dropThis && sent === 6) { turn.drops += 1; turn.listeners.delete(listener); res.destroy() }
        } else {
          write('chat-done', { state: turn.state, error: turn.error })
          turn.listeners.delete(listener)
          res.end()
        }
      }
      turn.listeners.add(listener)
      req.on('close', () => turn.listeners.delete(listener))
      return
    }
  }

  if (req.method === 'GET') { await serveWeb(res, url.pathname); return }
  send(res, 404, { error: 'not_found' })
}).listen(port, '127.0.0.1', () => console.log(`fake momso server on http://localhost:${port}${webDir ? ' (화면 파일 같은 출처)' : ''}`))
