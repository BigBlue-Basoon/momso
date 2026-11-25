# momso 화면 인계: 시작·연결·삭제·업데이트

기준: 통합 `d79a3695f`의 코드와 아래 원문을 S150에서 대조했다. 이 문서는 화면 작업을 위한 계약·미결 사항의 색인이며, 화면 문구나 모양을 결정하지 않는다. 과거 문서의 완료 여부는 그 문서의 시험 시점에 한정된다. 가짜 시험, 실제 계정·기기 확인, 배포는 서로 다르다.

## 정해진 것

- 고객에게 보이는 이름은 `momso` 또는 몸소다. `momso anywhere`와 `momso desktop`은 두 제품을 구분할 때 쓴다.
- 삭제 선택은 자료·백업을 보존하는 `쉬기`와 자료까지 지우는 `전부 지우기` 두 단추로 구분한다. `전부 지우기`는 사람이 명시적으로 선택한다. 구현의 `discardData`와 PC의 `--discard-data`는 그 선택에 대응한다.
- Windows 코드 서명 인증서는 사지 않고 SmartScreen 경고를 받아들인다. **Tauri 자동 업데이트 서명은 별개로 유지**하며 개인키는 개인키 소유자가 다룬다. 조직 정책이 서명 없는 파일을 막을 수 있다. [S118](desktop-release-preparation-s118.md), [결정 기록](windows-distribution-signing-decision.md)
- 입구 이미지는 공개 GHCR의 서명 목록에 고정된 참조를 고객 Cloud Run이 직접 쓴다. 고객 Artifact Registry 원격 저장소는 기본으로 만들지 않는다. 실제 발행·고객 프로젝트 적용은 별도 확인이다. [S111](momso-anywhere-public-gateway-s111.md)
- 공통 로그인·시작 사이트는 무료 정적 호스팅 이전을 준비했다. 운영 주소는 `https://anywhere.momso.yoga`, 소개 사이트와 별개다. 도메인 연결·실제 게시·OAuth 변경은 확인되지 않았다. 무료 상품의 정책 변경과 도메인 비용까지 0원을 보장하지 않는다. [S114](momso-anywhere-static-host-s114.md), [PWA](momso-anywhere-pwa-entry-20261121.md)

## 시작 화면

출처: [S112](momso-anywhere-first-google-account-s112.md), [S64](momso-anywhere-start-s64.md), [S65](momso-anywhere-start-s65.md), [S26](momso-anywhere-onboarding-s26.md), 구현 `scripts/momso_anywhere_start.mjs`, `momso_anywhere_setup.mjs`, `momso_anywhere_google_browser.mjs`.

| 호출·사람의 단계 | 반환·조건 |
| --- | --- |
| `projectsFromGesture({clientId, release, projectId?, billingAccountName?})` | 기존 프로젝트 `{projectId, displayName}[]`; 새 프로젝트는 `projectId`를 생략한다. |
| `billingAccountsFromGesture({clientId, release, projectId?, billingAccountName?})` | 접근 가능한 열린 결제 계정 `{name, displayName}[]`; 여러 개면 선택값 `name`을 `billingAccountName`으로 전달한다. 기존 프로젝트의 결제 연결은 자동으로 바꾸지 않는다. |
| `startFromGesture({clientId, release, projectId?, billingAccountName?, onProgress})` | `{url}`. `onProgress`는 `{step,state}`. 설치 완료 뒤 `{url}`을 여는 사용자 동작이 별도다. |
| `findFromGesture({clientId, release})` | 조건을 확인한 고객 입구 URL 목록. 여러 개면 선택이 필요하다. |
| `resumeAfterRedirect({clientId, release, onProgress, ...})` | 최초 진입과 `pageshow` 복원 시 중복 없이 호출. 이어갈 일이 없으면 `null`; 있으면 `kind`가 `projects`, `billing`, `start`, `find`, `destroy`, `deletion_preview`, `redirecting` 중 하나인 결과. 목록은 `projects` 또는 `billingAccounts`, 나머지는 `result`, 선택은 `selection: {projectId,billingAccountName}`이다. |
| `retryLastAction({clientId, release, onProgress})` | 마지막 작업을 같은 `kind` 계약으로 재시도한다. 구형 `retryFromGesture`는 호환용이며 항상 Google로 이동한다. |
| `openGoogleSetup({destination})` | 사용자 동작에서 호출하고 Google을 별도 탭으로 연다. `cloud` → `https://console.cloud.google.com/`, `billing` → `https://console.cloud.google.com/billing`, `trial` → `https://cloud.google.com/free`, `quota` → `https://console.cloud.google.com/projectcreate`. 기록이 없는 첫 진입은 결제·체험에서 `billing`, 그 외에는 `projects` 목록 읽기를 기록한다. |
| `watchGoogleSetupReturn({clientId, release, onProgress, onResult, onError?, intervalMs?})` | 원래 탭이 보일 때 `visibilitychange`, `focus`, 기본 15초 간격으로 마지막 목록 읽기 또는 설치를 한 번씩 재시도한다. 성공하면 `onResult`에 `retryLastAction`과 같은 결과를 주고 감시를 끝낸다. 빈 결제 목록은 계속 기다린다. `{check, stop}`을 반환하며 화면이 이탈할 때 `stop()`으로 해제한다. 삭제는 자동 재시도하지 않는다. 토큰 만료 시 `onError`의 `authorization_expired`를 처리해 사람이 재인증해야 한다. |

리디렉션 중 `{redirecting:true}` 또는 `{kind:'redirecting'}`은 실패가 아니다. 접근 토큰은 메모리에만 두고 주소·저장소·로그에 남기지 않는다. `onProgress.step`은 `project`(프로젝트), `billing`(결제), `services`(API), `privateNetwork`(사설망), `server`(VM·서버), `gateway`(입구), `gatewayReady`(입구 준비); `state`는 `working`, `done`, `failed`다. 알 수 없는 예외가 해당 단계 이름으로 전달될 수도 있다.

사람만 Google에서 로그인·권한 동의·약관 결정·결제수단·본인 확인을 한다. Google 탭이 스스로 닫히거나 원래 탭으로 돌아온다는 보장은 없다. 원래 탭이 다시 보이면 감시가 확인한다. 목록 조회의 성공만으로 약관·카드 등록 완료를 확정할 수는 없다. 일반 403의 `permission_denied`에는 `checks: ['billing_account', 'terms', 'project_permissions']`가 붙으며 원인을 단정하지 않는다. `terms_not_accepted`, `card_required`는 현재 계약에 없다. 실제 새 계정·휴대폰 시험은 미확인이다.

### 시작·연결 까닭 `SetupError.step`

아래는 값별 기술적 뜻이며 화면 문구가 아니다. 출처: 위 구현 파일과 `scripts/momso_anywhere_backup_setup.mjs`, `momso_anywhere_cloud_run_revision.mjs`, `momso_anywhere_oauth_redirect.mjs`.

