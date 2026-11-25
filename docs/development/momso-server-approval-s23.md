# momso server 도구와 승인 계약 S23

## S136 현재 상태

아래 내용은 S23·S32 당시 기록이다. 현재 라우터의 승인 응답은 `GET /turns/{id}/approval`의 `{approval}`, `GET /turns/{id}/requests`의 `{approval,inputs}`, `POST /approvals/{id}/respond`의 `{requestId,state:"answered"}`다. `choice`가 허용 목록 밖이면 `invalid_choice`, 이미 닫혔으면 `approval_closed`다. 입력 응답은 `{requestId,state:"answered"}` 또는 API 키의 `{requestId,state:"credential_saved"}`이며, 취소는 이미 닫힌 요청에도 `{requestId,state:"cancelled"}`다. 닫힌 입력의 응답은 `request_closed`, 유효하지 않은 값은 `invalid_input`이다.

현재 승인 조회의 `provider`는 코드에서 `codex`로 고정한다. Claude·Gemini의 별도 도구 루프는 현재 server의 일반 채팅 경로에 연결되지 않았으므로 다른 회사의 승인 카드가 완성됐다고 가정하지 않는다. `browser-human-required` 이벤트는 있으나 캡차·패스키를 휴대폰에서 직접 수행하는 화면은 미확인이다. 승인·입력 이벤트는 [server 시험](../../apps/alpha/server/tests/restart.rs)의 `approval_survives_reconnect_and_returns_to_fake_cli`, `verification_request_survives_reconnect_and_redacts_after_restart`, `requested_login_survives_reconnect_and_reaches_only_site`가 가짜 CLI·사이트로 확인한다. 실제 서비스 검증은 아니다.

**계약 판 S23-2026-11-21.2 — S34 자료 모으기 쓰기 승인 추가.**

## 범위와 인증

