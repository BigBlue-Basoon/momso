# 데스크톱 GitHub 연결 통일: S110 작업 기록

- 목표: 기존 화면 명령과 이벤트를 유지하면서 기기 로그인과 저장소·Git 백업에 momso 앱의 토큰을 쓴다.
- 순서: 기기 로그인과 취소, Windows 자격 증명 저장과 만료 토큰 갱신, 저장소 및 Git 전송 연결, 가짜 서버를 이용한 전체 흐름 검증, Windows 검증, 통합.
- 갱신 성공 시 이전 토큰 쌍이 무효가 되므로 동시 갱신을 막고 새 쌍을 자격 증명 보관함에 저장한다. 응답이나 저장이 실패하면 연결 재시도를 안전하게 안내해야 한다.
- 화면에 전하는 기기 코드를 제외하고 접근 토큰과 갱신 토큰은 화면 응답, 명령줄, 기록, 채팅 사본에 싣지 않는다. 알려진 비밀값 가리기에도 새 토큰을 포함한다.
- GitHub 공식 문서: [기기 로그인과 만료 토큰 갱신](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#refreshing-an-access-token-with-a-refresh-token). 기기 흐름의 토큰 갱신에는 client secret이 필요하지 않다.
- 실제 GitHub 확인: 제품의 `github_oauth::begin`으로 기기 코드 발급을 시작하고 곧바로 폐기했다(`live_device_start_then_discard_without_approval`). 코드를 저장·출력하거나 승인하지 않았다. Tauri 화면에서 승인을 완료하는 실계정 시험은 하지 않았다.
- 가짜 흐름: `fake_github_device_login_and_refresh_never_put_tokens_in_urls`가 가짜 서버의 기기 로그인·만료·갱신, 격리된 가짜 보관함, 저장소 생성 함수와 로컬 Git 백업을 잇는다. 저장소 API의 응답은 가짜 함수로 제공하고 Git 원격은 로컬 저장소를 쓴다. `github_device_login_cancel_command_cancels_pending_request`는 화면 취소 명령을 검증한다.
- GitHub CLI는 로그인 정보를 소유하지 않는다. 저장소 API와 Git 자식 프로세스에만 보관함에서 읽은 `GH_TOKEN`을 환경 변수로 주고, 만료 전에 몸소가 갱신한다. [GitHub CLI 공식 안내](https://docs.github.com/en/github-cli/github-cli/authenticating-with-github-cli)는 환경 변수 토큰을 저장된 자격 증명보다 우선하며 디스크에 쓰지 않는다고 설명한다.
- 화면 계약: 기존 `provider_login_start({provider:"github"})`, `provider_login_cancel({provider:"github"})`, `provider-login-device`와 `provider-login-device-closed`, `provider-auth-changed` 및 `archive_remote_*` 명령을 유지한다. 기기 로그인 이벤트의 기존 `expiresInSeconds`에 GitHub가 준 유효 시간을 채운다. 화면에 접근 토큰이나 갱신 토큰은 보내지 않는다. `ghConfigSource`는 기존 `owned` 값을 유지한다.
- 갱신 응답을 GitHub가 보낸 직후 연결이 끊기거나 보관함 저장이 실패하면 이전 갱신 토큰은 이미 무효일 수 있다. 이 경우 조용히 이전 값으로 되돌릴 수 없으며 GitHub를 다시 연결해야 한다.