| 값 | 뜻 |
| --- | --- |
| `authorization_declined` | Google 인증 응답에 오류가 있다. |
| `authorization_expired` | 접근 인증이 만료됐거나 Google이 401을 반환했다. |
| `authorization_invalid` | OAuth 복귀의 state·범위·토큰 형식·기한을 확인하지 못했다. |
| `identity_unavailable` | Google 사용자 식별자를 확인하지 못했다. |
| `invalid_authorization` | 인증 입력 또는 허용되지 않은 작업이 잘못됐다. |
| `invalid_redirect` | 시작 페이지 출처·경로가 OAuth 복귀 조건에 맞지 않는다. |
| `invalid_destination` | 고정된 Google 설정 목적지 밖의 값이다. |
| `checkpoint_unavailable` | 이어가기 기록이 없거나 읽고 쓸 수 없다. |
| `invalid_checkpoint` | 설치 ID 또는 저장된 작업 선택을 확인하지 못했다. |
| `invalid_project` | 프로젝트 ID 형식이 맞지 않는다. |
| `project_required` | 삭제·미리 보기에 프로젝트 선택이 없다. |
| `project_unavailable` | 선택된 프로젝트를 읽지 못했다. |
| `project_ownership` | 프로젝트 ID·활성 상태 또는 필요한 소유 표식이 맞지 않는다. |
| `invalid_billing_account` | 결제 계정 이름 형식이 맞지 않는다. |
| `billing_account_missing` | 접근 가능한 활성 결제 계정이 없다. 실제 계정 자체가 없다는 확정은 아니다. |
| `billing_account_unavailable` | 선택한 결제 계정이 열려 있지 않거나 목록에 없다. |
| `billing_selection_required` | 접근 가능한 계정이 있어도 자동으로 고를 수 없다. |
| `billing_inactive` | 기존 결제 연결은 있으나 활성 상태가 아니다. |
| `billing_not_ready` | 연결 후 결제 활성 상태를 확인하지 못했다. |
| `billing_setup_required` | Google 요청이 HTTP 402를 반환했다. |
| `billing_unreachable` | 결제 사전 확인에 접속하지 못했다. |
| `billing_unavailable` | 결제 사전 확인이 성공 응답을 주지 않았다. |
| `permission_denied` | Google 요청이 HTTP 403을 반환했다. 원인을 하나로 단정하지 않는다. |
| `quota_exceeded` | Google 요청이 HTTP 429를 반환했다. 모든 할당량 오류가 429인 것은 아니다. |
| `network` | Google 요청 전송에 실패했다. |
| `google_unavailable` | Google 요청이 5xx를 반환했다. |
| `google_request` | 위에 분류되지 않은 실패 HTTP 응답이다. |
| `google_response` | Google 응답 형식·페이지 진행을 확인하지 못했다. |
| `invalid_api` | 허용된 API 주소가 아니다. |
| `invalid_installation` | 프로젝트 또는 설치 번호 형식이 맞지 않는다. |
| `provider_not_ready` | 설치 공급자에 필요한 단계가 없다. |
| `invalid_operation` | 클라우드 작업 이름·위치가 예상과 다르다. |
| `operation_failed` | 클라우드 비동기 작업이 실패했다. |
| `operation_timeout` | 클라우드 비동기 작업 완료를 제한 시간 안에 확인하지 못했다. |
| `role_propagating` | 새 역할의 IAM 전파 지연으로 판별된 응답이다. 정해진 횟수만큼 재시도한다. |
| `resource_ownership` | 자원의 이름·라벨·구성 등 소유 조건이 맞지 않는다. |
| `identity_ownership` | 서비스 계정의 이름·설명·상태가 맞지 않는다. |
| `role_ownership` | 역할의 설명·상태·권한이 맞지 않는다. |
| `secret_ownership` | 비밀값의 설치 라벨이 맞지 않는다. |
| `secret_versions` | 비밀값 버전 목록·상태를 확인하지 못했다. |
| `policy_unavailable` | 필요한 IAM 정책 또는 안전 조건을 확인하지 못했다. |
| `release_not_ready` | 서명된 배포 목록·신뢰 이미지 등 설치 입력을 확인하지 못했다. |
| `vault_not_ready` | 보관소 키의 초기 버전이 활성 상태가 아니다. |
| `server_not_ready` | VM 서버 또는 필요한 사설 인증서 지문이 준비되지 않았다. |
| `gateway_address` | 입구 주소를 확인하지 못했다. |
| `gateway_ownership` | 입구의 설치 라벨이 맞지 않는다. |
| `gateway_policy` | 입구 정책에 허용하지 않는 공개 구성원이 있다. |
| `certificate_changed` | 이전에 고정한 VM 인증서 지문이 달라졌다. |
| `gateway_image_invalid` | 입구 이미지의 고정 지문 형식이 맞지 않는다. |
| `gateway_image_mismatch` | 새 판 또는 트래픽을 받는 판의 이미지가 지정 지문과 다르다. |
| `gateway_traffic_unknown` | 이전 트래픽 구성·합계를 확인하지 못했다. |
| `gateway_not_ready` | 새 판·트래픽·입구 준비 조건이 맞지 않는다. |
| `gateway_probe_failed` | 입구의 공개 준비 확인이 성공하지 않았다. |
| `gateway_rollback_failed` | 입구 실패 뒤 이전 트래픽 복구도 실패했다. |
| `vm_unreachable` | 휴대폰의 쉬기 준비·표 확인에서 VM에 닿지 못했다. 쉬기가 항상 차단된다는 뜻은 아니다. |
| `vm_unconfirmed` | 전체 삭제 후에도 VM이 있는 것으로 조회됐다. |

## 로그인·홈 화면

S133: 유효한 소유자 세션의 일반 fetch가 VM에 닿지 않아 깨우기를 요청하면 입구는 `503`, `X-Momso-Response: gateway`, `Retry-After: 5`와 JSON `{"error":"waking","retryAfterSeconds":5}`를 반환한다. 화면은 지정된 시간만큼 기다린 후 요청을 다시 시도한다. 세션 없는 요청은 VM을 깨우지 않는다. 페이지 열기 요청은 별도의 200·자동 새로고침 화면을 받는다. 본부의 실제 확인은 **페이지 열기**를 통한 깨우기이며 `/health` 준비까지 약 40초가 걸렸다. fetch 깨우기와 시간 보장은 실제 미확인이다. 입구 프로세스 안에서 5초 동안 깨우기 요청을 묶으며, 여러 입구 인스턴스 사이의 전역 중복 제거는 보장하지 않는다.

출처: [S42](momso-anywhere-s42-login-upgrade.md), [PWA·입구](momso-anywhere-pwa-entry-20261121.md), [S114](momso-anywhere-static-host-s114.md).

