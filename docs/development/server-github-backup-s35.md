정리본: [momso-anywhere-guide.md](momso-anywhere-guide.md)

# S35: server의 GitHub 연결과 몸소 폴더 백업

## 범위와 진행

Linux용 독립 꾸러미 `momso-github-backup`, S29 `momso-core`, S21 `momso-saved-login-core`를 사용한다. server HTTP 라우터와 휴대폰 화면은 이 변경에 포함되지 않는다. 줄3이 아래 계약을 server에 연결해야 실제 휴대폰에서 동작한다. 실제 GitHub 계정에 접근하지 않았으며 기기 로그인과 원격 저장소는 가짜 서버·로컬 bare Git으로 시험했다.

| 단계 | 상태 |
| --- | --- |
| 기기 로그인·저장소 만들기/고르기 | 독립 모듈 구현, server 연결 필요 |
| 변경 감지 후 Git 저장·업로드, 주기 재시도 | 독립 작업자 구현, server 시작·종료 및 변경 알림 연결 필요 |
| 새 VM에 Git 복원 | 빈 폴더 또는 없는 경로에 복제 구현, server 초기화 연결 필요 |
| 채팅 사본 | 데스크톱 형식과 가림 검사 후 Git에 보관. 사본 생성 및 DB에 다시 넣기는 server 측 연결 필요 |
| DB·원자료·로그인 보관소 백업 | 이 모듈 범위 밖. S30의 암호화 백업 사용 |

## 로그인 카드 및 server 끼움새

S17 AI 연결 카드와 같은 뜻으로, 인증된 사용자 세션에서만 아래 예시 HTTP 요청을 제공한다. 라우트는 제안이며 아직 구현된 server API가 아니다. 모든 응답은 `Cache-Control: no-store`로 처리한다.

1. `POST /providers/github/login` → `{"sessionId":"…"}`: `Github::start_device_login`을 부른다.
2. `GET /providers/github/login` (`X-Momso-Login-Session: …`) → `{"provider":"github","state":"pending","url":"https://github.com/login/device","code":"…","expiresInSeconds":900}`. `Github::login_status`를 사용한다. 코드는 선생님이 GitHub 화면에 입력하는 기기 코드이며 access token과 다르다.
3. server가 `poll_login`을 응답이 정한 간격으로 뒤에서 호출하고 기존 카드 이벤트 `provider-login-device`, 종료 이벤트 `provider-login-device-closed`에 같은 상태를 실어 보낸다. `POST /providers/github/login/cancel`은 `cancel_login`을 부른다. 기기 비밀 코드와 access token은 화면이나 AI로 보내지 않는다.
4. 로그인 성공 후 `account_name`, `list_private_repositories`, `create_or_find_repository(사용자 이름)` 또는 `inspect_repository(선택한 owner/name)`를 호출한다. 만들어지는 저장소 이름은 `momso-<사용자 이름>`이고 비공개다. 기존 저장소는 지우거나 덮지 않는다. HTTP 계정·저장소 목록과 선택 요청은 인증된 사용자에게만 노출한다.

제품에서 사용할 GitHub OAuth 앱의 **client ID**를 server 설정으로 제공하고 해당 앱의 기기 흐름을 켜야 한다. 이 저장소는 앱 등록을 대신하지 않는다. `repo` 권한으로 비공개 저장소에 접근하므로 이 OAuth 앱은 선택한 저장소 하나보다 넓은 범위를 가진다. 더 좁은 권한의 GitHub App 전환은 후속 선택지다. 기기 로그인은 [GitHub 공식 기기 흐름](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow), 저장소 생성은 [공식 REST API](https://docs.github.com/en/rest/repos/repos#create-a-repository-for-the-authenticated-user)를 따른다.

## 저장·복원과 상태

- server는 선택한 비밀이 아닌 저장소 이름과 기본 브랜치를 서버 설정에 보관하고 재시작 시 `Repository`를 다시 확인한다. 토큰을 그 설정에 쓰지 않는다.
- `SyncWorker::start(github, root, repository, interval)`을 시작하고 파일 변경 후 `changed()`를 호출한다. 주기적으로도 다시 시도한다. 종료 시 `stop()`을 부른다. 작업 중·완료·오류는 `status()`의 `state` (`idle`, `syncing`, `saved`, `saved_with_exclusions`, `failed`), `reason`, `excludedNonText`, `uploadedKeyFound`로 화면에 알려 줄 수 있다. 이벤트 발행은 server가 맡는다.
- 변경된 텍스트 기록을 로컬 Git에 저장하고 현재 저장소가 비공개인지 재확인한 뒤 업로드한다. 사람이 읽을 수 없는 바이너리는 원자료 저장소에 맡기고 `excludedNonText`로 센다. DB·로그·보관소·`.momso`와 원자료 `objects`는 Git 대상에서 제외한다. 변경 알림을 놓쳐도 주기 검사로 다시 찾는다.
- `restore(destination, repository)`는 없는 폴더 또는 빈 폴더만 받는다. 기존 자료를 덮지 않는다. 복원 뒤 server가 채팅 사본을 DB에 반영하고 색인을 갱신해야 한다. 서로 다른 PC에서 Git 이력이 갈라지면 `diverged`로 멈추며 강제 업로드하지 않는다.
- 문서·사전·스킬 쓰기와 Git 작업은 몸소 폴더의 `.momso/core-write.lock`을 함께 쓴다. 잠금은 **이 core를 사용하는** 별도 프로세스 사이에서만 적용된다. 외부 편집기나 잠금을 지키지 않는 직접 Git 작업까지 막지는 못한다.

## 비밀값과 알려 둔 것

GitHub 토큰과 저장된 사이트 비밀번호는 S21의 암호화 보관소에만 저장하고, 보관소는 몸소 폴더 밖에 두어야 한다. Git 연결은 URL·인자·설정 파일에 토큰을 넣지 않고 자식 Git에만 환경 변수로 전달한다. 요청·오류·이벤트·기록에 토큰이나 원문을 싣지 않는다. 같은 OS 사용자 권한의 다른 프로세스가 Git의 환경이나 보관소를 읽는 것까지는 이 구조가 막지 못한다. 서버 계정을 다른 사용자와 분리하고 VM 접근 권한을 관리해야 한다.

변경된 파일과 아직 올라가지 않은 Git 이력에 현재 아는 비밀값이 있으면 `credential_in_archive` 또는 `credential_in_history`로 전송을 막는다. 안전한 사본이나 커밋으로 고친 뒤 다시 시도해야 한다. 이미 올라간 이력은 다시 쓰지 않는다. 그 이력에서 지금 아는 비밀값이 발견되면 전송을 계속하되 `uploadedKeyFound: true`로 알려 그 서비스에서 키를 교체할 수 있게 한다. 과거에 쓰고 보관소에서 지워진 값, 형식이 변형된 값, 몸소가 모르는 값까지 모두 알아보는 보장은 없다. 아주 짧은 비밀번호는 일반 글과 우연히 일치해 검사가 멈출 수 있다.

채팅 사본은 `chats/chat_<id>.json`의 데스크톱 형식이어야 한다. 현재 저장된 비밀번호 또는 알려진 비밀값 형식이 남아 있으면 `chat_redaction_required`를 돌려주고 업로드하지 않는다. 이 모듈은 원문을 임의로 바꾸지 않는다. server는 가려진 사본을 만들어 다시 시도해야 한다. 서버의 인증·요청 제한, 기존 AI 세션에서 비밀값을 직접 읽지 못하게 하는 OS 권한 분리는 별도 server 통합 과제다.
