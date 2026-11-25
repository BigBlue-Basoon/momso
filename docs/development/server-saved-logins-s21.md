# momso server 저장된 로그인: S21 연결 계약

## S136 현재 상태

아래 S21의 “HTTP 미연결”과 “브라우저 연결 전”은 **당시 기록**이다. 현재 Linux server는 인증된 `POST`·`GET /saved-logins`, `PUT`·`DELETE /saved-logins/{id}`를 `browser_bridge`에 연결한다. 각각 `{login}`, `{logins}`, `{login}`, `{state:"deleted"}`를 돌려주며 비밀번호는 반환하지 않는다. 수정에서 `password` 생략은 유지, `loginPath:""`는 경로 해제다. `POST` 입력 오류는 `invalid_input`, 잘못된 ID 수정은 `login_not_found`; 보관소 오류는 고정 코드다. 목록 읽기의 보관소 오류는 HTTP 503, 저장·수정·삭제의 보관소 오류는 코드 경로에 따라 HTTP 400일 수 있다. 성공 응답은 `Cache-Control: no-store`다.

현재 응답에 `iconDataUrl` 필드는 **없다**. UI는 이 필드나 실제 사이트 로그인 성공을 가정하지 않는다. `phone_saved_login_is_encrypted_and_never_echoed`와 `requested_login_survives_reconnect_and_reaches_only_site`는 가짜 사이트·시험 비밀로 확인한다. 실제 휴대폰·계정은 이 시험의 범위가 아니다. 아래의 새 VM 키 이전 불가 서술도 S21 당시 기록이며, 현재 운영 복원 절차는 [anywhere 안내서](momso-anywhere-guide.md)를 따른다.

S24 화면 계약: 저장된 로그인 목록 항목에 선택적인 `iconDataUrl`(PNG data URL 또는 `null`) 자리를 둔다. 서버의 그림 수집·보관 및 필드는 아직 구현되지 않았으며, 구현 시에도 목록 조회만으로 사용자가 방문하지 않은 사이트에 접속하지 않는다. 그림이 없으면 화면은 이름 두 글자를 쓴다.

담당: 서버·브라우저 연결 담당. 이 문서는 독립 보관 부품의 계약이다. S17 HTTP 서버나 S20 브라우저에 연결된 기능이 아니며, 휴대폰에서 실제로 저장하거나 로그인하는 제품 흐름은 아직 검증하지 않았다.

## 보관과 신뢰

Linux 독립 부품 `apps/alpha/desktop/src-tauri/saved-login-core`는 각 저장 항목을 하나의 age 암호문으로 보관한다. 화면 판에 데스크톱 코드를 옮길 때 공통 인터페이스가 누락되지 않도록 이 위치에 두었으며, 서버는 별도 Cargo 패키지로 사용할 수 있다. 비밀번호·아이디·제목·사이트가 파일에 평문으로 저장되지 않는다. 파일은 서비스 전용 디렉터리의 `vault.age`이며 임시 파일도 암호문이다. 파일 갱신 중 잠금을 쓰고 암호문을 동기화한 뒤 교체한다. Windows는 기존 Windows 자격 증명 관리자와 기존 항목별 저장·이전 동작을 유지하고 같은 내부 `CredentialBackend` 인터페이스를 쓴다. 양쪽 모두 기존 데이터베이스와 채팅에는 비밀번호를 저장하지 않는다.