- 공통 로그인은 전체 창 Google OIDC 이동이며 팝업·별도 몸소 비밀번호를 요구하지 않는다. 화면 HTML의 `#google-login`을 연결하고 `document.body.dataset.state`는 초기값 없음, `ready`, `working`, `error`, `invalid`다. `ready`와 `error`에서만 단추가 켜진다. Google 계정 선택·로그인은 사람이 한다. 세션 설정은 30일이며 실제 30일 기기 시험과 iPhone·Android 홈 화면 복귀는 별도다.
- 입구가 직접 거절하면 JSON `{"error":"<까닭>"}`와 `X-Momso-Response: gateway`를 반환한다. VM에서 전달된 응답은 `X-Momso-Response: upstream`이며 본문은 VM의 계약을 따른다. 표식이 없으면 Cloud Run·IAM 등 입구 앞단 또는 중간 전달 경로에서 나온 응답일 수 있으며 원인을 단정하지 않는다. 허용된 로그인 출처의 표 호출에서는 CORS가 이 머리글을 노출한다. 허용되지 않은 출처는 브라우저가 응답을 읽지 못할 수 있다. `POST /__momso/auth/logout`의 화면 단추 여부는 미결이다.
- 시작 페이지 `/start/`와 로그인 `/`은 같은 정적 출처의 다른 경로다. 홈 화면 추가 대상은 시작 페이지가 아니라 고객별 입구다. 시작 `config.json`의 `clientId`, `release.manifestUrl`, `release.publicKeySpki`, `release.channel`, `release.loginOrigin`, 운영 채널의 `release.trustBootstrap`은 공개 설정이나 변조를 막아야 한다. 실제 정적 배포·OAuth 출처와 redirect, CORS·헤더·아이콘은 별도 확인이다.

| 입구 까닭 | 뜻 |
| --- | --- |
| `login_required` | 유효한 로그인 세션이 없다. |
| `session_expired` | 서명이 확인된 소유자 세션의 30일 유효 기간이 지났다. |
| `login_failed` | 로그인 결과·검증·일회용 상태를 확인하지 못했다. |
| `not_owner` | 서명이 확인된 신원 또는 세션이 설정된 소유자와 다르다. |
| `origin_not_allowed` | 요청 출처가 허용된 출처와 다르다. |
| `invalid_host` | 입구의 Host가 설정과 다르다. |
| `preflight_not_allowed` | 출처 간 사전 요청의 메서드 또는 머리글이 허용되지 않는다. |
| `pause_requires_app_session` | 쉬기 준비에 필요한 앱 세션이 없다. |
| `invalid_request`, `request_rejected` | 요청 형식이 잘못됐거나 분류되지 않은 요청 거절이다. |
| `forbidden` | 별도 원인으로 분류되지 않은 접근 거절이다. |
| `not_found`, `method_not_allowed`, `method_not_supported` | 경로가 없거나 해당 메서드를 처리하지 않는다. |
| `length_required`, `request_too_long`, `headers_too_large`, `http_version_not_supported` | 요청 길이·머리글·HTTP 버전을 처리하지 못한다. |
| `gateway_unavailable`, `internal_error`, `vm_unreachable` | 입구 설정·내부 처리 또는 VM 연결을 확인하지 못했다. |

## GitHub 연결과 백업

출처: [데스크톱 S110](desktop-github-momso-s110.md), [서버 S48](server-update-github-s48.md), [S35](server-github-backup-s35.md), `apps/alpha/server/src/github_bridge.rs`.

| 화면 | 호출·값·사람의 단계 |
| --- | --- |
| 데스크톱 연결 | `provider_login_start({provider:'github'})`, `provider_login_cancel({provider:'github'})`; 이벤트 `provider-login-device`, `provider-login-device-closed`, `provider-auth-changed`. `expiresInSeconds`는 GitHub가 준 시간이며 `ghConfigSource`는 `owned`. 저장소는 기존 `archive_remote_*`를 쓴다. |
| 휴대폰 연결 | `POST /github/login` → `{sessionId}`; `GET /github/login`과 `POST /github/login/cancel`에는 `X-Momso-Login-Session`을 붙인다. 조회값은 `{provider:'github',state,url,code,expiresInSeconds}`. 이벤트 SSE가 아니라 조회다. |
| 저장소·백업 | `GET /github/repositories`, `GET`·`POST /github/repository`, `POST /github/sync`, `POST /github/restore`. 선택 조회에는 `repository`, `backupStatus`, `backupReason`, `githubBackup`, `objectBackup`이 있다. 동기화 요청은 `{state:'queued'}`. |

사람은 `https://github.com/login/device`에서 본인 로그인과 화면에 나온 기기 코드 입력·승인을 한다. 접근·갱신 토큰과 기기 비밀 코드는 화면·채팅·기록에 내보내지 않는다. GitHub의 `repo` 권한 범위는 특정 저장소 하나보다 넓다. 갱신 응답 유실·보관 실패로 기존 갱신 토큰이 이미 무효면 재연결이 필요할 수 있다. 실제 Tauri 승인 완료와 휴대폰 전체 경로는 각 원문의 시험 경계를 따른다.

휴대폰 기기 로그인 `state`는 `pending`(승인 대기), `connected`(연결), `expired`(기한 만료), `cancelled`(취소), `failed`(승인 실패)다. GitHub의 `authorization_pending`과 `slow_down`은 서버의 내부 조회 응답으로, 후자는 조회 간격을 늘린다. 이 둘을 화면 반환 상태로 새로 만들지 않는다. 구현: `apps/alpha/desktop/src-tauri/momso-github-backup/src/github.rs`.

서버 백업 상태는 `not_configured`(연결되지 않음), `pending`(남은 변경), `syncing`(동기화 중), `saved`(저장됨), `saved_partial`(일부만 확인됨), `failed`(실패)다. 선택·수동 동기화 요청의 `queued`는 대기 요청이지 저장 완료가 아니다. 구현: `apps/alpha/server/src/github_worker.rs`, `github_bridge.rs`.

