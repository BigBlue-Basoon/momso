# S36: Claude와 Gemini 연결 준비 (미완료)

## S138 현재 상태

아래의 “서버 입력·채팅 미연결”은 S36 당시 기록이다. 현재 Linux server는 Claude·Gemini API 키 요청과 일반 채팅을 연결한다. `GET /providers`는 `{providers:[{provider,state,accountLabel}]}`이고 두 회사의 상태는 `credential_saved`, `not_connected`, `storage_unavailable`이다. `credential_saved`는 저장 성공이지 키 유효성 검증 성공이 아니다. Linux 외에는 `not_supported`다.

`POST /providers/{provider}/key/request`는 `provider_api_key` 입력 객체를 만들고, `GET` 같은 경로는 `{inputs:[...]}`로 재조회한다. `provider`는 `claude` 또는 `gemini`만 허용하며 그 밖은 `invalid_provider`다. 입력 객체는 `requestId`, 빈 `chatId`·`turnId`, `kind`, `origin:null`, `provider`, `expiresAt`, `fields:[{id:"apiKey",secret:true}]`, 빈 `choices`를 담는다. `POST /input-requests/{id}/respond`에 `{apiKey}`를 보내면 `{requestId,state:"credential_saved"}`이고 비밀값을 반환하지 않는다. 누락된 입력은 `invalid_input`, 형식에 맞지 않는 키는 `invalid_key`다. 취소는 `{requestId,state:"cancelled"}`, 닫히거나 만료된 요청의 응답은 `request_closed`다. 요청 생성·재조회 응답은 `no-store`다.