서비스 관리자는 별도로 만든 age 개인 키를 systemd의 `LoadCredentialEncrypted=momso-vault-identity:...`로 전달한다. `LinuxVault::from_systemd`는 systemd가 제공한 `$CREDENTIALS_DIRECTORY/momso-vault-identity`에서 읽는다. 개인 키는 명령 인자·일반 환경 변수·저장소·로그에 넣지 않는다. 운영 설치 담당은 systemd 암호화 자격 증명 파일과 서버 디렉터리를 관리하고 둘 다 GitHub·R2·채팅 백업 대상에서 빼야 한다. systemd는 암호화된 자격 증명을 서비스 시작 시 복호화해 해당 서비스의 자격 증명 디렉터리에 전달한다. [systemd.exec 공식 문서](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html), [systemd-creds 공식 문서](https://www.freedesktop.org/software/systemd/man/latest/systemd-creds.html), [age 공식 설명](https://github.com/FiloSottile/age).

이 구조는 호스트와 systemd의 키 보호, 서비스 계정 및 그 파일 권한을 믿는다. 같은 OS 사용자로 전체 권한을 가진 AI 프로세스가 스스로 파일이나 프로세스를 읽는 것까지는 막지 못한다. 기존 연결 정보와 같은 한계이며, 승인 요청 권한 단계에서 그런 명령은 사용자에게 보인다. 서비스 계정·제한된 자식 프로세스 분리, 잠금 메모리와 메모리 지우기는 후속 검증 대상이다. 저장소 암호문과 복호화용 호스트 키를 함께 백업하면 이 보호가 약해진다. 새 VM으로 키를 안전하게 옮기는 절차는 아직 없으며, 그렇지 않으면 사용자가 새 VM에 다시 입력한다.

## 휴대폰 입력 — 제안 계약, 아직 HTTP에 미연결

서버 담당은 사용자 인증과 TLS를 먼저 적용한 별도 비공개 API를 구현한다. S17의 임시 공용 토큰만으로 인터넷에 노출하지 않는다. 제안 요청 `POST /saved-logins`의 본문은 `{ "siteUrl": "https://example.org/login", "label": "센터", "username": "...", "password": "...", "askBeforeUse": false, "loginPath": "/login" }`이다. 비밀번호는 자리 표시이며 실제 값을 문서·기록에 넣지 않는다. 이 부품의 `PhoneSaveRequest`를 `LinuxVault::save_from_phone`에 넘긴다. 응답 `LoginInfo`에는 `id`, `siteUrl`, `origin`, `label`, `username`, `createdAt`, `updatedAt`, `favorite`, `askBeforeUse`, `loginPath`, `defaultForPath`만 있고 비밀번호는 없다. 기본은 저장 시 정한 자동 사용이며 `askBeforeUse`가 참이면 매번 사람의 승인을 요구한다. 저장 성공은 사이트 로그인 성공을 뜻하지 않는다.

목록은 `list`, 수정은 `update_from_phone(id, request, favorite)`, 삭제는 `delete(id)`다. HTTP의 인증·권한, 요청 크기와 오류 응답, 전송 경로의 본문 기록 비활성화는 서버 연결 시 반드시 구현한다. `PhoneSaveRequest`와 `BrowserLogin`은 `Debug`나 응답 직렬화를 제공하지 않는다. HTTP 본문, 오류 원문, 메모리 덤프, 브라우저 입력값을 기록하지 않는 것은 호출 측의 의무다. 원격 화면·API를 현재 부품이 열거나 인증하지 않는다.

항목에는 경로별 기본 계정 표시 `defaultForPath`도 있다. 계정 선택 동작은 서버와 브라우저 연결 시 구현해야 한다.

## Linux 브라우저에 넘기는 자리

호스트만 `browser_login(id, top_url, frame_url, approved)`를 부른다. AI가 제공한 주소가 아니라 실제 브라우저의 현재 최상위 문서와 실제 대상 프레임에서 읽은 주소를 넘기고, 채우기 직전에 다시 검사한다. 스킴·호스트·포트가 모두 저장된 출처와 같아야 한다. 다른 프레임·리디렉션·비슷한 호스트는 `origin_mismatch`다. `askBeforeUse`인 항목은 별도 승인 뒤에만 `approved=true`로 부르며, AI가 직접 이 값을 정하게 하면 안 된다. 반환 객체는 호스트 내부에서만 즉시 입력하고 MCP 응답·페이지 읽기·로그·채팅으로 내보내지 않는다. 브라우저 코드가 비밀번호 필드와 입력 이벤트의 값을 AI에게 노출하지 않는지, 입력 직전 화면과 프레임이 바뀌지 않았는지는 S20 연결 시험에서 별도로 지켜야 한다. 인증번호·캡차·추가 확인은 사람에게 넘긴다.

오류는 `invalid_site`, `origin_mismatch`, `login_not_found`, `approval_required`, `credential_unavailable`, `credential_invalid`, `storage_unavailable`, `storage_invalid` 같은 고정 값만 전달하고 파일 경로·입력값·내부 오류 원문을 넣지 않는다.

## 진행 기록

S21 독립 저장소와 Windows 어댑터를 작성했다. 서버 HTTP와 실제 브라우저 연결 및 그 경로의 비밀값 유출 검사는 각각 담당 작업에서 이어야 한다. 실제 계정은 사용하지 않는다.