| 값 | 뜻 |
| --- | --- |
| `github_client_id_missing` | 서버 GitHub 로그인 앱 ID를 사용할 수 없다. |
| `credential_unavailable` | 자격 증명 보관을 사용할 수 없다. |
| `login_not_found` | 해당 로그인 세션을 찾지 못했다. |
| `repository_not_selected` | 백업할 저장소가 선택되지 않았다. |
| `repository_not_found` | 요청한 저장소를 확인하지 못했다. |
| `backup_unavailable` | 백업 부품을 사용할 수 없다. |
| `storage_unavailable` | 저장소 자료에 접근하지 못했다. |
| `changes_after_backup` | 백업한 뒤 추가 변경이 있다. |
| `object_backup_not_integrated` | GitHub 저장과 별도 원자료 백업 상태가 맞지 않는다. |
| `credential_in_archive` | 현재 아는 비밀값이 올릴 파일에서 발견돼 전송을 막았다. |
| `credential_in_history` | 아직 올리지 않은 Git 이력에서 현재 아는 비밀값이 발견됐다. |
| `chat_redaction_required` | 채팅 사본에서 비밀값을 가려야 해 전송을 막았다. |
| `uploadedKeyFound: true` | 이미 올라간 이력에서 현재 아는 비밀값을 발견했다. 모든 과거 비밀을 찾는 보장은 아니다. |
| `unavailable` | GitHub 연결 또는 작업을 사용할 수 없다. |
| `provider_unavailable` | GitHub 로그인 제공자를 사용할 수 없다. |
| `provider_invalid_response` | GitHub 응답 형식을 확인하지 못했다. |
| `session_not_found` | 지정한 기기 로그인 세션을 찾지 못했다. |
| `credential_store_unavailable` | 연결 정보를 자격 증명 보관소에 저장하거나 읽지 못했다. |
| `not_connected` | GitHub 연결 자격 증명을 사용할 수 없다. |
| `invalid_repository` | 저장소 식별자가 허용 형식이 아니다. |
| `repository_not_private` | 선택한 저장소가 비공개가 아니다. |
| `invalid_username` | 저장소 생성에 쓸 사용자 이름 형식이 맞지 않는다. |
| `repository_create_failed` | 저장소 생성 요청이 실패했다. |
| `clock_unavailable` | 로그인에 필요한 시간을 확인하지 못했다. |
| `storage_error` | 서버 저장 작업이 실패했다. |
| `maintenance_required` | 복원에 필요한 유지보수 상태가 아니다. |
| `work_in_progress` | 복원과 동시에 진행 중인 작업이 있다. |
| `invalid_snapshot` | 복원할 채팅 사본 형식이 올바르지 않다. |
| `unsupported_snapshot` | 지원하지 않는 채팅 사본 형식이다. |
| `invalid_document` | 복원할 문서 형식을 확인하지 못했다. |
| `restore_conflict` | 기존 자료와 복원 내용이 충돌한다. |
| `generation_restore_conflict` | 생성물 복원 내용이 기존 자료와 충돌한다. |
| `restore_changed` | 복원 확인 중 대상이 변경됐다. |
| `attachments_restore_unavailable` | 첨부자료 복원을 확인하지 못했다. |
| `not_found` | 요청된 복원 대상이 없다. |

## 설정: 쉬기·전부 지우기

출처: [S120·S127·S120b·S120c](momso-anywhere-deletion-s120.md), [S117c](momso-anywhere-rehearsal-s117c.md), `scripts/momso_anywhere_start.mjs`, `momso_server_install.py`.

| 작업 | 호출·결과 |
| --- | --- |
| 미리 보기 | 휴대폰 `deletionPreviewFromGesture({clientId, release, projectId, installationId, ...})`; `resumeFromRedirect` 복귀값은 `action:'deletion_preview'`, `value`이고 화면용 `resumeAfterRedirect`는 `kind:'deletion_preview'`, `result`다. PC는 `python scripts/momso_server_install.py delete-preview --account <계정> --project <프로젝트> --state <설치기록>`. |
| 쉬기 | PC `python scripts/momso_server_install.py destroy --account <계정> --project <프로젝트> --state <설치기록>`은 VM 문을 닫은 뒤 연결된 백업을 다시 확인하고 삭제가 확인될 때까지 문을 잡는다. 휴대폰은 아래 표 전달 후 `destroyFromGesture({clientId, release, projectId, installationId, discardData:false, ...})`를 부른다. |
| 전부 지우기 | PC는 위 명령에 `--discard-data`, 휴대폰은 `destroyFromGesture({clientId, release, projectId, installationId, discardData:true, ...})`. 되돌릴 수 없는 선택은 사람이 명시한다. 결과 `mode:'delete_all'`, `resources`, `billable`, `billingZero`, `estimatedZeroAt`, `backupBillingZero`, `backupChargesEndAt`; 휴대폰은 `remaining`, `status:'pending'|'complete'`도 준다. |
| 프로젝트 판별 | PC `python scripts/momso_server_install.py project-exclusivity --account <계정> --project <프로젝트> --state <설치기록>`; 휴대폰 미리 보기의 `projectExclusivity`, 또는 공급자 `projectExclusivity(id)`. `status:'not_exclusive'|'unverified'`, `observedTypes`, `enabledServices`, `installerServices`, `unattributedServices`, `checks`, `foreignEvidence`, `limitations`. 현재 완전한 전용 증거가 없어 `exclusive`를 반환하지 않는다. 프로젝트 종료 호출은 없다. |

PC 삭제 JSON과 휴대폰 삭제 결과는 공통으로 `outcome`, `unverified`, `resources`, `remaining`, `billable`, `billingZero`, `estimatedZeroAt`, `reason`, `retryable`, `complete`, `mode`, `status`를 반환한다. `retryable:null`과 `billingZero:null`은 미확인이다. 기존 `retained`, `backupBillingZero`, `backupChargesEndAt` 등은 있는 경우 그대로 유지한다. 휴대폰의 삭제 실패는 예외 대신 `outcome:'failed'`와 `reason`으로 반환하며, Google 인증 이동은 기존 `{redirecting:true}`다.

| `outcome` | 뜻 |
| --- | --- |
| `deleted` | 확인한 소유 자원이 없거나 끝나는 시각이 확인된 보존분만 남았다. |
| `deleted_unverified` | 확인된 잔존은 없으나 `unverified`에 나열된 종류는 확인하지 못했다. |
| `remaining` | 끝나는 시각이 없는 잔존 자원이 확인됐다. `resources`, `remaining`, `retryable`을 확인한다. |
| `failed` | 삭제 자체가 실패했다. `reason`을 확인한다. |

S117a·c는 `deleted`와 `deleted_unverified`를 삭제 단계 통과로 다루고 미확인 종류를 출력한다. `remaining`·`failed`는 통과가 아니다. 다른 설치·검증 단계 실패는 별도다.

기존 시작 화면의 `resumeAfterRedirect` 호환 경로는 아직 `remaining`만 읽으므로 `failed`는 기존처럼 오류를 발생시키고 `deleted_unverified`의 미확인 종류는 표시용 `remaining`에 `unverified:<종류>`로 더한다. 원래 공통 객체의 `outcome`과 `unverified`는 유지한다. 화면 담당은 네 `outcome`을 직접 읽도록 연결하고 이 임시 호환 표기를 제거해야 한다. `destroyFromGesture`와 `resumeFromRedirect`는 공통 객체를 그대로 반환한다.

