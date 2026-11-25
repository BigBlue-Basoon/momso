정리본: [momso-anywhere-guide.md](momso-anywhere-guide.md)

# S64 휴대폰 시작 준비

## 위치와 묶음

시작 화면은 공통 로그인과 **같은 HTTPS 출처의 `/start/`**, 로그인은 `/`에 둔다. 출처를 하나만 OAuth에 등록하고 정적 파일을 따로 관리할 수 있다. 소개 사이트는 별개이며 링크만 둔다. 운영 출처는 `https://anywhere.momso.yoga`, 첫 시험은 본부가 마련한 임시 `run.app` 출처를 쓴다. `/start`는 `/start/`로 보내야 상대 경로 설정과 모듈을 찾는다.

화면 담당 스냅샷의 워크스페이스를 별도 폴더에 꺼내 최상위에서 `npm ci`, `apps/alpha/desktop`에서 `VITE_MOMSO_START=1`로 `npm run build -- --base=/start/`를 실행한다. PowerShell에서는 먼저 `$env:VITE_MOMSO_START='1'`을 설정한다. 원본 화면 폴더를 수정하지 않는다. 그 뒤:

```powershell
python scripts/prepare_momso_anywhere_start_site.py --dist <화면의-dist> --output <새-출력-폴더> --client-id <공개-웹-클라이언트-ID> --release-manifest-url <서명된-배포-목록-HTTPS-주소> --release-public-key <Ed25519-공개키-PEM> --login-origin <공통-로그인-HTTPS-출처>
```

출력 폴더의 내용을 정적 사이트의 `/start/`에 올린다. 화면 파일과 `momso_anywhere_start.mjs`, `momso_anywhere_google_browser.mjs`, `momso_anywhere_cloud_run_revision.mjs`, `momso_release_manifest.mjs`, `momso_anywhere_setup.mjs`, `momso_anywhere_vm_startup.mjs`, `momso_anywhere_oauth_capture.mjs`, `momso_anywhere_oauth_redirect.mjs`, `config.json`이 함께 있어야 한다. `config.json`은 공개 Client ID, 목록 주소, Ed25519 공개키와 로그인 출처를 포함하며 토큰·개인키를 넣지 않는다. 이전 입구를 찾는 데 필요한 이전 이미지 지문은 `--trusted-gateway-image`를 반복해 추가한다. 시작 페이지와 설정을 바꿀 수 있는 배포 권한은 보호해야 한다. 이 명령은 게시하지 않는다.

운영 묶음은 고정된 운영 공개키만 받으며, 개발용은 별도로 `--release-channel development`를 지정한다. 개발 공개키는 운영 묶음에서 거절한다. 키와 채널 계약은 [S76](momso-release-signing-s76.md)을 따른다.

스냅샷 `3b3bef51896729de766946b790acf7f6f2b9a773`의 실제 화면을 이 기준으로 빌드하고 묶었다. 현재 시작 화면은 프로젝트·결제 선택을 모듈에 전달하지 않는다. **기존 프로젝트를 선택해 실제 시험하기 전에는 아래 화면 연결이 필요하다.** 시작 화면의 기존 매니페스트는 `/` 범위이므로 시작 페이지 자체를 홈 화면 앱으로 설치하는 용도는 별도 결정·검증이 필요하다.

## Google 설정

