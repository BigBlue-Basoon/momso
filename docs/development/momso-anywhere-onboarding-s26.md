정리본: [momso-anywhere-guide.md](momso-anywhere-guide.md)

# momso anywhere: 휴대폰 시작 흐름

확인일: 2026-11-21. 휴대폰 시작 로직과 고객 프로젝트 설치 흐름은 가짜 클라우드로 시험했다. **실제 Google 계정·클라우드·휴대폰에서는 아직 검증하지 않았다.** 시작 화면과 정식 산출물 게시도 아직 필요하다.

## 본부의 한 번 준비

아래는 고객마다 반복할 설치가 아니라 본부가 처음 한 번 준비하는 일이다. 결제·계정 생성·OAuth 등록과 실제 배포는 개발자의 확인 후 본부가 한다. 비밀 키나 고객 데이터는 이 문서·명령 기록·채팅에 넣지 않는다.

1. 소개 사이트 `momso.yoga`는 소개만 한다. 개발자가 확정한 공통 시작·찾기·로그인 출처는 **`https://anywhere.momso.yoga`**다. 소개 사이트에는 시작 주소 `https://anywhere.momso.yoga/start/`로 가는 링크만 둔다. 이 이름을 정한 것이며 DNS 연결이나 게시·인증서 발급을 마친 뜻은 아니다. 도메인 없이 먼저 검증할 때는 별도의 Cloud Run 기본 `run.app` 주소를 쓴다. Google OAuth의 시험 주소 등록과 실제 로그인 성공은 아직 검증하지 않았다. 공개 운영 때 도메인 소유를 확인한다. Google OAuth 검증에는 제품 홈페이지와 개인정보 처리방침 등도 필요하다. [공식 정책](https://developers.google.com/identity/protocols/oauth2/production-readiness/policy-compliance), [Cloud Run 기본 주소](https://docs.cloud.google.com/run/docs/triggering/https-request).
2. Google Cloud Console의 **Google Auth Platform → Branding**에서 앱 이름 `momso`, 지원 이메일과 홈페이지·개인정보 처리방침을 등록한다. **Audience**는 대상에 맞게 설정하고, 개발 중 Testing에는 실제 시험자의 Google 계정을 추가한다. Testing은 최대 100명이며 승인 후 7일이 지나면 다시 승인해야 한다. [공식 안내](https://support.google.com/cloud/answer/15549945)
3. **Clients → Create Client → Web application**에서 웹 클라이언트를 만든다. 시작과 로그인을 같은 출처에 두는 기본안의 운영 **Authorized JavaScript origins는 `https://anywhere.momso.yoga` 하나**다. 경로나 와일드카드는 넣지 않는다. 전체 화면 OIDC 로그인은 별도로 **Authorized redirect URIs**에 공통 로그인 페이지의 정확한 루트 주소 `https://anywhere.momso.yoga/`를 등록해야 한다. 시험용 `run.app` 출처와 그 뒤 `/`가 붙은 redirect URI는 별도로 등록한다. Google의 수락·검증 결과는 실제 시험이 필요하다. 발급된 공개 **Client ID**를 기록한다. 이 구현은 클라이언트 비밀을 브라우저에 넣지 않는다. [공식 설정](https://developers.google.com/identity/oauth2/web/guides/get-google-api-clientid), [OIDC 참조](https://developers.google.com/identity/openid-connect/reference).
4. **Data Access**에서 필요한 권한을 확인한다. 로그인은 `openid`, `email`; 고객 프로젝트 설치는 `https://www.googleapis.com/auth/cloud-platform`을 요청한다. 후자는 Google Cloud의 조회·수정·삭제를 포함하는 넓은 권한이다. Google 콘솔에서 해당 범위의 민감·제한 분류를 확인하고 필요하면 공개 전 검증을 신청한다. 분류를 이 문서에서 단정하지 않는다. [공식 범위](https://developers.google.com/identity/protocols/oauth2/scopes)
5. 소개 사이트와 별개인 출처의 `/`에 공통 로그인 페이지를, `/start/`에 시작 화면을 둔다. 같은 출처라도 두 페이지는 서로 다른 파일이다. 로그인 쪽 `config.json`에는 `{"clientId":"<3번 Client ID>"}`가 필요하고, 시작 쪽 설정에는 `release`도 필요하다. 자세한 파일 배치는 아래 **시작 화면 게시 계약**을 따른다. 고객별 입구 `run.app` 주소를 OAuth 클라이언트에 일일이 등록하지 않는다.
6. 아래 **PC 시험 명령과 한 번의 준비**에 나온 대로 줄3의 검증된 Linux 서버·화면 빌드로 VM 묶음과 입구 이미지를 만든다. 공개 산출물의 URL, VM 묶음 SHA-256, 입구 이미지의 `@sha256` 지문, 3번 Client ID와 5번 로그인 출처를 시작 화면의 신뢰된 설정에 넣는다. 현재 게시된 정식 산출물과 지문은 아직 없다.
7. 본부가 고객과 분리한 시험 Google 프로젝트에서 아래 `anywhere` 명령으로 검증할 때는 `--account`, `--project`, `--state`, 서버·화면 파일과 각각의 SHA-256, `--google-client-id`, Google이 서명한 신원 토큰으로 확인한 `--owner-sub`, `--login-origin`, 서명된 목록 주소와 고정 공개키를 명시한다. 고객 실제 자료는 쓰지 않는다. 실제 휴대폰 로그인·대화·홈 화면 확인 전에는 가짜 시험 성공을 실사용 성공이라고 적지 않는다.

## 사람이 하는 일

휴대폰에서 시작을 누르고 Google 계정을 선택·로그인한 뒤, 고객 프로젝트를 만들고 관리하는 권한과 예상 요금을 확인해 동의한다. 유효한 Google Cloud 결제 계정이 없으면 고객이 가입·결제 수단과 필요한 신원 확인을 완료해야 한다. 프로젝트와 결제 계정을 연결하려면 각각의 권한이 필요하다. 개인 프로젝트의 IAP 최초 설정은 별도 콘솔 단계가 필요할 수 있어 이 경로에서는 IAP 대신 직접 검증한 Google 로그인과 세션을 사용한다. 이 흐름이 실제 계정에서도 로그인과 동의만으로 끝나는지는 아직 시험하지 않았다. [결제 연결](https://docs.cloud.google.com/billing/docs/how-to/modify-project), [결제 계정 만들기](https://docs.cloud.google.com/billing/docs/how-to/create-billing-account), [Cloud Run IAP](https://docs.cloud.google.com/run/docs/securing/identity-aware-proxy-cloud-run).

## 구성과 우리 쪽 준비

공개 정적 시작 페이지, Google OAuth 웹 클라이언트 등록과 허용된 HTTPS 출처, 검증 가능한 momso server·화면·입구 서비스 산출물 및 SHA-256 공개가 우리 쪽에 한 번 필요하다. 정적 파일만 제공하고 고객 Google 접근 토큰을 브라우저 메모리에만 두면 우리 서버가 고객의 열쇠나 채팅을 받을 필요는 없다. 다만 시작 페이지와 공급망이 악성으로 바뀌면 브라우저의 토큰을 훔칠 수 있으므로 출처 보호·배포 검증이 필요하다. Google은 브라우저 토큰 모델로 REST/CORS를 지원하며 토큰 갱신이 필요하면 사용자의 동작으로 다시 승인받는다. [Google Identity](https://developers.google.com/identity/oauth2/web/guides/use-token-model).

2026-11-21에 계정 없이 가상 프로젝트 이름으로 보낸 `OPTIONS` 확인에서 Resource Manager·Billing·Service Usage·Compute·IAM·Secret Manager·Cloud Run의 대표 주소는 모두 `200`과 요청 출처 허용 응답을 주었다. 이는 브라우저의 사전 요청 확인일 뿐 인증된 실제 자원 생성의 성공은 아니다.

시작 로직은 고객의 명시적 승인으로 고객 프로젝트에 VM, 사설망, 전용 서비스 계정·최소 권한, 비밀 저장소, 인증된 Cloud Run 입구를 만드는 것을 목표로 한다. 입구는 고객 프로젝트의 기본 `run.app` HTTPS 주소를 쓰고 Direct VPC로 VM에 접근한다. VM에는 공인 IP를 달지 않는다. PC·휴대폰 경로 모두 가짜 공급자로만 시험했다. 고객 데이터와 서버 토큰은 고객 프로젝트 안에서만 다뤄야 한다. [Direct VPC](https://docs.cloud.google.com/run/docs/configuring/vpc-direct-vpc), [Secret Manager 권한](https://docs.cloud.google.com/secret-manager/docs/access-control).

VM 안 프록시는 VM에서 만든 인증서의 지문을 입구가 고정해 확인하고 별도 전송 비밀을 확인한 뒤에만 `127.0.0.1`의 momso server에 서버 토큰을 붙인다. 전송 비밀과 로그인 세션 키는 고객 프로젝트 Secret Manager에 두고 전용 서비스 계정에 해당 비밀만 읽을 권한을 준다. VM의 지문과 전송 비밀은 설치 시 인증된 IAP SSH로 읽는다. 이 경로는 실제 클라우드에서 아직 시험하지 않았다.

사람 확인은 IAP의 개인 프로젝트 최초 콘솔 설정 대신 Google의 서명된 ID 토큰을 공식 라이브러리로 검증하고, 미리 확인한 주인의 불변 `sub`와 일회용 nonce가 맞을 때만 서명된 Secure·HttpOnly 세션 쿠키를 발급한다. 인증 없이 채팅·화면·API를 받을 수 없다. 공통 정적 로그인 페이지는 Google OAuth에 등록된 고정 출처를 쓰고, 그 페이지가 돌려줄 입구의 출처와 referrer가 다르면 멈춘다. 입구의 challenge 전체를 Google nonce에 묶어 다른 입구로 옮긴 응답을 거절한다. Google은 각 출처 등록을 요구하고 와일드카드를 허용하지 않으므로 임의 고객의 `run.app` 전체를 공통 OAuth 클라이언트에 등록하는 방식은 쓰지 않는다. [Google ID 토큰 검증](https://developers.google.com/identity/gsi/web/guides/verify-google-id-token), [OAuth 출처 규칙](https://developers.google.com/identity/protocols/oauth2/javascript-implicit-flow).

입구가 꺼진 VM을 켜고 준비될 때까지 기다린 뒤 같은 주소에서 화면과 API를 제공할 수 있다. 화면이 보내는 턴 결과는 SSE로 전달한다. Cloud Run 요청 제한은 기본 5분, 설정 시 최대 60분이고 긴 연결은 중간에 끊길 수 있으므로 화면은 서버의 offset으로 다시 받아야 한다. 공식 한도와 로컬 가짜 스트림 시험은 실제 몇 분짜리 휴대폰 연결의 성공을 보장하지 않는다. [시간 제한](https://docs.cloud.google.com/run/docs/configuring/request-timeout)을 참고한다.

Cloud Run은 사용량과 지역에 따라 무료 한도 이후 과금될 수 있다. VM이 정지해도 디스크·네트워크 등 별도 비용이 남을 수 있다. VPC 전송도 비용이 생길 수 있고, 별도 VPC 커넥터를 쓰면 커넥터 비용도 생긴다. 따라서 지금 숫자 하나를 확정하지 않고 고객 지역·사용량을 넣어 [Cloud Run 요금](https://cloud.google.com/run/pricing)과 [네트워크 요금](https://cloud.google.com/vpc/network-pricing)으로 계산해야 한다. 유휴 자동 종료는 서버의 진행 중 턴·쓰기·백업을 확인해야 하므로 별도 계약이다.

금액 감각: Google의 요청 기반 Cloud Run 무료 한도는 청구 계정 합산 월 200만 요청·18만 vCPU초·36만 GiB초이며 지역별 요금으로 환산된다. 공식 NAT 표는 할당된 VM 한 대당 시간당 $0.0014, 공인 IP 하나당 시간당 $0.005, 처리량 GiB당 $0.045를 제시한다. VM 한 대와 IP 한 개가 한 달 720시간 내내 과금된다고 가정하면 NAT 시간 요금만 약 $4.61이고 처리량·VM·디스크 등은 별도다. 정지 중 실제 IP 할당과 서울 가격·세금은 확인해야 하므로 이것은 월 청구액 확정이 아니다. [Cloud Run 요금](https://cloud.google.com/run/pricing), [NAT 요금](https://cloud.google.com/vpc/network-pricing).

## 구현·시험과 남은 연결

| 부분 | 현재 상태 |
| --- | --- |
| `scripts/momso_anywhere_gateway.py`, `scripts/momso_anywhere_vm_proxy.py` | 소유자 세션을 확인하고 지문이 맞는 사설 TLS로 화면·API·SSE를 중계한다. 문서 탐색에서 VM이 꺼졌으면 켠 뒤 대기 화면을 5초마다 다시 연다. 로컬 가짜 서버로만 시험했다. |
| `scripts/momso_anywhere_start.mjs`, `scripts/momso_anywhere_google_browser.mjs` | Google 권한·고유 ID 확인과 프로젝트·결제·API·사설망·비밀·VM·입구 생성, 실패 후 재시도. 로컬 가짜 Google API로만 시험했다. 서버·입구의 검증된 산출물이 없으면 변경 전에 거절한다. |
| 휴대폰 화면·Google 로그인 | 스냅샷 `911cb60`에 시작 화면이 있고 가짜 공급자로 확인했다. 공개 호스팅·OAuth 등록과 실제 Google 로그인은 아직 시험하지 않았다. 화면은 시작 완료 뒤 고객 입구의 같은 출처로 서버를 부르고 AI 연결을 진행한다. |
| 고객 프로젝트 설치 | 브라우저 REST 경로와 별도로 PC의 `gcloud` 경로도 있다. 둘 모두 실제 고객 프로젝트에 적용하지 않았다. 화면의 로그인·채팅을 통한 실사용 확인은 남았다. |
| PC 설치 경로 | `anywhere` 명령은 검증된 화면 묶음·서버와 사설 TLS·입구를 연결한다. 가짜 공급자로만 시험했다. 실제 앱 화면에서 같은 출처 로그인과 채팅이 되는지는 미검증이다. |

화면 담당 계약: 시작 단추의 직접 클릭에서 `startFromGesture({google, clientId, release, onProgress})`를 부르고 반환된 `{url}`을 완료 화면에 표시한다. 화면에서 **몸소 열기**를 누를 때 고객 입구로 이동해도 된다. `onProgress`는 `{step,state}`를 주며 에러의 `step`으로 결제 준비·다시 시도 등을 구분한다. 인증 뒤 같은 출처의 상대 경로를 사용하고 서버 토큰 입력을 요구하지 않는다. API로 확인한 입구 준비와 사람이 실제로 로그인해 채팅한 성공은 다르다. 서버 담당 계약: 화면 산출물 서빙, SSE 재접속 offset, 실제 AI 연결 상태와 안전한 유휴 종료를 제공한다. 입구가 공개되더라도 인증 전에는 로그인 핸드셰이크와 정해진 공개 아이콘·매니페스트만 제공한다.

## 시작 화면 게시 계약

이 절은 스냅샷 `911cb60` 화면과 현재 S26 코드의 계약이다. **게시와 실제 휴대폰 시험은 아직 하지 않았다.**

1. 운영 주소는 소개 사이트와 별개인 **`https://anywhere.momso.yoga`**다. 공통 로그인은 `https://anywhere.momso.yoga/`, 시작·내 몸소 찾기는 `https://anywhere.momso.yoga/start/`에 둔다. 시작 화면의 `config.json`, `momso_anywhere_start.mjs`, `momso_anywhere_setup.mjs`, `momso_anywhere_google_browser.mjs`, `momso_anywhere_vm_startup.mjs`는 모두 `/start/`에 둔다. 모듈은 서로 상대 경로로 가져온다. 화면 빌드가 참조하는 `/assets/`, `/app-icons/`, `/brand/`는 그 출처의 루트에서 제공한다. 반드시 `/start/`처럼 끝에 `/`가 있는 주소를 열거나 서버가 그 주소로 보내야 `./config.json`과 모듈이 `/start/`에서 읽힌다. `.mjs`는 JavaScript MIME 형식으로, `config.json`은 JSON으로 제공한다. 화면은 설정과 모듈을 미리 읽고, Google 권한 요청은 시작·찾기 단추를 직접 누른 자리에서 호출한다. 도메인 없는 첫 시험에서는 같은 배치를 임시 `run.app` 출처에 둔다.
2. 시작 화면의 같은 폴더 `./config.json`은 `{ "clientId": "<Google 웹 클라이언트 ID>", "release": { "manifestUrl": "<서명된 목록 HTTPS 주소>", "publicKeySpki": "<고정된 Ed25519 공개키 SPKI base64>", "loginOrigin": "<공통 로그인 HTTPS 출처>", "channel": "production", "trustedGatewayImages": ["<필요한 이전 입구 이미지@sha256:지문>"] } }`다. `trustedGatewayImages`는 선택이다. 개발 시험에는 별도 키와 `development` 채널을 쓴다. 서명은 목록 주소 뒤 `.sig`에 있다. `release`가 없거나 검증에 실패하면 시작 모듈은 `release_not_ready`로 멈춘다. 화면이 이 값을 임의로 채우지 않는다. 설정에는 공개 값만 두고 OAuth 접근 토큰·서버 토큰·개인 키는 넣지 않는다. 설정을 바꿀 수 있는 배포 권한을 보호해야 한다.
3. 공통 Google 로그인은 같은 출처의 루트 `/`에 로그인용 `index.html`, `boot.mjs`, `momso_anywhere_login.mjs`, 로그인용 `config.json`을 둔다. `release.loginOrigin`과 PC 설치의 `--login-origin`은 이 출처만이며 `/start/` 같은 경로가 아니다. 입구는 로그인 출처의 `/`로 이동시킨다. 시작 화면과 로그인 화면을 같은 `/`에 덮어쓰면 안 된다. 기본안에서는 Google OAuth의 JavaScript 출처 하나와 로그인 루트 redirect URI 하나를 등록한다. 만약 두 페이지를 **서로 다른 출처**에 두면 시작 화면도 자기 출처의 `/`에 둘 수 있지만 각각의 정확한 출처를 Google OAuth에 등록해야 한다. 시작 쪽 `release.loginOrigin`과 설치 인자는 로그인 출처를 가리킨다. 어느 경우에도 소개 사이트나 고객별 입구를 OAuth 출처로 추가할 필요는 없다. 아래 배치는 코드 대조로 정한 것이고 실제 서버에서 아직 검증하지 않았다.
4. 고객의 홈 화면 앱은 시작 페이지가 아니라 **고객별 `run.app` 입구**에서 추가한다. 시작 빌드의 기존 `/manifest.webmanifest` 링크는 시작 페이지에서 404가 날 수 있다. 화면 담당은 시작 빌드에서는 그 링크를 빼고, 고객 입구에는 기존 매니페스트와 아이콘을 둔다. 시작 사이트에 고객 입구용 매니페스트를 복사하면 시작 주소가 시작 사이트를 가리키므로 대체가 아니다. 완료 화면의 안내 순서는 **몸소 열기 → 입구 로그인 → 그 입구를 홈 화면에 추가**다. [웹 앱 시작 주소](https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Manifest/Reference/start_url).
5. 결제 계정이 둘 이상이면 `SetupError.step = "billing_selection_required"`를 돌려준다. 화면은 사용자 동작에서 `billingAccountsFromGesture({google, clientId, release})`로 열린 계정의 `{name, displayName}` 목록을 받고, 사용자가 고른 `name`을 `startFromGesture`의 `billingAccountName`으로 전달해 재시도할 수 있다. 목록은 모든 페이지를 읽으며 연결 전에 다시 확인한다. 화면의 계정 선택 UI는 아직 연결되지 않았다. 계정 목록 조회 권한이 연결 권한까지 뜻하지는 않는다.
6. 이미 연결된 결제 계정은 자동 변경하지 않는다. 닫혔거나 목록에서 사라진 선택은 `billing_account_unavailable`이다. 인증 토큰과 계정 목록은 브라우저 메모리에서만 다루며 기록·URL·설치 체크포인트에 남기지 않는다. [계정 목록](https://docs.cloud.google.com/billing/docs/reference/rest/v1/billingAccounts/list), [프로젝트 연결](https://docs.cloud.google.com/billing/docs/reference/rest/v1/projects/updateBillingInfo).
7. `startFromGesture` 성공의 `{url}`은 입구 준비 결과다. 자동 이동은 필수가 아니며 완료 화면에 머물러 복사하거나 **몸소 열기**를 눌러 이동하는 현재 화면과 충돌하지 않는다. 주소가 없으면 성공으로 보이지 않는다. `findFromGesture`는 현재 코드가 검증한 입구 URL 배열을 준다.

### 정적 파일을 올리는 차례

아래는 **게시하지 않은 준비 절차**다. 실제 시험용 프로젝트·서비스 이름과 검증된 이미지 지문은 본부가 정한다. 소개 사이트의 파일이나 설정은 바꾸지 않는다.

1. 화면 스냅샷의 `apps/alpha/desktop`에서 `npm ci`를 실행한 다음 PowerShell에서 `$env:VITE_MOMSO_START = '1'`과 `npm run build`를 차례로 실행한다. 만들어진 `dist`의 실제 파일과 `index.html`의 자산 경로를 확인한다. 시작 화면의 매니페스트 링크 제거는 화면 담당이 적용해야 하며, 현재 스냅샷은 아직 해당 링크가 있다.
2. 새 게시 폴더의 `public` 아래를 다음과 같이 만든다. `dist/index.html`은 `public/start/index.html`로, `dist/assets`, `dist/app-icons`, `dist/brand` 등 실제 빌드가 `/`에서 찾는 산출물은 `public/` 바로 아래로 복사한다. 로그인용 `index.html`은 시작 빌드의 파일이 아니라 `scripts/momso_anywhere_login_site/index.html`이다. 시작 화면과 로그인 화면의 두 설정은 각각 별도 파일이다.

```text
public/
  index.html                              공통 로그인 화면
  config.json                             { clientId }
  boot.mjs
  momso_anywhere_login.mjs
  assets/  app-icons/  brand/             시작 빌드의 공개 자산
  start/
    index.html                            시작 빌드의 index.html
    config.json                           { clientId, release }
    momso_anywhere_start.mjs
    momso_anywhere_setup.mjs
    momso_anywhere_google_browser.mjs
    momso_anywhere_cloud_run_revision.mjs
    momso_anywhere_vm_startup.mjs
```

3. 두 `config.json`에 같은 공개 Client ID를 넣고, `public/start/config.json`의 `release`에는 위 계약의 공개 URL·SHA-256·이미지 지문과 **이 정적 사이트의 HTTPS 출처만** `loginOrigin`으로 넣는다. 운영 값은 `https://anywhere.momso.yoga`, 첫 시험 값은 본부가 받은 임시 `run.app` 출처다. 이 값에 `/start/`나 마지막 `/`는 붙이지 않는다. 고객의 채팅·열쇠·토큰은 이 사이트에 넣지 않는다. 소개 사이트의 운영 링크는 `https://anywhere.momso.yoga/start/`다.
4. 정적 호스팅에는 위 `public/`만 게시하고 `/start/`에서 `index.html`을 제공한다. Cloud Run 기본 `run.app` 주소로 도메인 없이 시험하려면 정적 파일을 제공하는 웹 서버 이미지가 필요하다. 아래는 그 이미지를 만드는 예다. `<검증된 nginx 이미지@sha256:지문>`을 본부가 확인한 이미지로 바꾸고, 아래 두 파일을 게시 폴더에 둔다. 실제 명령은 계정·프로젝트·기존 이미지 저장소와 비용을 본부가 확인한 뒤 실행한다.

```dockerfile
FROM <검증된 nginx 이미지@sha256:지문>
COPY nginx.conf /etc/nginx/nginx.conf
COPY public/ /usr/share/nginx/html/
```

```nginx
events {}
http {
  include /etc/nginx/mime.types;
  types { application/javascript mjs; }
  access_log off;
  server {
    listen 8080;
    root /usr/share/nginx/html;
    index index.html;
    add_header Cross-Origin-Opener-Policy same-origin-allow-popups always;
    add_header Referrer-Policy strict-origin always;
    location / { try_files $uri $uri/ =404; }
  }
}
```

5. 예를 들어 본부가 준비한 Artifact Registry 이미지 주소를 `<이미지>`라 하면 PowerShell에서 `gcloud builds submit "<게시 폴더>" --tag "<이미지>" --project "<시험 프로젝트>" --account "<시험 계정>"` 다음 `gcloud run deploy "<시험 서비스>" --image "<이미지>" --region asia-northeast3 --port 8080 --allow-unauthenticated --project "<시험 프로젝트>" --account "<시험 계정>"`을 실행한다. 공개되는 것은 위 정적 파일뿐이다. 배포가 준 `https://….run.app` 출처와 끝에 `/`가 붙은 redirect URI를 Google OAuth에 등록하고, 두 설정과 설치 명령의 `--login-origin`에 같은 출처를 넣어 이미지 재게시가 필요하면 다시 한다. 서비스·이미지 저장소 준비, Google의 시험 주소 수락, 전체 화면 복귀·실제 휴대폰 동작은 아직 확인하지 않았다. 실제 정식 주소의 DNS·인증서·OAuth 검증도 따로 필요하다.

## PC 시험 명령과 한 번의 준비

본부는 공개 HTTPS 정적 출처에 `scripts/momso_anywhere_login_site/index.html`과 `scripts/momso_anywhere_login.mjs`를 같은 폴더로 게시하고, 그 폴더의 `config.json`에 `{"clientId":"<Google 웹 클라이언트 ID>"}`를 준비한다. Google OAuth 클라이언트에는 이 고정 출처를 등록한다. 시작 사이트·개인정보 처리방침·도메인 소유 확인 등 Google 정책도 충족해야 한다. 화면 묶음은 루트에 `index.html`이 있는 tar.gz이며 서버와 화면의 SHA-256을 별도로 확인한다. 소유자 `sub`는 Google이 서명한 ID 토큰을 검증한 뒤 얻어야 하며 이메일을 임의 숫자로 바꾼 값이 아니다. [Google 검증 준비](https://developers.google.com/identity/protocols/oauth2/production-readiness/policy-compliance).

휴대폰 경로용 공개 VM 묶음은 `python scripts/prepare_momso_anywhere_bundle.py --server "<서버>" --server-sha256 "<서버 SHA-256>" --web "<빌드된 화면 폴더>" --output "<새 tar.gz 경로>"`로 만든다. 입구 이미지는 `python scripts/prepare_momso_anywhere_image.py --web "<빌드된 화면 폴더>" --output "<새 이미지 빌드 폴더>"`로 맥락을 준비하고 이미지의 `@sha256:…` 지문을 확정한다. 이 두 주소와 지문은 S38 목록에 담아 개인키 소유자가 서명한다. 화면 설정에는 목록 주소·공개키·로그인 출처만 둔다. 공개 정적 시작 페이지와 그 설정 파일의 무결성은 배포자가 보호해야 한다. 이미지·묶음 게시와 정식 서명은 아직 하지 않았다. [S73](momso-anywhere-signed-install-s73.md)

VM은 Codex CLI Linux x86-64 공식 배포본 `rust-v0.169.0`을 원 배포처에서 직접 받고 압축 파일 SHA-256 `9af4d751fce183faedea629437d5252dbf458eeb04130cf4f97be844621ea985`을 확인한 뒤 설치한다. Debian의 `bubblewrap`이 없으면 설치하며, 연결 계정은 고객이 화면에서 직접 승인한다. 이 판과 momso server의 실제 로그인·대화 호환성은 아직 시험하지 않았다. [Codex 공식 배포 안내](https://github.com/openai/codex), [해당 릴리스 산출물](https://github.com/openai/codex/releases/tag/rust-v0.169.0).

```powershell
python scripts/momso_server_install.py anywhere --account "<Google 계정>" --project "<고객 프로젝트 ID>" --state "<설치 기록 경로>" --artifact "<Linux 서버 파일>" --sha256 "<서버 SHA-256>" --web-archive "<화면 tar.gz>" --web-sha256 "<화면 SHA-256>" --google-client-id "<OAuth 웹 클라이언트 ID>" --owner-sub "<검증된 주인 sub>" --login-origin "https://anywhere.momso.yoga" --release-manifest-url "<서명된 목록 HTTPS 주소>" --release-public-key "<설치기에 고정한 공개키 PEM>"
```

첫 시험에서는 위 `--login-origin`을 시험 정적 사이트의 임시 `https://….run.app` 출처로 바꾸고 그 출처를 Google OAuth에 등록한다. 고객 프로젝트의 Cloud Run·Secret Manager·Compute API, 결제와 설치자의 IAM 권한을 준비한다. 미리 만든 이미지의 저장소 접근 권한과 서명 검증 한계는 [S72](momso-anywhere-gateway-images-s72.md)를 따른다. 이 명령은 고객 프로젝트에 유료 자원을 만들 수 있다. 실제 실행·로그인·채팅은 본부의 별도 확인이 필요하며, 화면 빌드가 아직 토큰 입력을 요구하면 화면 담당이 같은 출처 계약을 적용해야 한다.

Windows에서는 `gcloud compute ssh`가 `plink`를 사용할 때 파이프로 보낸 표준 입력이 전달되지 않고 대기하는 경우가 실제로 있었다. 설치기는 서버와 화면 묶음 등 입력 자료를 동일한 IAP·엄격한 호스트 키 검사와 전용 SSH 키를 쓰는 `gcloud compute scp`로 임시 전달하고, VM에서 SHA-256을 확인한 뒤 사용한다. `gcloud`의 기본 계정·프로젝트 설정은 바꾸지 않고 모든 호출에 명시한다. 전송 뒤 SSH 실행 전 통신이 완전히 끊기면 VM의 `/tmp`에 임시 전달 파일이 남을 수 있다. 설치가 끝까지 되는지는 실제 재시험 결과로 별도 기록한다.

실제 시험에서는 Debian 12의 Python 3.11.2가 `tarfile.extractall(..., filter=...)`를 받지 않아 화면 압축 해제가 실패했고, 프록시가 켜지는 직후 상태를 확인해 일시적으로 연결이 거절되는 경우도 있었다. 화면 묶음은 상대 경로·일반 파일·디렉터리만 확인하고, `data_filter`를 지원하는 Python에서만 그 인자를 전달한다. 프록시는 최대 30초 동안 준비 상태를 확인한다. 본부가 시험 사본으로 이 두 곳을 고쳐 설치를 마쳤고, 작업 브랜치에도 같은 취지의 수정과 가짜 시험을 넣었다. 이 브랜치의 최종 커밋으로 전체 실제 설치를 다시 한 것은 아니다.

휴대폰 설치의 VM 시작 스크립트도 같은 방법으로 옛 Python을 지원한다. 두 경로 모두 흔한 `./` 루트 디렉터리와 `./파일`을 받아들이며, 절대 경로·상위 폴더 이동·심볼릭 링크·하드 링크는 풀기 전에 거절한다. 필터 인자가 없는 환경은 가짜로 시험했다. 다른 VM용 Python 파일 네 개도 3.11 문법으로 구문 검사했으나 모든 실행 경로를 실제 3.11.2에서 실행한 것은 아니다.

## Google 심사

기본 신원 범위 `openid`, `email`과 고객 프로젝트 자원을 만들기 위한 `cloud-platform` 권한을 요청한다. 후자는 프로젝트의 자료를 보고 바꾸고 지울 수 있는 매우 넓은 권한이며, 실제 자원 변경에는 해당 계정의 프로젝트 생성·결제 연결·서비스 활성화·Compute·IAM·Secret Manager·Cloud Run 권한도 각각 필요하다. Google 공식 범위 목록만으로 이 범위의 민감·제한 분류는 확인하지 못했으므로 OAuth 콘솔에서 확인하고 필요 시 검증한다. 외부 앱의 테스트 상태는 등록된 시험 사용자 최대 100명이며, 해당 조건의 갱신 토큰에는 7일 제한이 있을 수 있다. 이 코드는 갱신 토큰을 저장하지 않는다. [범위 목록](https://developers.google.com/identity/protocols/oauth2/scopes), [Google의 테스트 상태와 검증](https://developers.google.com/identity/protocols/oauth2/production-readiness/overview), [서비스 계정 연결 권한](https://cloud.google.com/iam/docs/service-account-permissions).

### S44 설치와 화면 계약

브라우저 시작 모듈은 `project`, `billing`, `services`, `privateNetwork`, `server`, `gateway`, `gatewayReady`를 순서대로 `{step, state: 'working'|'done'|'failed'}`로 알린다. 오류는 `SetupError.step`의 코드로 전달한다. 결제 계정이 여러 개면 `billing_selection_required`이며 위의 목록·선택 계약을 화면에 연결해야 한다. Google IAM 변경이 전파되기까지 시간이 걸릴 수 있어 정책 설정의 일시적인 400을 최대 6회 재시도한다. 계속 실패하면 오류를 반환하며 다음 시작 때 같은 설치 ID로 다시 조정한다. [IAM 전파](https://cloud.google.com/iam/docs/access-change-propagation).

새 프로젝트 대신 기존 것을 고를 때 화면은 사용자 동작에서 `projectsFromGesture({google, clientId, release})`로 활성 프로젝트의 `{projectId, displayName}` 목록을 받고 `startFromGesture`에 `projectId`를 전달한다. 선택 프로젝트는 없으면 다시 만들지 않고 `project_unavailable`을 반환하며, 기존 프로젝트의 이름·라벨·결제 연결을 임의로 바꾸지 않는다. 화면 선택 UI는 아직 연결되지 않았다. '내 몸소 찾기'는 접근 가능한 활성 프로젝트의 입구 목록을 읽고 설치 표시·신뢰 이미지·계정과 설정을 확인하므로 기존 프로젝트도 찾을 수 있다. 프로젝트에서 Cloud Run 목록을 읽을 권한이 없거나 이전 이미지 지문이 빠지면 발견하지 못할 수 있다. 실제 Google 권한과 브라우저에서 목록·설치 성공 여부는 미검증이다.

시작 페이지가 서명된 목록을 확인하면 VM은 그 목록의 공개 HTTPS `bundleUrl`에서 server·웹·프록시가 든 묶음을 받고 `bundleSha256`을 비교한다. 예를 들어 본부가 공개 읽기를 설정한 Cloud Storage 객체의 `https://storage.googleapis.com/<bucket>/<object>`를 제공할 수 있다. 시작 페이지의 고정 공개키나 코드를 바꿀 수 있는 공격자는 신뢰를 바꿀 수 있으므로 배포 경로도 보호해야 한다. 공개 객체에는 계정 토큰이나 고객 자료를 넣지 않는다. [공개 객체 주소](https://cloud.google.com/storage/docs/access-public-data). 프록시가 뜰 때까지 최대 30초 기다리며 Debian 12의 구형 Python도 받아들이는 경로를 사용한다. 최초 묶음을 설치한 VM을 재부팅해도 그 묶음으로 업데이트된 파일을 덮지 않는다. 기존 설치로 판단할 수 있는 파일이 없어진 경우에는 옛 묶음을 다시 덮는 대신 오류를 낸다.

위 재부팅 보호는 **새 시작 스크립트가 이미 적용된 VM**의 규칙이다. 옛 시작 스크립트로 설치된 VM은 메타데이터를 바꾸는 것만으로 보호됐다고 볼 수 없다. 재부팅 시 기존 `momso-bootstrap.service`가 옛 `/opt/momso/bootstrap.py`를 먼저 실행할 수 있기 때문이다. 기존 VM에서 옛 자동 설치를 안전하게 멈추고 새 스크립트로 옮기는 절차와 실제 재부팅 검증은 아직 없다. 따라서 그 전에는 [S38 업데이트 부품](server-update-s38.md)을 실제 기존 VM에 연결하지 않는다. 새 스크립트도 기존 systemd 서비스 파일을 부팅 때 덮어쓰지 않는다. S38의 서명 검증·작업 중지·스냅샷·복구와는 별개의 조치이며 그것을 완성했다는 뜻은 아니다.

휴대폰 설치도 [S56 계약](server-installer-s56.md)에 맞춰 VM 전용 서비스 계정, VM 하나만 끌 수 있는 권한, 전용 보관함 Secret Manager 비밀값을 준비한다. 브라우저는 보관함 열쇠를 만들거나 읽지 않는다. VM에서 `age-keygen`으로 생성하고 비밀값의 1번 판을 재사용하며, 초기화 완료와 해당 판을 확인한 뒤 임시 버전 추가 권한을 회수한다. 기존 암호화 자료가 있는데 열쇠가 없으면 새 열쇠로 덮지 않는다. 열쇠와 서버 토큰은 systemd credential로 전달한다. 유휴 종료는 server의 안전 확인이 통과할 때만 시도하고 백업이 연결되지 않았다는 응답에서는 종료하지 않는다. VM 묶음에 두 Python 도우미가 포함돼야 한다. 실제 VM에서의 최초 생성·재부팅·안전 종료는 별도 검증이 필요하다.

열쇠 최초 생성 권한은 설치 중 실패해도 회수를 시도한다. 브라우저가 강제 종료되어 회수 코드를 실행하지 못하는 경우에도 권한이 계속 유효하지 않도록 해당 VM 계정의 임시 IAM 조건을 15분 뒤 만료시킨다. 만료 시각은 휴대폰 시계를 기준으로 하므로 시계가 크게 틀리면 설치가 실패하거나 권한이 예상보다 오래 남을 수 있다. 만료 후 초기화가 아직 안 됐다면 같은 설치를 다시 시작해 권한을 다시 얻는다. IAM 변경의 전파 지연과 회수 실패는 별도로 확인해야 한다. [Secret Manager의 시간 조건](https://cloud.google.com/secret-manager/docs/access-control), [Google의 임시 권한 예시](https://cloud.google.com/iam/docs/configuring-temporary-access).

본부의 실제 올리기 시험에서 발견한 두 문제를 PC 설치기 코드에서 보강했다. 교체 전에 server·화면·설정의 복구 사본을 만들고 새 판 건강 검사가 실패하면 파일을 되돌린 뒤 건강을 다시 확인한다. 이미 서비스가 죽어 있어도 교체를 시작하며 `reset-failed`를 시도한다. 복구가 실패하면 사본을 남긴다. 가짜 서비스와 임시 파일로 확인했으며 실제 VM은 시험하지 않았다. **DB 마이그레이션이 이미 일어난 경우 자료까지 되돌리는 기능, 작업 접수 중지와 중단 후 자동 복구는 아직 없다.** 이것은 [S38 계약](server-update-s38.md)의 완전한 쓰기 중지·자료 스냅샷·복구를 충족하지 않으므로 DB 변경 판을 안전하게 되돌린다고 보장할 수 없다. 본부가 보존한 실제 실패 자료는 시험이나 정리 대상으로 삼지 않는다.

가짜 Google REST 시험은 시작 단추에서 프로젝트·결제·사설망·VM·입구까지 공급자 코드를 연결하고 VM 생성 중 실패한 뒤 같은 설치를 이어가는 경우를 확인한다. **실제 Google API의 브라우저 CORS, 프로젝트·결제 권한, 실제 클라우드 전체 단계와 실제 휴대폰은 아직 검증하지 않았다.** Google의 공개 이미지와 묶음은 본부가 준비해야 한다. PC 설치기 destroy가 서브넷·네트워크 삭제에 실패하면 남은 것으로 확인된 이름을 설치 상태 파일의 `destroy_pending`에 적고 같은 명령을 다시 실행하도록 안내한다. 주소 회수 지연 외의 권한 오류일 수도 있다.

서버 교체 전에 실행 파일·화면·설정의 복구 사본을 만들고 건강 검사가 실패하면 파일을 되돌린 뒤 건강을 다시 확인한다. 복구가 실패하면 사본을 남긴다. 가짜 서비스와 임시 파일로 확인했으며 실제 VM은 시험하지 않았다. DB 마이그레이션이 이미 일어난 경우 자료까지 되돌리는 기능과 중단 후 자동 복구는 없다. 따라서 DB 변경 판을 안전하게 되돌린다고 보장할 수 없다.

**남은 확인:** 화면 담당이 시작 화면과 서버의 같은 출처 호출을 연결하고, 본부가 공개 산출물·로그인 페이지·OAuth 클라이언트를 준비한 뒤 실제 계정에서 실행·로그인·AI 연결·채팅·긴 SSE를 확인해야 한다. 결제 계정이 여러 개면 임의로 고르지 않고 선택이 필요하다고 돌려준다. Google 로그인으로 기존 입구를 찾는 계약과 iPhone 홈 화면의 한계는 [홈 화면·입구 문서](momso-anywhere-pwa-entry-20261121.md)에 적었다. 가짜 Google 응답의 통과는 실제 Google API 성공을 뜻하지 않는다.