휴대폰 앱은 소유자 세션으로 `POST /__momso/pause/prepare`를 부른다. 성공 시 받은 `ticket`, `expiresIn`과 입구 출처·프로젝트 ID·설치 ID, 발급 직후 밀리초 시각을 시작 페이지 조각에 `pause_ticket`, `gateway`, `project_id`, `installation_id`, `created_at`, `expires_at`으로 전달한다(`expires_at = created_at + expiresIn * 1000`). 시작 페이지의 `resumeAfterRedirect({clientId, release, onProgress, ...})`는 조각을 소비해 `{kind:'pause_ready', selection:{projectId, installationId}}`를 반환한다. 화면이 `destroyFromGesture`를 부르면 인증 필요 시 `{redirecting:true}`, 복귀 후 `resumeAfterRedirect`는 `{kind:'destroy', result}`를 반환한다. `cancelPauseFromGesture({session, fetchImpl})`는 준비 상태에서 해제 성공 시 `{ok:true}`, 삭제 준비가 진행 중이면 `{ok:true,pending:true}`를 반환하고 원래 삭제 호출은 해제 후 `pause_cancelled`로 끝난다. VM 삭제 요청을 이미 보내 결과를 모르면 `vm_deletion_unconfirmed`이며 문을 열지 않는다. 표가 없으면 `pause_requires_app_session`, 표가 만료·재사용됐으면 `invalid_pause_ticket`, 확인 불능은 `vm_unreachable` 또는 `maintenance_release_unconfirmed`이다. 인증 이동 중 표는 이 탭의 `sessionStorage` 한 키에만 잠시 두며 복귀 시 삭제한다. 입구 코드 적용에는 새 서명된 입구 이미지가 필요하다.

소유자 세션의 같은 출처 호출은 `GET /__momso/pause/status` → `{state}`, `POST /__momso/pause/owner-release`와 `POST /__momso/pause/reissue`도 제공한다. 준비 표의 기본 유효 기간은 300초다. 준비 상태에서 만료되면 문을 열지만 확정된 상태는 만료로 열지 않는다. 확인된 VM 삭제 실패에는 해제하고, 삭제 요청 결과가 불명확하면 열지 않는다. 확인·해제 표 호출은 설정된 로그인 출처만 CORS로 허용하며 쿠키 없이 표로 인증한다. 실제 휴대폰·클라우드 삭제 확인은 미확인이다. [S120](momso-anywhere-deletion-s120.md)

`backupExists:true|false|null`, `lastBackupAt`, `lastBackupValidity`는 미리 보기이며 과거 시각만으로 마지막 변경의 백업을 보장하지 않는다. `estimatedMonthly`의 `kms`, `storage`, `subtotal`, `total`은 확인 범위별 부분 추정이다. `null`은 0이 아니라 미확인이다. `billable[].resource`, `billable[].endsAt`은 확인된 과금 가능 자원과 기한이며 `null` 기한은 모른다. `backupBillingZero`·`backupChargesEndAt`은 버킷·KMS만의 범위이고 `billingZero:null`은 전체 요금 미확인이다. `status:'complete'`도 전체 프로젝트 요금이 0이라는 뜻이 아니다. 같은 프로젝트·설치 ID로 재시도한다. S117c 삭제 단계는 `deleted_unverified`도 통과로 기록하되 미확인 종류를 별도로 출력한다. `remaining`은 통과가 아니다.

| 값 | 뜻 |
| --- | --- |
| `deleted` | 해당 자원 삭제를 확인했다. |
| `retained` | 해당 자원이 남아 있다. `reason`을 함께 본다. |
| `unverified` | 존재·삭제·비용을 확인하지 못했다. |
| `pending` | 남은 삭제·보존·확인을 같은 설치 ID로 재시도해야 한다. |
| `complete` | 이 경로의 확인된 삭제 절차가 끝났다. 전체 비용 0의 증거는 아니다. |
| `backup_not_connected` | 최신성 확인 대상 백업이 연결되지 않았다. |
| `backup_freshness_unverified` | 마지막 변경까지 백업됐는지 확인하지 못했다. |
| `maintenance_release_unconfirmed` | VM 유지보수 문을 다시 열었는지 확인하지 못했다. |
| `vm_unreachable` | VM에 닿지 못했다. |
| `pause_requires_app_session` | 앱의 소유자 세션에서 발급한 표가 없다. |
| `invalid_pause_ticket` | 표가 잘못됐거나 만료·재사용됐다. |
| `vm_unconfirmed` | VM 삭제 뒤에도 VM이 조회된다. |
| `vm_deletion_unconfirmed` | PC에서 VM 삭제 뒤에도 해당 VM이 조회된다. |
| `ownership_unconfirmed` | 자원 이름·설치 ID 라벨 등 소유권이 확인되지 않았다. |
| `resource_ownership` | 휴대폰 자원 소유권·구성이 맞지 않아 중단했다. |
| `soft_delete_retention` | 이미 소프트 삭제된 자료가 보존 기간 동안 남는다. |
| `destruction_scheduled` | KMS 키 버전의 파기가 예약돼 있으며 시각까지 과금될 수 있다. |
| `keyring_not_deleted` | 키링을 아직 삭제하지 않았다. |
| `bucket_still_exists` | 버킷이 아직 조회된다. |
| `missing_or_soft_deleted` | 버킷이 보이지 않으나 보존본 여부는 확인하지 못했다. |
| `key_missing_or_unavailable` | 키가 없거나 접근할 수 없어 키 상태를 확정하지 못했다. |
| `cleanup_unverified` | 정리 작업 또는 결과를 확인하지 못했다. |
| `delete_not_verified` | 삭제 요청 뒤 부재를 확인하지 못했다. |
| `not_verified` | 해당 상태를 아직 확인하지 않았다. |
| `not_inspected` | 해당 자원을 조회하지 않았다. |
| `not_inventoried` | 해당 종류의 잔여 자원을 조사하지 않았다. |
| `cleanup_unavailable` | 백업 정리 실행기 또는 인증을 사용할 수 없다. |
| `not_found_or_unavailable` | 자원이 없거나 조회할 수 없다. |
| `backup_recovery` | 백업 복원을 위해 보관소 열쇠를 남긴다. |
| `invalid_scope` | 프로젝트·설치 ID 또는 조회 입력 형식이 맞지 않는다. |
| `not_exclusive` | 다른 설치 표시 또는 다른 VM·버킷을 찾았다. |
| `observed` | 해당 목록을 읽었다. 모든 Google 자원을 보았다는 뜻은 아니다. |
| `partial` | 일부 결과만 유효하게 읽었다. |
| `different_installation` | 다른 설치 ID 라벨을 발견했다. |
| `other_named_resource` | 이 설치 이름과 다른 VM 또는 버킷을 발견했다. |
| `unsupported_asset_types` | Google 자원 검색이 지원하지 않는 종류가 있다. |
| `asset_inventory_delay` | 자원 검색에 반영 지연이 있을 수 있다. |
| `service_enablement_provenance_unknown` | 활성 API를 누가 켰는지 알 수 없다. |
| `iam_and_external_billing_unverified` | IAM과 외부 청구를 확인하지 않았다. |