1. OAuth 웹 클라이언트의 **승인된 JavaScript 출처**에 시작 페이지 출처를 등록한다. 운영은 `https://anywhere.momso.yoga`, 시험은 실제 임시 `https://….run.app`의 출처만 넣는다. `/start/`는 출처에 넣지 않는다. 로그인과 시작이 같은 출처면 한 항목이다. [Google 설정](https://developers.google.com/identity/oauth2/web/guides/get-google-api-clientid).
2. **승인된 리디렉션 URI**에는 공통 로그인 페이지 루트 전체 주소를 등록한다. 운영은 `https://anywhere.momso.yoga/`, 시험은 본부가 배포한 로그인 출처 뒤에 `/`를 붙인 정확한 주소다. 고객별 입구는 등록하지 않는다. [OpenID Connect](https://developers.google.com/identity/openid-connect/reference). 등록 직후 전파 지연으로 `redirect_uri_mismatch`가 날 수 있다.
3. Data Access에 `openid`, `email`, `https://www.googleapis.com/auth/cloud-platform`을 등록하고 해당 클라이언트 ID를 시작·로그인 설정에 넣는다. Google 공식 범위 설명에서 마지막 권한은 Google Cloud 데이터를 보고·고치고·설정하고·삭제하며 계정 이메일을 볼 수 있다는 넓은 권한이다. 이는 공식 설명이며 실제 동의 화면의 한국어 문구는 언어와 Google 정책에 따라 달라질 수 있어 실기기에서 확인한다. [공식 범위 설명](https://developers.google.com/identity/protocols/oauth2/scopes).
4. 외부 앱이 Testing이면 실제 시험 계정을 시험 사용자에 등록한다. Google은 최대 100명이라고 안내한다. `cloud-platform`의 민감·제한 분류와 공개 심사 필요 여부는 콘솔 Data Access에서 확인한다. 고객 계정에는 선택 프로젝트의 자원 생성·IAM·서비스 사용 권한과 필요한 결제 권한도 있어야 한다. [Google 시험 상태](https://developers.google.com/identity/protocols/oauth2/production-readiness/overview).
5. S65에서 시작 화면 인증은 팝업 대신 전체 화면 redirect로 바뀌었다. OAuth 클라이언트에 같은 출처의 `/start/`를 추가해야 하며 화면의 복귀 처리 연결이 필요하다. 자세한 계약과 보안 한계는 [S65](momso-anywhere-start-s65.md)를 따른다.

## 화면 연결 계약

화면이 사용자 클릭에서 `projectsFromGesture({google, clientId, release})`를 호출하면 `{projectId, displayName}[]`를 받는다. 새로 만들기는 `projectId`를 생략하고, 기존 프로젝트는 사용자가 고른 `projectId`를 `startFromGesture`에 전달한다. `billingAccountsFromGesture({google, clientId, release})`는 열린 결제 계정 `{name, displayName}[]`를 반환하며, 사용자가 고른 `name`은 `billingAccountName`으로 전달한다. 기존 프로젝트에 이미 연결된 결제 계정은 바꾸지 않는다. 토큰과 목록을 URL·기록·공개 설정에 넣지 않는다.

`startFromGesture({google, clientId, release, projectId?, billingAccountName?, onProgress})`는 `{url}`을 반환한다. `onProgress`는 `{step, state}`이고 단계는 `project`, `billing`, `services`, `privateNetwork`, `server`, `gateway`, `gatewayReady`, 상태는 `working`, `done`, `failed`다. `SetupError.step`이 까닭 값이다. 특히 `billing_selection_required`, `billing_setup_required`, `billing_account_unavailable`, `project_unavailable`, `authorization_expired`, `permission_denied`, `network`, `server_not_ready`, `gateway_not_ready`를 구분한다. 다시 시도는 같은 사용자·프로젝트의 로컬 체크포인트를 재사용한다. 완료 후 화면이 보여 주는 버튼으로 `{url}`을 연다. `findFromGesture`는 입구 주소 목록을 준다. 화면 글과 선택 UI는 화면 담당이 정한다.

## 시험 자원과 제거

기존 프로젝트 선택 시 그 프로젝트 자체·기존 결제 연결·기존 자원은 만들거나 지우지 않는다. 새 설치 ID(24자리)의 앞 12자리를 `짧은 ID`로 쓰며, 설치가 만드는 것은 `momso-<짧은 ID>` VM·부팅 디스크, `momso-<짧은 ID>-net` 네트워크·`-sub` 서브넷·`-private` 방화벽·`-router`와 NAT, `momso-any-<짧은 ID>` Cloud Run, `momso-vm-<짧은 ID>`와 `momso-gateway-<짧은 ID>` 서비스 계정, `momsoStop<짧은 ID>`·`momsoAnywhere<짧은 ID>` 역할, `momso-transport-<짧은 ID>`·`momso-session-<짧은 ID>`·`momso-vault-<짧은 ID>` 비밀값이다. API 활성화와 기존 결제 상태도 변경될 수 있다.

PC 설치기의 기존 `destroy`는 휴대폰 설치 표식 형식과 다르므로 그대로 사용하지 않는다. S65에서 휴대폰 설치용 별도 삭제 명령을 추가했다. **이 명령은 해당 VM·자동 삭제 디스크와 보관함 열쇠까지 지운다. 필요한 자료나 암호화된 로그인 복원에 쓸 열쇠라면 실행하지 않는다.** 기존 프로젝트·결제 계정·별도 백업 버킷은 지우지 않는다. 명령과 지연된 자원의 재시도는 [S65](momso-anywhere-start-s65.md)에 있다. 실제 Google 자원 삭제는 아직 시험하지 않았다.

## 검증

가짜 Google REST 시험은 새 프로젝트와 기존 프로젝트 각각에서 VM 생성 중 실패 후 같은 설치를 다시 이어 간다. 기존 프로젝트 시험은 프로젝트를 새로 만들지 않고 기존 라벨을 보존한다. 화면 묶음 시험은 `/start/` 자산 경로, 공개 설정, 모듈 포함과 잘못된 입력을 확인한다. 실제 Google API, 휴대폰, 결제·로그인, Cloud Run과 고객 VM은 본부가 별도 확인한다.
