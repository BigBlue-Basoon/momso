정리본: [momso-anywhere-guide.md](momso-anywhere-guide.md)

# S42 로그인과 기존 설치 갱신 진행 기록

## 정해진 방식

Google의 [OpenID Connect discovery 문서](https://accounts.google.com/.well-known/openid-configuration)는 `response_types_supported`에 `id_token`을, `response_modes_supported`에 `fragment`를 표시한다. [공식 OIDC 참조](https://developers.google.com/identity/openid-connect/reference)는 `id_token` 요청에 nonce가 필요하며 응답을 fragment로 받을 수 있다고 설명한다. 범위는 공식 참조가 `openid`와 `email` 또는 `profile`을 요구하므로 `openid email`을 요청한다. 이는 액세스 토큰을 요청하지 않는 ID 토큰 경로다.

로그인 전 고객 입구는 서명된 state와 단기 challenge 쿠키를 만들고 글자 없는 200 페이지에서 공통 로그인 페이지로 이동한다. 스크립트가 없으면 meta refresh를 사용한다. 공통 페이지는 state의 `run.app` 출처와 referrer가 있으면 그 출처를 확인해 이 탭의 `sessionStorage`에 저장한다. 단추를 누르면 별도의 무작위 state와 challenge 해시 nonce를 넣어 Google로 전체 화면 이동한다. Google이 공통 페이지의 `/`에 fragment로 돌려준 결과는 즉시 주소에서 지운 뒤 무작위 state를 확인하고, 고객 입구의 `/__momso/auth/return`에 fragment로 전달한다. 입구의 빈 페이지도 fragment를 먼저 지운 다음 같은 출처의 `/__momso/auth/finish`에 POST한다. 입구가 challenge 쿠키·서명·원래 출처·Google 서명과 대상·주인 `sub`·nonce를 확인한 후에만 세션을 발급한다. challenge 쿠키는 돌아오는 상위 문서 이동을 위해 `SameSite=Lax`, 세션 쿠키는 기존 `SameSite=Strict`다. 인증 실패는 새 challenge를 만든 공통 페이지의 오류 상태로 돌아간다.

우리 쪽 공통 로그인 사이트는 정적 파일만 제공하며 ID 토큰 처리 서버를 새로 두지 않는다. 토큰은 URL의 fragment와 브라우저 메모리를 지나며 fragment는 각 페이지에서 즉시 지운다. fragment가 서버 요청에 실리지 않는다고 해서 기기·브라우저 확장·악성 스크립트로부터도 숨겨진다는 뜻은 아니다. 공통 정적 파일과 고객 입구의 공급망을 신뢰한다. referrer가 있으면 state 안의 출처와 다른 사이트에서 건넨 로그인 요청을 사전에 거절하지만, 브라우저나 출처가 referrer를 생략할 수 있으므로 인증 증명이 아니다. 비어 있을 때는 허용하되, 그 경우 잘못된 로그인 시도를 사전에 걸러내는 보조 수단이 줄어든다. 인증은 고객 입구의 state 서명·challenge 쿠키·nonce·Google 서명/aud·주인 sub 검증에 의존한다. 임의의 유효한 `run.app`으로 유도하는 피싱과 공통 페이지 공급망 공격을 이 검사만으로 막는다는 뜻은 아니다. [Referrer-Policy 설명](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Referrer-Policy). 고객별 입구를 Google OAuth에 등록할 필요는 없지만, **공통 로그인 페이지의 `/` 전체 주소는 Google OAuth의 승인된 redirect URI에 정확히 등록**해야 한다. 등록 직후에는 전파 지연으로 `redirect_uri_mismatch`가 잠시 날 수 있으며, 실제 시험에서는 약 30분 뒤 성공했다(보장된 시간은 아니다).

오류 상태에서도 단추를 사용할 수 있다. 이 탭에 검증된 입구 출처와 state가 남아 있으면 다시 Google로 이동한다. challenge는 입구에서 5분간 유효하므로 발급 후 4분이 지났거나 발급 시각이 잘못된 경우에는 단추를 누를 때 입구로 돌아가 새 challenge를 받는다. 남은 문맥이 없는 오류는 페이지를 다시 열어 확인하며 인증을 임의로 진행하지 않는다. 다른 상태 중 `ready`만 단추가 켜지고 `working`과 `invalid`는 꺼진다.

화면 담당 스냅샷 `c54362a`의 HTML·CSS와 공개 아이콘으로 묶음을 만드는 데 성공했다. 실제 Google 로그인과 휴대폰 확인은 여전히 별도로 필요하다.

## 제품 연결 현황

- A·B: 전체 화면 OIDC와 입구의 자동 이동을 작성했다. 가짜 Google 응답·로컬 HTTP 입구·모의 브라우저로 state·fragment 제거·세션 발급을 연결한 시험을 Windows와 별도 임시 Linux(Alpine 3.24.4, Python 3.14.10, Node 24.22.3)에서 실행했다. 본부는 이전 판에서 실제 Google 로그인과 세션 유지를 확인했다. **S43 변경 뒤 실제 Google·휴대폰 홈 화면 검증과 실제 입구 재배포는 아직 하지 않았다.** 특히 Safari의 referrer와 탭별 sessionStorage·쿠키 처리는 실제 기기에서 확인해야 한다.
- C: 화면 담당의 `login-page/index.html`, `login.css`를 고치지 않고 `--login-page-dir`로 받고, `--web`으로 화면 빌드 폴더 또는 tar.gz를 받아 동일한 매니페스트·아이콘도 함께 묶는다. 새 화면이면 `#google-login` 단추와 필수 슬롯을 검사해 `boot.mjs`, 로그인 모듈과 `config.json`을 더한다. 화면을 지정하지 않으면 기본 화면을 쓴다. 단추 글과 모양은 화면 담당의 HTML이 제공하며 스크립트는 상태와 disabled만 바꾼다. state가 없거나 출처가 다르면 `invalid`다. 기존 세션 길이 `SESSION_SECONDS`는 30일 그대로다.
- D: 완성된 기존 설치의 동일 `anywhere` 명령은 로그인 설정·VM·입구 소유권과 입력 파일 SHA-256을 확인한 뒤 서버 실행 파일·화면을 교체하고 기존 Cloud Run 서비스에 입구 코드를 재배포한다. 서버 DB와 채팅 디렉터리를 수정하는 명령은 실행하지 않는다. 중간에 실패하면 이전 화면과 새 서버가 잠시 섞일 수 있어 같은 명령으로 재시도해야 한다. 같은 서비스의 주소는 배포 전후 비교한다. [Cloud Run 서비스의 안정 주소](https://cloud.google.com/run/docs/resource-model).
- D는 가짜 공급자로만 확인했다. 실제 Linux VM 위의 DB 마이그레이션과 화면·대화 보존은 시험하지 않았으므로 실제 업그레이드 성공을 뜻하지 않는다.