PC에서 접속 실패와 문 열림 미확인이 함께 있으면 `vm_unreachable: maintenance_release_unconfirmed`가 나올 수 있다. 문이 열렸다고 가정하지 않는다.

## 업데이트 상태

출처: [서버 S38](server-update-s38.md), [S90](server-release-discovery-s90.md), [입구 S115](momso-anywhere-revision-s115.md), [데스크톱 S69](desktop-update-s69.md).

휴대폰은 인증된 `GET /update/status`로 `{state,reason}`을 읽는다. 입구 전환 실패에는 공통 `{code,rolledBack,retryable}`도 붙는다. PC 설치기는 같은 객체를 표준 출력의 JSON 한 줄로 내고 사람용 오류는 표준 오류에 남긴다. 휴대폰 시작은 기존 `SetupError.step`과 함께 이 세 필드를 오류 및 단계 진행 이벤트에 제공한다. `rolledBack:true`는 이전 입구 트래픽 복구를 확인했다는 뜻이고 `false`는 복구 실패, `null`은 확인하지 못한 것이다. `retryable:null`은 재시도 가능 여부 미확인이다. 자동 업데이트 상태는 `checking`, `up_to_date`, `downloading`, `waiting_for_idle`, `backing_up`, `installing`, `updating_gateway`, `completed`, `rolling_back`, `rolled_back`, `recovery_required`, `failed`다. `reason`은 안전한 오류 이름 또는 `null`이며 서버 재시작 동안 입구가 마지막 상태를 제공하도록 설계됐다. 새 입구는 지문과 트래픽을 확인하고 `GET /__momso/ready`의 200·`{"ready":true}`를 검사한다. 이 확인은 VM·채팅의 정상 동작까지 증명하지 않는다.

S129e: PC 갱신과 VM 자동 갱신은 입구 전환 실패 시 이전 실행 파일·화면·자료 스냅샷 복구를 시도한다. 자료 복원이 실패하면 실행 파일과 자료가 실패 전 새 판 그대로인지 확인된 경우에만 서버 재시작을 시도한다. 그 확인이나 재시작이 실패하면 서버가 켜졌다고 가정하지 않는다. 복구 실패는 `rollback_failed` 등 상태로 확인한다. 이 복구 경로는 가짜 시험 범위다. [S59](server-upgrade-data-s59.md)

### 운영 서명키 교체

운영 시작 설정의 `release.trustBootstrap`에는 순번 0의 공개키 셋과 정족수 2, 최초 릴리스 키가 들어간다. 평소 릴리스는 현재 주 키 하나로 서명하고, 신뢰 교체에는 기존·새 키 집합 각각 3개 중 2개의 서명이 필요하다. `trust-root-<순번>.json`과 `trust-root-<순번>.sigs.json`을 순서대로 확인한 뒤 현재 주 키로 릴리스를 확인한다. 교체의 서명된 `issuedAt`은 이전 root의 유효 기간 안이어야 하고 앞선 교체보다 이르거나 미래면 안 된다. 중간 root는 현재 시각으로 만료를 다시 보지 않고, 맨 끝 root는 현재 시각으로 만료를 확인한다. PC 설치기·VM 작업자는 신뢰 순번을 파일에 보존하고, 브라우저는 저장된 교체 이력을 초기 root부터 재검증한다. `issuedAt`은 서명자가 주장한 시각이지 외부에서 증명한 서명 시각이 아니다. 브라우저 저장소 삭제·새 기기에서 오래된 root가 제시되는 경우와 최신 목록을 숨기는 공격까지 방지한다고 보장하지 않는다. 개발 채널은 단일 개발 키를 쓴다. 실제 운영 키 발급·교체·휴대폰 검증은 미확인이다. [S142](momso-anywhere-key-rotation-s142.md)

| 값 | 뜻 |
| --- | --- |
| `release_discovery_unavailable` | 공개 릴리스 목록 조회에 실패했다. |
| `release_list_invalid` | 릴리스 목록 형식이 올바르지 않다. |
| `release_list_too_large` | 목록이 정해진 페이지 한도를 넘었다. |
| `gateway_image_permission` | 입구 이미지에 접근할 권한을 확인하지 못했다. |
| `gateway_operation_failed` | 입구의 클라우드 작업이 실패했다. |
| `gateway_image_mismatch` | 새 판 또는 트래픽 판의 이미지 지문이 다르다. |
| `gateway_traffic_mismatch` | 트래픽이 의도한 판과 다르다. |
| `gateway_not_ready` | 입구 판이 준비되지 않았다. |
| `gateway_probe_failed` | 입구의 공개 준비 확인이 실패했다. |
| `gateway_rollback_failed` | 이전 입구 트래픽의 복구를 확인하지 못했다. |
| `gateway_image_invalid` | 입구 이미지와 지문 형식이 맞지 않는다. |
| `gateway_response_invalid` | 입구 클라우드 응답을 확인하지 못했다. |
| `gateway_address_invalid` | 입구 주소를 확인하지 못했거나 기존 주소가 달라졌다. |
| `gateway_ownership` | 입구의 소유권이 맞지 않는다. |
| `gateway_policy` | 입구 접근 정책이 허용된 범위와 다르다. |
| `gateway_unhealthy` | 입구 건강 확인이 실패했다. |
| `server_unhealthy` | 서버 건강 확인이 실패했다. |
| `rollback_failed` | 이전 상태로 되돌리는 데 실패했다. |
| `maintenance_release_failed` | 유지보수 문을 다시 열지 못했다. |
| `managed_cleanup_pending` | 관리 파일 정리가 남았다. |
| `recovery_record_invalid` | 복구 기록을 확인하지 못했다. |
| `recovery_lease_unavailable` | 복구 잠금을 얻지 못했다. |
| `server_start_failed` | 복구 중 서버를 시작하지 못했다. |
| `update_failed_rolled_back` | 갱신 실패 뒤 이전 상태로 되돌렸다. |
| `update_failed` | 세부 실패를 안전하게 분류하지 못한 갱신 오류다. |
| `server_stop_failed` | 복구 중 서버 중지에 실패했다. |
| `data_restore_failed` | 이전 자료 복구에 실패했다. |
| `helper_restore_failed` | 관리 보조 파일 복구에 실패했다. |
| `release_restore_failed` | 이전 실행 판 복구에 실패했다. |
| `sequence_restore_failed` | 이전 설치 순번 복구에 실패했다. |
| `proxy_restore_failed` | 사설 프록시 복구에 실패했다. |
| `server_health_failed` | 복구한 서버의 건강 확인을 수행하지 못했다. |
| `gateway_restore_failed` | 이전 입구 복구에 실패했다. |
| `gateway_health_failed` | 복구한 입구의 건강 확인을 수행하지 못했다. |
| `signature_invalid` | 서명된 목록을 검증하지 못했다. |
| `root_invalid` | 운영 신뢰 root의 형식·공개키를 확인하지 못했다. |
| `root_version_invalid` | 교체 순번이 현재 순번의 다음이 아니거나 자산 이름과 다르다. |
| `root_expired` | 최종적으로 사용할 현재 신뢰 root의 유효 기간이 지났다. |
| `root_time_invalid` | 교체 발행 시각이 앞선 root의 유효 기간 밖이거나 역행·미래다. |
| `signature_threshold_invalid` | 이전·새 신뢰 집합이 각각 요구하는 교체 서명 수를 채우지 못했다. |
| `root_assets_missing` | 필요한 교체 순번 또는 교체 파일 쌍이 빠졌다. |
| `root_assets_ambiguous` | 같은 교체 자산의 출처를 하나로 확인하지 못했다. |
| `root_assets_invalid` | 교체 서명 파일 등의 형식이 맞지 않는다. |
| `root_chain_too_long` | PC 설치기에서 교체 이력이 허용 한도를 넘었다. |
| `clock_invalid` | 신뢰 root 확인에 필요한 시간대가 없는 시각이다. |
| `manifest_invalid` | 배포 목록의 채널·순번·지문·주소 등 형식이 맞지 않는다. |
| `key_invalid` | 고정 공개키 형식을 확인하지 못했다. |
| `openssl_unavailable` | 서명 검증에 필요한 OpenSSL을 사용할 수 없다. |
| `channel_invalid` | 배포 채널이 허용된 값이 아니다. |
| `invalid_url` | 배포 주소가 허용 조건에 맞지 않는다. |
| `download_too_large` | 내려받기 크기가 한도를 넘었다. |
| `digest_mismatch` | 묶음의 SHA-256이 목록과 다르다. |
| `invalid_bundle` | 묶음 형식·내용을 검증하지 못했다. |
| `update_busy` | 다른 업데이트가 잠금을 사용 중이다. |
| `busy` | 작업이나 복구가 진행 중이다. |
| `maintenance_unavailable` | 유지보수 문을 잡지 못했다. |
| `current_state_unavailable` | 현재 판·자료 상태를 확인하지 못했다. |
| `installed_release_mismatch` | 설치된 파일과 기준 판이 일치하지 않는다. |
| `recovery_required` | 이어가기 전에 복구가 필요한 상태다. |
| `update_unavailable` | 업데이트 작업을 사용할 수 없다. |