서버는 S27(`66ae18ddd`)의 브라우저·보관소 부품을 이용한다. Codex app-server의 실험적 `dynamicTools`를 `thread/start`에 등록하고 `item/tool/call`을 처리한다. 재개한 대화는 Codex에 저장된 도구 정의를 사용한다. 도구의 인자에는 승인이나 비밀번호를 넣지 않는다. [Codex app-server 공식 설명](https://developers.openai.com/codex/app-server/#dynamic-tool-calls-experimental).

모든 API와 이벤트는 기존 서버 토큰이 필요하다. 이 토큰은 사용자별 인증이 아니므로 서버를 인터넷에 직접 열지 않고, 인증과 TLS가 있는 비공개 통로 뒤에서 사용한다.

S34의 `momso_crm_upsert`와 `momso_crm_mark_read`는 `kind: "crm_write"` 승인 요청을 낸다. `description`은 도구 이름과 건수만 담으며, 이름·연락처·기록 본문은 담지 않는다. `options`는 `allow_once`, `deny`이고 응답·닫힘 이벤트·취소·만료는 기존 승인 계약과 같다. AI 권한 단계가 무엇이든 이 쓰기 승인은 따로 받는다. **기존 Codex 대화는 새 도구를 받지 못하는 한계가 있다.** 설치된 CLI의 실험용 JSON 스키마에서 `thread/start`에는 `dynamicTools`가 있지만 `thread/resume`에는 없다. 대화를 보존하면서 새 도구를 붙이는 길은 아직 확인하지 못했다.

## 승인과 브라우저

`POST /chats/{chatId}/turns`에 선택적인 `permissionMode`를 보낸다. 값은 `Ask for approval`(기본), `Approve for me`, `Full access`다. Codex에는 데스크톱과 같은 권한 단계를 넘기고, 브라우저에도 같은 단계를 적용한다. `Approve for me`의 브라우저 자동 심사는 아직 없어서 사람에게 묻는다. 재시도에도 원래 단계를 유지한다. `Full access`는 브라우저 사이트 승인도 건너뛰고 Codex에 전체 권한을 준다.

사람에게 물을 때 `agent-approval-request` SSE의 값은 `{requestId,chatId,turnId,provider,kind,description,options}`다. Codex의 명령·파일 승인 외에 `kind`가 `browser_site`면 `description`은 실제 사이트 호스트, `browser_login`이면 실제 사이트 출처다. 브라우저 요청의 `options`는 `allow_once`, `deny`다. AI가 도구 인자로 승인하거나 출처를 지정해 승인된 것으로 만들 수 없다. `GET /turns/{turnId}/approval`은 현재 대기 요청 또는 null을 주고, `POST /approvals/{requestId}/respond`의 `{choice}`로 답한다. 중단·5분 시간 초과·서버 재시작 때 대기 승인은 취소한다. 요청이나 대상이 바뀌면 브라우저 부품이 누르지 않는다. S27은 승인한 사이트를 같은 채팅방에서 기억한다.

승인·인증번호·처음 저장할 로그인은 공통으로 `GET /turns/{turnId}/requests`에서 `{approval,inputs}`를 조회한다. `approval`은 기존 승인 또는 null이고, `inputs`는 대기 중인 입력 요청이다. SSE는 기존 `agent-approval-request`와 새 `agent-input-request`를 보내며, 입력이 끝나면 `agent-input-closed {requestId,chatId,turnId}`를 보낸다. 입력 요청은 `{requestId,chatId,turnId,kind,origin,expiresAt,fields}`다. `kind`가 `one_time_code`면 필드는 `code`, `saved_login`이면 `label`, `username`, `password`다. 각 필드는 `{id,secret}`으로 구분한다. `POST /input-requests/{requestId}/respond`는 인증번호의 경우 `{code}`, 로그인은 `{label,username,password}`를 받으며 답에는 입력값이 없다. `POST /input-requests/{requestId}/cancel`은 요청을 닫고 이미 닫힌 요청도 오류 없이 처리한다. 답은 한 번만 받으며 잘못된 입력은 대기를 유지한다. 화면 연결이 끊겨도 같은 요청을 조회할 수 있고, 턴 중단·5분 경과·서버 재시작 시 닫힌다.

캡차·패스키 등은 우회하지 않고 `browser-human-required` 이벤트에 `{id,chatId,turnId,reason,origin}`을 준다. `GET /turns/{turnId}/browser-events`는 놓친 이벤트를 준다. 이 추가 조치를 휴대폰에서 직접 수행하는 화면은 아직 없다.

## 저장된 로그인

`POST /saved-logins`는 S21의 `{siteUrl,label,username,password,askBeforeUse?,loginPath?,defaultForPath?}`를 받아 암호화 보관소에 저장한다. 응답은 `{login: ...}`이며 비밀번호는 없다. 입력 오류는 `invalid_input`이고 보관소 오류는 S21의 고정 값이다. 요청 본문·비밀번호는 AI 도구, 채팅, 로그에 내보내지 않는다. 저장소는 systemd가 `CREDENTIALS_DIRECTORY/momso-vault-identity`로 전달한 키를 사용하고, 보관소를 열 수 없으면 브라우저 페이지도 AI에 내보내지 않는다. 사이트에 맞는 저장 계정이 없으면 위 `saved_login` 입력 요청을 내고, 사용자가 입력한 값을 현재 사이트 출처에 묶어 암호화 저장한 뒤 해당 사이트에만 넣는다. 계정이 하나면 그것을 쓰고 여럿이면 `select_saved_login` 요청을 낸다. 그 요청의 `choices`는 해당 사이트·경로에 맞는 `{id,label,username}`만 포함하며, 화면은 `POST /input-requests/{requestId}/respond`에 `{selectionId}`를 보낸다. 제시되지 않은 계정은 거절한다. 실제 로그인과 인증번호 제출에는 HTTPS를 요구한다(격리된 디버그 시험만 로컬 HTTP 허용).

Codex의 작업 폴더는 서버 자료와 분리하고 자식 환경에서 `CREDENTIALS_DIRECTORY`를 제거했다. 그러나 같은 OS 사용자로 실행되는 전체 권한 프로세스가 파일이나 프로세스에 접근하는 것까지 막지는 못한다. S21 문서의 계정 분리·호스트 보호 한계를 적용한다. S19b 유휴 종료는 승인 대기도 진행 중 작업으로 세어야 하며, 유휴 종료 자체는 아직 구현하지 않았다.


## 인증번호 재시작 뒤 가림

인증번호 원문은 요청 응답·채팅·로그·DB에 저장하지 않고 브라우저의 해당 확인 화면에 한 번만 넘긴다. 서버는 보관소의 systemd 키로 만든 HMAC과 길이만 DB에 보관한다. 페이지를 AI에 반환하기 전 그 HMAC으로 일치하는 표기를 가린다. 서버를 다시 켠 뒤 사이트가 같은 번호를 다시 보여도 적용한다. 키가 없거나, 번호 기록이 있는데 키가 바뀌어 검증할 수 없으면 브라우저 결과를 AI에 전달하지 않는다. 이때 원래 보관소 키를 복구해야 한다. 원문을 DB에 남기지 않지만 같은 OS 사용자 권한의 프로세스나 메모리 유출까지 막지는 못한다.

## 시험 경계

줄4 부품의 `apps/alpha/server-browser/src/linux.rs`와 `Cargo.toml`에 가짜 사이트 시험용 `fixture-sites` 기능만 더했다. 로컬 주소 예외는 그 기능을 명시한 디버그 빌드에서만 켜지며 릴리스에서는 켜지지 않는다. 실제 서비스에는 접속하지 않는다. `MOMSO_CHROMIUM_PATH`로 Chromium 실행 파일을 지정할 수 있다.

가짜 CLI가 실제 S27 브라우저로 가짜 사이트를 열고, 승인을 받은 뒤 누른 결과를 응답에 싣는 `fake_cli_browser_site_approval_round_trip`은 격리된 Chromium이 필요해 일반 시험에서는 제외하고 별도로 실행한다. 사용자 로그인·실제 AI·실제 외부 사이트는 시험하지 않았다.

## 검증 결과

Linux VM에서 코드 `b4ad37af5` 기준 일반 서버 시험 14개가 통과했다. 같은 실행에서 분리된 Chromium 시험 `fake_cli_browser_site_approval_round_trip`도 별도로 통과했다. 가짜 CLI → 실제 S27 브라우저 → 임시 사이트 → 승인 또는 거절 → CLI 결과 → 답 저장을 확인했고, 허락 시 등록 1회·거절 시 추가 등록 0회를 확인했다. `phone_saved_login_is_encrypted_and_never_echoed`는 가짜 휴대폰 입력이 S21 보관소에 암호화돼 저장되고 비밀번호가 응답·암호문에 평문으로 나오지 않음을 확인한다. fmt와 clippy도 통과했다.

Windows 전체 시험은 실행하지 않았다. 이 작업에서 가져온 Windows 공통 경로는 `apps/alpha/desktop/src-tauri/src/browser_engine.rs`와 `apps/alpha/desktop/src-tauri/saved-login-core/`이며 기존 Windows 데스크톱에 연결하지 않았다. 실제 Codex 계정, 실제 외부 사이트, 휴대폰 사용자 경로는 시험하지 않았다. S25에서 만든 이전 릴리스 파일에는 S23이 포함되지 않는다.

## 입력 요청과 재시작 가림 검증

Linux VM에서 코드 `7dc5e299c` 기준 일반 서버 시험 16개, 격리 Chromium 시험 3개, fmt·clippy가 통과했다. `verification_request_survives_reconnect_and_redacts_after_restart`는 대기 요청 재조회, 입력 이벤트와 닫힘 이벤트, 잘못된 입력 후 재입력, 중복 응답 거절, 사이트에 번호 전달, DB 원문 부재, 서버 재시작 뒤 번호 가림과 키 변경 시 차단을 확인한다. `requested_login_survives_reconnect_and_reaches_only_site`는 승인 → 로그인 입력 재조회 → 암호화 저장 → 같은 가짜 사이트에만 입력과 응답 가림을 확인한다. 실제 계정·외부 사이트는 시험하지 않았다.

S27과 겹치는 시험 전용 변경은 `apps/alpha/server-browser/Cargo.toml`의 `fixture-sites`와 `src/linux.rs`의 로컬 주소·HTTP 로그인 예외다. 명시적으로 해당 기능을 켠 디버그 시험에서만 사용하며 릴리스 빌드에는 적용되지 않는다.

## S32 화면과 설치 담당 계약

- 승인 카드가 닫히면 `agent-approval-closed {requestId,chatId,turnId,reason,decision}`이 온다. `reason`은 `responded`, `provider_cancelled`, `timeout`, `server_restarted`; `decision`은 `allow_once`, `allow_session`, `deny`, `cancel`이다. 재연결 후에는 `GET /turns/{turnId}/requests`의 현재 대기 목록을 정본으로 삼는다.
- 인증번호는 `one_time_code` 입력 요청에 `{code}`로 응답한다. 값은 응답·채팅·로그에 되돌리지 않는다. 취소는 `/input-requests/{requestId}/cancel`이다.
- `GET /saved-logins`는 `{logins:[...]}`를, `PUT /saved-logins/{id}`는 `{login:...}`, `DELETE /saved-logins/{id}`는 `{state:"deleted"}`를 반환한다. 비밀번호는 반환하지 않는다. 수정은 `siteUrl`, `label`, `username`, `password`, `askBeforeUse`, `loginPath`, `defaultForPath`, `favorite` 중 변경할 값을 보낸다. `password`를 생략하면 기존 비밀번호를 유지하고, `loginPath`에 빈 문자열을 보내면 경로 지정을 지운다.
- `GET /chats/{chatId}`의 `activeTurn`과 `GET /activity`는 서버 계약의 S32 절을 따른다.

S32 검증: Linux 서버 일반 시험 19개, 격리 Chromium 시험 3개, fmt·clippy가 통과했다. `approval_timeout_emits_closed_while_turn_continues`는 승인 시간 초과 이벤트와 이어지는 턴을, `cancel_then_retry_keeps_one_user_message_and_replays_by_offset`는 진행 턴 재조회와 0부터의 SSE를 확인한다. `requested_login_survives_reconnect_and_reaches_only_site`는 같은 사이트의 두 계정 가운데 제시된 계정만 선택되는지 확인한다. 인증번호 재접속·재시작 가림 시험도 통과했다. 실제 AI·사이트·휴대폰은 시험하지 않았다.

서버가 재시작되면 이전 이벤트 연결은 끊긴다. 이때 닫힘 이벤트 수신을 가정하지 말고 대기 목록을 다시 조회한다. `GET /activity`는 조회 순간의 상태이며 실제 VM 종료 기능이나 외부 백업 완료를 뜻하지 않는다.