S138에서 `run_provider`를 S40 도구 루프에 연결했다. Codex·Claude·Gemini는 브라우저·CRM·스킬·사전 정의와 서버 디스패처를 공유하며 기존 승인·사람 입력·취소 경로를 사용한다. 승인 응답의 `provider`는 해당 턴의 공급자다. 공급자 응답은 각 도구 차례마다 완성된 응답을 받은 뒤 기존 `chat-delta`와 `chat-done`으로 내보내므로 응답이 표시되기까지 기다릴 수 있다. 첨부파일·생각 정도·고속 설정도 기존 형식을 유지한다. 가짜 API를 통한 실제 서버의 CRM 왕복과 격리 브라우저 시험은 실제 계정에서의 동작을 뜻하지 않는다. [Claude 도구 사용](https://platform.claude.com/docs/en/agents-and-tools/tool-use/overview), [Gemini 함수 호출](https://ai.google.dev/gemini-api/docs/function-calling).

검증 범위: `provider_keys_and_chat_use_encrypted_vault_and_fake_api`는 실행 중인 서버에서 두 가짜 공급자의 CRM 호출·도구 결과·답과 첨부파일·모델 설정을 확인한다. `both_provider_server_browser_approval_round_trip`은 실행 중인 서버에서 두 가짜 공급자가 격리 Chromium·가짜 사이트·승인 카드를 거쳐 등록하는 흐름을 확인한다. `fake_cli_browser_site_approval_round_trip`은 Codex의 서버 경로에서 승인과 거절을 확인한다. 여러 도구 호출·오류·사람 입력 대기·취소는 두 공급자의 가짜 도구 모듈 시험이다. 세 공급자를 하나의 동일한 시험으로 비교한 것은 아니며, **실제 공급자 계정과 휴대폰에서의 사람 입력 왕복은 아직 확인하지 않았다.**

이 문서는 서버에 연결된 기능의 명세가 아니라 독립 모듈을 잇기 위한 작업 기록이다. `apps/alpha/server`와 화면은 바꾸지 않았다. API 전송과 취소는 독립 모듈에 구현했으나 서버의 입력 요청·채팅에 연결하지 않았다. 브라우저·CRM 도구 연결도 구현하지 않았다. 따라서 통합 전 `/providers`에서 지원한다고 표시하면 안 된다.

## 인증과 공식 근거

- Anthropic의 [인증 문서](https://code.claude.com/docs/en/authentication)는 `claude setup-token`이 브라우저 승인을 거쳐 구독 OAuth 토큰을 출력한다고 설명한다. 같은 문서에서 `--bare`는 그 토큰을 읽지 않으며 API 키 또는 `apiKeyHelper`를 쓰도록 한다. [CLI 문서](https://code.claude.com/docs/en/cli-reference)는 `-p`, `stream-json`, `--mcp-config`를 설명한다. 향후 `-p` 기본값이 바뀐다는 예고의 현재 공식 근거는 확인하지 못했다.
- [Anthropic 약관 설명](https://code.claude.com/docs/en/legal-and-compliance)은 외부 제품이 사용자 Claude.ai 세션 토큰을 수집·보관하거나 구독 자격을 대신 사용하는 것을 제한하고 사용자 소유 API 키 경로를 설명한다. 공개된 수정되지 않은 Claude Code를 사용자가 직접 로그인하여 호스팅하는 경우는 별도로 언급한다. 몸소에 구독 토큰을 받아 보관하는 길은 만들지 않는다. 사용자가 모바일에서 Console API 키를 발급해 안전한 연결을 통해 직접 입력하는 길이 필요하다. 키 발급 자체가 로그인만으로 끝난다고 확인하지 못했다.
- 같은 문서의 원문 대목: “developers may not collect, store, or intermediate Claude.ai credentials or session tokens”; “route requests through Free, Pro, Max, or Ultra plan credentials”. 문서는 사용자가 수정하지 않은 공식 Claude Code 바이너리에 자기 구독으로 로그인하는 경우를 별도로 설명한다. **사용자의 자기 VM에서 몸소가 실행하는 정확한 방식과 데스크톱의 자기 PC에서 실행하는 정확한 방식이 각각 이 조건에 해당하는지는 확인 못 함.** 이 두 경우를 허용 또는 금지로 단정하지 않는다.
- Gemini는 이번 제품 결정에 따라 API 키만 사용한다. [Google 공식 시작 안내](https://ai.google.dev/gemini-api/docs/get-started)는 AI Studio 키 발급과 `x-goog-api-key` 헤더를, [스트리밍과 도구](https://ai.google.dev/gemini-api/docs/streaming)는 스트림과 함수 호출을 설명한다. 모바일에서 AI Studio에 로그인하여 키를 만든 뒤 몸소에 직접 입력해야 한다. Google 계정만으로 몸소에 연결되는 길은 제공하지 않는다.

## 보관소와 보안 경계

S21의 `LinuxVault`는 `saved-logins` 항목 하나만 취급한다. 브라우저 로그인 보관소와 데이터를 덮어쓰지 않도록 서버 전용 자료 폴더의 `provider-credentials/claude`와 `provider-credentials/gemini`에 각각 독립된 S21 보관소를 둔다. 암호화 키는 S21과 같은 systemd 자격 증명으로 공급한다. 기존 S21 보관소와 파일을 공유하지 않는다. 백업과 복원 시 암호화 파일 및 복호화 자격 증명의 보존 정책은 서버 통합 측에서 정해야 한다.

인증된 사람의 전용 요청에서만 키를 받아야 하며, 모델 도구·명령 인자·로그·채팅·오류 원문에 넣지 않는다. 서버의 인증, TLS, 입력 요청 본문 기록 금지, 같은 OS 계정의 다른 프로세스에서 비밀 보호는 이 독립 모듈이 보장하지 않는다. 사람에게만 키를 받는 요청과 `/providers` 상태·이벤트 연동은 줄3이 추가해야 한다. 저장 성공은 유효한 API 키라는 뜻이 아니다.

줄3과 화면에 제안하는 모양(서버에 아직 연결되지 않음):

- 키는 아래 S32 입력 요청을 통해 인증된 사람만 TLS에서 전송한다. 응답·기록·분석에는 키를 넣지 않고 `Cache-Control: no-store`로 처리한다. 저장 결과 `credential_saved`는 서버 유효성 검증 전의 상태다.
- `GET /providers`: 저장소를 읽을 수 있고 키가 있으면 `credential_saved`, 없으면 `not_connected`, 저장소를 읽을 수 없으면 `storage_unavailable`로 제안한다. 아직 인증이 되었다는 `connected`를 임의로 내지 않는다. 상태 조회는 CLI 로그인이나 브라우저를 열지 않는다.
- 채팅 요청·중지와 `chat-delta`, `chat-done`은 S17의 기존 경로와 필드에 맞춘다. 회사 선택을 요청에 어떻게 실을지는 줄3 서버의 기존 저장 방식과 대조해야 하며 현재 미확정이다.
- S32 입력 요청을 재사용하는 대안: 서버가 `agent-input-request`에 `kind: "provider_api_key"`, `provider: "claude" | "gemini"`, `fields: [{id:"apiKey",secret:true}]`를 보낸다. `POST /input-requests/{requestId}/respond`의 `{apiKey}`는 인증된 사람만 한 번 제출하고 응답에 값을 넣지 않는다. 취소·만료·재시작은 S32 규칙 그대로다. `input::SecretInput`은 그 브로커를 연결하는 자리이며 직접 HTTP 서버는 만들지 않는다. 이 새 kind와 필드에 대한 화면·서버 처리는 줄3과 화면 담당이 추가해야 한다.

## 채팅과 도구 연결

S17의 `chat-delta`는 UTF-8 바이트 단위의 누적 `offset`, `chat-done`은 완료 또는 `failed/cancelled`를 쓴다. `transport::chat`은 공식 API의 SSE를 받아 조각과 누적 바이트를 호스트에 전하고 취소 신호를 받는다. 정확한 키가 여러 조각에 걸쳐 반복되면 안전한 앞부분만 내보낸 뒤 `sensitive_output`으로 중지한다. 서버는 원본 출력이나 오류 본문을 기록하면 안 된다. 이 방식은 정확한 키 문자열만 다루며 변형·인코딩된 비밀이나 다른 비밀까지 막지는 못한다. API 호출의 billing은 각 키 소유자에게 적용된다.

## S40 도구 호출

`tool_loop::chat_with_tools`는 두 회사의 API에서 도구 호출을 받아 호스트가 실행한 뒤 결과를 같은 대화에 이어 보낸다. [Anthropic 공식 문서](https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls)는 `tool_use`/`tool_result`를, [Google 공식 문서](https://ai.google.dev/gemini-api/docs/function-calling)는 `functionCall`/`functionResponse`를 설명한다. Gemini 모델의 원래 응답 부분은 `thoughtSignature`를 포함해 그대로 다음 요청에 보존한다. 회사별 스키마 변환과 도구 결과 형식은 이 부품이 맡는다.

`MomsoTools`는 S22 브라우저 일곱, S33 자료 다섯, S34의 `momso_skill_read`·`momso_dictionary_search`를 등록한다. 줄3은 `MomsoDispatch`의 세 경로를 기존 서버의 브라우저·자료·스킬/사전 실행 경로에 연결한다. 채팅·턴 번호와 취소 신호는 호스트가 전달하고 AI의 도구 인자가 정하지 못한다. 권한 단계는 서버가 해당 턴에 저장한 `permissionMode`를 호스트 디스패처가 보유해 기존 브라우저·자료 실행 경로에 넘기며, 모델 인자에서 받지 않는다. 기존 S32의 사이트 승인·저장 로그인·인증번호 입력과 결과 가림을 그 경로에서 그대로 사용해야 한다. 이 부품은 승인 카드를 직접 만들거나 AI가 승인하게 하지 않는다. 한 턴 32회 왕복·회당 32개 호출 한도, 등록되지 않은 이름·중복 호출 번호 거절, API 키가 결과에 섞인 경우 중지가 있다.

**서버 통합은 아직 안 됐다.** `MomsoDispatch`를 줄3의 기존 S32/S34 호스트와 연결한 뒤에야 실제 승인 카드·입력 카드와 같은 이벤트가 화면에 간다. 이 부품은 API를 직접 쓰며 실제 클로드·제미나이 CLI 연결은 제공하지 않는다. 도구를 쓰는 턴은 API 응답을 왕복마다 모은 뒤 안전한 텍스트를 `chat-delta`로 내보낸다. 따라서 한 API 응답을 받는 도중에는 글이 실시간으로 나오지 않는다. 중단하면 이미 내보낸 글은 호스트가 보존하며, 비밀값 가림을 위해 잠시 보류한 글은 내보내지 않을 수 있다. 도구 없는 기존 S36 경로는 실시간 조각을 유지한다. 실제 회사 계정·키와 실제 사이트는 시험하지 않는다.

격리 Chromium 시험은 S22가 운영에서 막는 루프백 주소를 시험 중에만 쓰기 위해 `fixture-loopback` 개발 기능을 켠다. 이 기능은 명시적으로 켠 디버그 빌드에서만 작동하며 릴리스에서 켜면 컴파일을 거부한다. 운영 서버에서는 켜지 않는다. S32의 실제 입력·승인 카드와 인증값 격리는 줄3의 서버 연결 검증이 따로 필요하다.

Linux VM에서 코드 `7deda011b` 기준 fmt·clippy와 일반 시험 15개가 통과했다. 기본 시험에서 건너뛰는 격리 브라우저 시험 `both_providers_approved_registration_with_s22_browser`도 따로 실행해 통과했다. 두 회사 각각 가짜 API → S22 브라우저 → 가짜 승인 → 로컬 가짜 사이트 등록 → 읽은 결과를 답으로 내보낸다. 가짜 CLI는 시험 요청을 제공하는 호출자이고 실제 AI CLI 연결을 확인한 것은 아니다. `denied_tool_call_never_reaches_the_site`는 거절 시 가짜 사이트에 가지 않는지, `crm_route_uses_real_store_and_its_approval`은 실제 S33 저장 부품이 쓰기를 거절하는지 확인한다.

별도로 S22 브라우저 패키지의 fmt·clippy와 일반 시험 5개도 통과했다. 그 패키지의 격리 Chromium 시험 3개는 이번 별도 실행에서 건너뛰었다. 위 S40 격리 Chromium 시험을 명시적으로 따로 실행한 결과와 구별한다.

`phone_secret_vault_then_fake_api_never_echoes_key`는 가짜 CLI 호출자가 말만 제공하고 S32 모양의 비밀 입력 → S21 암호화 저장·로드 → 가짜 Claude API 응답을 처리하는지 본다. 가짜 CLI는 테스트 호출자이며 실제 Claude·Gemini CLI를 사용하는 경로의 시험은 아니다. `fake_claude_and_gemini_stream_and_offsets`는 양쪽 가짜 API의 조각과 바이트 위치, `cancellation_preserves_already_emitted_text`는 응답 중 중단을 검증한다. 실제 휴대폰의 입력 카드와 줄3 서버가 이 모듈을 호출하는 연결도 별도로 필요하다.