데스크톱은 `app_version()`, `update_check()`, `update_install()`, `update_restart()`를 쓴다. 확인 상태 `available`, `up_to_date`, `no_release`, `failed`, `unavailable`; 설치·재시작에는 `downloaded`, `busy`, `install_started`가 있다. 이벤트는 `update-available`, `update-progress {downloadedBytes,totalBytes?}`, `update-done`, `update-failed {state,reason?}`다. 진행 중 턴이면 `busy`·`active_turn`이고 설치하지 않는다. 설치 시작 시 앱이 종료되므로 이벤트 도착을 보장하지 않는다.

| 데스크톱 `reason` | 뜻 |
| --- | --- |
| `timeout` | 요청 제한 시간이 지났다. |
| `connection_failed` | 연결에 실패했다. |
| `network_failed` | 그 밖의 네트워크 요청 오류다. |
| `download_http_error` | 내려받기의 HTTP 오류다. |
| `release_unavailable` | 릴리스를 찾지 못했다. |
| `invalid_release_info` | 릴리스 정보 또는 버전 형식이 잘못됐다. |
| `platform_unavailable` | 현재 OS·구조에 맞는 릴리스가 없다. |
| `signature_invalid` | 서명 또는 서명된 버전을 확인하지 못했다. |
| `configuration_invalid` | 업데이트 주소·전송 설정이 올바르지 않다. |
| `configuration_unavailable` | 업데이트 설정을 사용할 수 없다. |
| `update_not_selected` | 업데이트할 판이 선택되지 않았다. |
| `update_not_downloaded` | 서명 확인된 파일이 준비되지 않았다. |
| `active_turn` | 진행 중인 채팅 턴이 있다. |
| `backup_failed` | 설치 전 데이터 백업에 실패했다. |
| `installation_failed` | 설치 또는 복구 프로세스 준비에 실패했다. |
| `worker_failed` | 비동기 작업이 실패했다. |
| `unknown_failure` | 다른 분류에 속하지 않는 오류다. |

## 그 밖의 서버 화면 계약

S136 대조 기준: [현재 server 라우터](../../apps/alpha/server/src/main.rs), [입력 요청](../../apps/alpha/server/src/human_prompt.rs), [저장 로그인](../../apps/alpha/server/src/browser_bridge.rs), [AI 연결](../../apps/alpha/server/src/provider_login.rs). 오류 본문은 `{error:<코드>}`다. 아래 과거 문서의 “미연결”은 당시 상태와 현재를 구분해 읽는다.

| 화면 계약 | 현재 응답·주의 | 미연결 또는 한계 |
|---|---|---|
| 승인 | `GET /turns/{id}/requests` → `{approval,inputs}`; `POST /approvals/{id}/respond` `{choice}` → `{requestId,state:"answered"}`. `invalid_choice`, `approval_closed` | `provider`는 해당 턴의 공급자이며 저장된 값이 없으면 `codex`로 대체한다 |
| 민감 입력 | `POST /input-requests/{id}/respond` → `{requestId,state:"answered"}` 또는 API 키 `{requestId,state:"credential_saved"}`; `/cancel` → `{requestId,state:"cancelled"}`. `invalid_input`, `request_closed` | 캡차·패스키를 휴대폰에서 직접 처리하는 화면은 미확인 |
| 저장 로그인 | `GET /saved-logins` → `{logins}`, `POST /saved-logins`와 `PUT /saved-logins/{id}` → `{login}`, `DELETE /saved-logins/{id}` → `{state:"deleted"}`. 비밀번호 미반환; 오류는 `invalid_input`, `login_not_found` 및 보관소 고정 코드 | `iconDataUrl`은 서버 응답에 없음 |
| AI 키 | `POST /providers/{provider}/key/request` → 입력 객체; `GET` → `{inputs}`. `provider`는 `claude`·`gemini`, 그 외 `invalid_provider`; `{apiKey}` 응답은 `credential_saved`, 잘못된 형식은 `invalid_key` | 저장이 키 검증 성공을 뜻하지 않음. S138에서 Claude·Gemini 브라우저·CRM 도구 루프를 서버에 연결. 화면은 기존 승인·사람 입력 이벤트를 사용하고 승인 객체의 `provider`는 턴의 공급자. 실제 계정 검증은 별도 |
| Codex 연결 | `POST /providers/codex/login` → `{sessionId}`. `GET /providers/codex/login` 및 `/events`, `POST /providers/codex/login/cancel`에는 `x-momso-login-session`이 필요하며 틀리면 `login_not_found`. 조회는 `{provider,state,url,code,expiresInSeconds,expiresAt}`, 취소는 `{provider,state:"cancelling"}` | 기기 코드와 로그인 주소는 실제 CLI가 제공할 때만 나타남. 실제 사용자 로그인은 별도 확인 |

