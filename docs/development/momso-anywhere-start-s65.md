정리본: [momso-anywhere-guide.md](momso-anywhere-guide.md)

# S65 시작 인증과 설치 제거

## 전체 화면 인증

본부 결정에 따라 Google OAuth 웹 클라이언트의 승인된 redirect URI에 **`https://anywhere.momso.yoga/start/`**를 추가한다. 시험 출처를 사용하면 그 출처의 `/start/` 전체 주소도 정확히 추가한다. JavaScript 출처는 경로 없이 등록한다. Google의 [클라이언트 웹 앱 문서](https://developers.google.com/identity/protocols/oauth2/javascript-implicit-flow)는 `response_type=token`의 전체 창 이동과 fragment 응답을 설명한다. 다만 Google은 이 직접 implicit 방식을 보안상 강하게 비권장하며 코드+PKCE를 권고한다. 여기서는 정적 페이지와 팝업 없는 본부 결정에 따라 사용한다. 진짜 Google·휴대폰에서 재검증해야 한다.

시작 단추는 무작위 state와 `openid`, `email`, `cloud-platform` 범위를 넣어 Google로 이동한다. 페이지의 맨 앞에서 `momso_anywhere_oauth_capture.mjs`가 fragment를 메모리에만 받고 주소에서 즉시 제거한다. 복귀 처리는 state·요청 출처·클라이언트·기한·범위·토큰 형식을 확인하고 토큰을 모듈과 공급자의 메모리에만 둔다. 화면에 토큰을 반환하거나 URL·로컬 저장소·로그에 넣지 않는다. 클라우드 요청 중 토큰이 만료되면 선택한 작업·프로젝트·결제 계정만 이 탭에 남기고 다시 로그인한다. 설치 번호는 기존 로컬 체크포인트로 이어진다. 기기·브라우저 확장·정적 사이트 공급망이 악성일 때 토큰 노출을 막는다는 뜻은 아니다.

## S65b: 인증 한 번으로 이어 쓰기

Google에서 돌아온 접근 토큰은 **그 페이지의 모듈 메모리에서만**, 받은 만료 시각까지 보관한다. 프로젝트·결제 목록, 설치, 찾기, 지우기는 같은 출처와 클라이언트에서 이 토큰을 다시 쓴다. 선택한 프로젝트나 결제 계정은 작업마다 공급자에 새로 전달한다. 페이지를 닫거나 새로 고치면 메모리도 사라진다. 토큰이 만료됐거나 Google이 인증 실패를 반환할 때만 진행할 작업과 선택을 탭 저장소에 남기고 다시 Google로 이동한다. 토큰·그 해시는 저장하지 않는다. `clearStartAuthorization()`으로 메모리를 명시적으로 비울 수도 있다.

화면의 정상 흐름은 기존 `resumeAfterRedirect`와 각 작업 함수의 반환값을 그대로 사용한다. **화면 담당이 바꿀 곳:** 오류 후 ‘다시 시도’는 구형 `retryFromGesture` 대신 `await retryLastAction({clientId, release, onProgress})`를 호출하고, 결과를 `resumeAfterRedirect` 결과와 같은 방식으로 표시한다. 반환값은 `{kind:'projects'|'billing'|'start'|'find'|'destroy', ...}` 또는 `{kind:'redirecting'}`이다. 구형 함수는 호환을 위해 남아 있지만 언제나 Google로 이동한다. 화면 스냅샷 `883b404`는 아직 구형 재시도를 쓰므로 이 화면 변경 전에는 오류 뒤 재시도에서 불필요한 재로그인이 생길 수 있다.

화면을 열 때마다 한 번 `resumeAfterRedirect({clientId, release, onProgress})`를 호출한다. 이어 갈 인증이 없으면 `null`이다. 복귀 결과는 목록이면 `{kind:'projects', projects, selection}` 또는 `{kind:'billing', billingAccounts, selection}`, 설치·찾기·삭제면 `{kind:'start'|'find'|'destroy', result, selection}`이다. `selection`은 `{projectId, billingAccountName}`이며 선택되지 않은 값은 `null`이다. 목록을 요청할 때도 현재 고른 값을 `projectsFromGesture` 또는 `billingAccountsFromGesture`에 넘겨야 복귀 후 유지된다. 시작·찾기·목록·삭제 함수가 Google로 이동할 때는 `{redirecting:true}`이므로 실패로 표시하지 않는다. 복귀 처리 중 토큰이 만료되면 같은 선택과 체크포인트로 Google에 다시 보내고 `{kind:'redirecting'}`을 반환한다. 묶음 스크립트는 빌드된 JavaScript에 복귀 함수 이름이 없으면 `screen_resume_missing`으로 거절한다. 이름이 있다는 것만으로 실제 연결 성공을 보장하지는 않는다. 캡처 모듈을 화면보다 먼저 싣는 것은 묶음 스크립트가 한다.

## 휴대폰 설치 제거

`destroyFromGesture({clientId, projectId, installationId})`는 인증 후 `resumeAfterRedirect`에서 `{kind:'destroy', result:{remaining}, selection}`을 돌려준다. 각 단계는 `{step,state}`이며 지연된 자원은 `state:'pending'`이다. `remaining`이 비어 있지 않으면 해당 프로젝트·설치 ID와 남은 이름을 로컬 저장소에 기록한다. 나중에 **같은 프로젝트와 설치 ID**로 다시 호출하면 남은 것만 조사한다. 화면은 삭제가 VM 자료와 보관함 열쇠를 지운다는 점을 분명히 보여 주어야 한다.

PC에서는 명령의 표준 입력으로만 토큰을 전달한다. `<...>`은 본부가 직접 채운다. 토큰을 파일·인자·기록에 적지 않는다.

```powershell
gcloud auth print-access-token --account=<계정> --project=<고객-프로젝트> | node scripts/momso_anywhere_destroy_cli.mjs --project <고객-프로젝트> --installation-id <24자리-설치-ID> --state <전용-상태파일.json>
```

상태 파일에는 프로젝트·설치 ID·남은 자원 이름만 쓴다. 이미 다른 설치의 상태 파일이면 덮지 않는다. 종료 코드 `0`은 남은 것이 없음, `2`는 네트워크 자원이 남아 같은 명령으로 나중에 재시도, `1`은 실패다. `subnet`·`network`가 남는 것은 Cloud Run 주소 회수 지연뿐 아니라 권한 등 다른 원인일 수도 있다. 이 명령은 Cloud Run, VM·자동 삭제 부팅 디스크, NAT·라우터·방화벽·서브넷·네트워크, 전용 비밀값·서비스 계정·역할을 지운다. **보관함 열쇠도 지우므로 저장된 로그인이나 그 복원에 열쇠가 필요하면 실행하지 않는다.** 프로젝트·결제 계정·별도 백업 버킷은 지우지 않는다.

삭제 전 모든 대상의 설치 표식과 VM 부팅 디스크·서비스 계정, 라우터의 다른 연결, 역할 권한을 확인하고 각 삭제 직전 다시 확인한다. 다른 설치나 소유 표식이 다른 자원을 발견하면 삭제를 시작하지 않는다. 실제 Google에는 아직 실행하지 않았으며 가짜 Google 시험은 실제 IAM 권한·비동기 삭제·동시 변경의 완전한 보장을 뜻하지 않는다.