검증: `provider_key_request_reconnect_and_cancel_contract`, `provider_keys_and_chat_use_encrypted_vault_and_fake_api`, `phone_saved_login_is_encrypted_and_never_echoed`, `approval_survives_reconnect_and_returns_to_fake_cli`는 가짜 입력·CLI·사이트 범위다. 실제 계정이나 휴대폰의 수락 시험은 아니다.

- 승인·민감 입력: `POST /chats/{chatId}/turns`의 `permissionMode`는 `Ask for approval`, `Approve for me`, `Full access`. `agent-approval-request {requestId,chatId,turnId,provider,kind,description,options}`, `GET /turns/{turnId}/requests`, `POST /approvals/{requestId}/respond {choice}`, `POST /input-requests/{requestId}/respond`, `/cancel`을 쓴다. 캡차·패스키 등 `browser-human-required`는 사람에게 넘기며 휴대폰 직접 수행 화면은 원문에서 미완료다. [S23](momso-server-approval-s23.md)
- 저장된 로그인: `POST /saved-logins` 입력에는 비밀번호가 있을 수 있으나 반환값에 비밀번호는 없다. 인증번호·비밀번호를 AI·채팅·기록에 내보내지 않는다. 사이트 로그인 성공과 저장 성공은 다르다. [S23](momso-server-approval-s23.md), [S21](server-saved-logins-s21.md)
- 유휴 종료·깨우기: 서버가 작업·백업을 마친 뒤 멈추고 입구가 깨운다. 임시 `maintenance_in_progress`에 대한 재시도가 필요하지만 끊긴 요청의 정확히 한 번 처리는 보장되지 않는다. [S50](server-idle-stop-s50.md), [S26](momso-anywhere-onboarding-s26.md)
- 문서·스킬·용어 사전: [S29](server-momso-core-s29.md)는 독립 공통 자료 부품의 당시 Rust 계약이며 그 문서만으로 서버 HTTP나 화면 연결을 증명하지 않는다. 현재 서버 계약과 화면 스냅샷을 따로 대조한다.
- AI 연결: `/providers`, `/providers/codex/login`, `/providers/codex/login/events`, `/providers/codex/login/cancel`, `/providers/{provider}/key/request` 등 실제 라우터 기준이다. Claude·Gemini API 키 입력의 제안 계약과 현재 구현 상태는 구분해야 한다. [S36](server-providers-s36-contract.md), `apps/alpha/server/src/main.rs`.

## 뒤쪽에서 먼저 맞춰야 할 것

- 프로젝트 전용 판정은 `not_exclusive` 또는 `unverified`뿐이다. Asset Inventory가 모든 자원을 다루지 않으므로 `exclusive`·자동 종료를 약속할 수 없다. 프로젝트 종료 후 30일 복원 기간에도 일부 자원은 일찍 사라질 수 있고 청구 주기 말까지 비용이 생길 수 있다. [S120](momso-anywhere-deletion-s120.md)
- 저장 로그인 응답에는 `iconDataUrl`이 없다. 휴대폰에서 캡차·패스키를 직접 수행하는 화면은 미확인이다. [S21](server-saved-logins-s21.md), [S23](momso-server-approval-s23.md)
- 정적 호스팅 이전은 준비 상태다. 운영 도메인 연결, 운영 공개키·root 게시, OAuth 변경, 실제 게시와 기기 확인은 남아 있다. [S114](momso-anywhere-static-host-s114.md), [S142](momso-anywhere-key-rotation-s142.md)

## 화면에서 정할 것

- 시작 화면: Google에서 설정을 마친 뒤 돌아온 상태와 재확인 결과를 어떻게 보여 줄지.
- 로그인·홈 화면: 로그아웃 단추를 둘지와 어디에 둘지.
- 설정의 삭제: 정해진 두 선택과 별도 확인 단계를 화면에 어떻게 배치할지.
- 설정의 삭제 미리 보기: 마지막 백업과 미확인 항목을 어떻게 보여 줄지.
- 설정의 삭제 결과: 남은 자원·예상 비용·종료 시각과 미확인 항목을 어떻게 구분해 보여 줄지.
- 설정의 프로젝트 판별: 전용이 아님과 판단 못 함을 어떻게 구분해 보여 줄지.
- 업데이트 상태: 진행·이전 판 복구·복구 미확인을 어떻게 보여 줄지.
- 홈 화면: VM을 깨우는 동안 기다림과 재시도를 어떻게 보여 줄지.
- 승인·민감 입력 화면: 공급자별 승인과 사람에게 넘긴 요청을 어떻게 보여 줄지.

## 충실도 확인

- S150 대조: 시작·Google 복귀는 `momso_anywhere_start.mjs`·`momso_anywhere_google_browser.mjs`, 입구와 깨우기는 `momso_anywhere_gateway.py`, 쉬기·표는 `momso_anywhere_pause_ticket.py`·`momso_anywhere_start.mjs`, 삭제 outcome은 `momso_anywhere_delete_result.mjs`·S117a·c, 입구 전환과 복구는 작업자의 `cloud_run_gateway.py`·`update.py`와 S59를 대조했다.
- 운영 키 셋·순번·정족수와 브라우저 저장 한계는 `trust_rotation.py`·`momso_release_trust.mjs`·시작 묶음 생성기와 S140·S142를 대조했다. 승인 공급자와 서버 응답은 `apps/alpha/server/src/main.rs`·S136·S138을 대조했다. 단추 둘·표시 이름·Windows 인증서 미구매는 본부가 전달한 결정과 대조했다.
- **고친 의미 차이:** 옛 표의 승인 `provider` 고정 설명은 현재 코드의 턴별 공급자와 달랐다. S117c가 미확인을 통과시키지 않는다는 문장도 `deleted_unverified`를 통과로 기록하는 현재 코드와 달랐다. 둘 다 위 본문에서 바로잡았다. 그 밖에 대조한 결정·조건·수치·까닭 코드에서 의도적으로 의미를 바꾼 곳은 없다. 이 차이는 코드 결함이라는 뜻은 아니다.
- S151: 중간 root 만료를 현재 시각으로 다시 보던 브라우저·VM 경로 차이를 서명된 발행 시각 기준으로 맞췄다. 공통 가짜 벡터 시험이며 실제 운영 키 교체는 미확인이다.
- 실제 확인은 본부 기록의 순번 6 자동 갱신, 입구 트래픽 전환, 유휴 종료, 페이지 열기에 의한 깨우기만 여기에 반영한다. fetch 깨우기, 휴대폰 삭제·운영 키 교체·정적 호스팅, 실제 새 Google 계정과 그 밖의 가짜 시험은 실제 확인으로 적지 않았다. 실제 화면·휴대폰에서 계약이 모두 연결됐는지는 **미확인**이다.
