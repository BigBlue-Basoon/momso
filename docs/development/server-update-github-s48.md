정리본: [momso-anywhere-guide.md](momso-anywhere-guide.md)

# S48: 실제 systemd 갱신과 GitHub 기기 로그인 연결

## VM 업데이트와 설치기 공통 경로

`systemd_host.py`의 `SystemdHost`는 S38 `Updater`에 systemd 서비스 중지·시작, 판을 가리키는 심볼릭 링크의 원자적 교체, 실제 자료 폴더 복사·복원과 HTTP 건강 검사를 제공한다. 교체 전 진행 작업 접수를 막는 `admission`은 줄3 S46의 실제 구현이 필요하다. S48 시험은 **별도 이름의 임시 systemd 서비스, 가짜 대기/입구, 가짜 서버·배포처·서명 키**를 사용하며 기존 VM 서비스와 실제 고객 데이터를 사용하지 않는다. server가 시작할 때 자체 DB 이행을 한다는 전제를 실서비스에 연결하기 전에 server 담당이 확인해야 한다.

`cloud_run_gateway.py`의 `CloudRunGateway`는 서명된 manifest에 있는 **지문 고정 이미지**로 무트래픽 revision을 배포하고 준비된 revision으로 교체한다. 실패 시 이전 revision과 트래픽 비율로 복귀한다. 계정 비밀을 명령줄에 넣지 않고 VM의 서비스 계정 인증을 사용한다. 테스트는 가짜 gcloud만 사용했다. Cloud Run은 [revision 트래픽과 되돌리기](https://docs.cloud.google.com/run/docs/rollouts-rollbacks-traffic-migration)를 제공하지만, 실제 고객 프로젝트에서 필요한 IAM·health·긴 연결은 아직 검증하지 않았다.

**S48 당시 기록:** S43 `e1afcb862`의 `GoogleAnywhere.upgrade_gateway`는 `--source`를 Cloud Build로 다시 빌드해 두 길이 합쳐지지 않았다. 이후 S72에서 PC 설치기는 이 어댑터를 호출하고 휴대폰은 같은 revision 전환 계약을 REST로 구현했다. S73에서 설치 경로에도 서명된 목록 확인을 연결했다. 현재 상태와 남은 신뢰 조건은 [S73](momso-anywhere-signed-install-s73.md)을 따른다. 부품만 합쳐서는 실제 자동 갱신이 켜지지 않는다.

서명 담당이 검토·생성한 manifest 원본을 개인 키로 직접 서명하는 예(키는 그 사람만 다루며 파일 경로·값은 공유하지 않는다):

```sh
openssl pkeyutl -sign -inkey "<본인 Ed25519 개인 키>" -rawin -in release.json -out release.sig
```

서명 대상은 [S38 계약](server-update-s38.md)의 manifest 원본 바이트이며, `bundleSha256`과 `gatewayImage` 지문을 그 전에 확정해야 한다. 공개 키를 설치 시 안전하게 고정하고 서명 파일과 manifest를 HTTPS로 배포하는 작업이 필요하다. 기존 Windows Tauri 서명 형식과 다른 Ed25519 계약이므로 Windows 개인 키를 임의로 읽거나 변환하지 않는다. [OpenSSL pkeyutl](https://docs.openssl.org/3.0/man1/openssl-pkeyutl/).

## 휴대폰의 GitHub 연결

우리 쪽에서 한 번 할 일은 GitHub의 OAuth 앱을 등록하고 **Enable Device Flow**를 켠 뒤 공개 `client_id`를 server 설치 설정 `MOMSO_GITHUB_CLIENT_ID`에 넣는 것이다. 앱을 이 작업에서 만들지 않았다. GitHub의 기기 흐름은 client secret을 요구하지 않는다. 제공자 승인 화면에서 누구의 앱인지 확인할 수 있도록 앱 이름·홈페이지와 개인정보 안내를 준비한다. GitHub는 기기 코드 사칭 위험을 안내하므로 고객에게 **몸소 화면에 표시된 코드만 직접 입력하고 다른 사람이 준 코드는 입력하지 않도록** 설명한다. [OAuth 앱 등록](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/creating-an-oauth-app), [기기 흐름](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow).

1. 인증된 휴대폰이 `POST /github/login`을 호출하고 `{sessionId}`를 자기 화면 메모리에 둔다. server는 GitHub에 기기 코드를 요청한다.
2. 휴대폰은 `GET /github/login`에 `X-Momso-Login-Session`을 붙여 `{provider:"github",state,url,code,expiresInSeconds}`를 받는다. 현재 줄3 초안은 **SSE 이벤트를 제공하지 않으므로 조회 방식**이다. `Cache-Control: no-store`를 지킨다.
3. 고객은 GitHub의 공식 `https://github.com/login/device`에서 직접 로그인하고 화면의 코드를 넣어 승인한다. server는 GitHub가 정한 `interval`을 지키며 결과를 조회하고, `slow_down`에서는 간격을 늘린다. 승인되면 암호화 보관소에 토큰을 저장하고 화면에 토큰을 주지 않는다. 이 OAuth 앱은 현재 `repo` 범위를 요청하므로 승인 화면의 권한이 특정 저장소 하나보다 넓다.
4. 취소는 `POST /github/login/cancel`에 같은 세션 헤더를 보낸다. 만료·거절 뒤 다시 시작할 수 있다. 앱 재시작 시 진행 중 기기 로그인은 잃을 수 있으므로 다시 시작한다. 로그인된 서버 접근 권한과 세션 ID가 둘 다 유출된 기기는 현재 구분하지 못한다.
5. 연결 뒤 server는 S35의 비공개 `momso-<사용자 이름>` 저장소 생성·선택 및 백업 작업자를 이어야 한다. 현재 `9a24aaaff`·`2872221d9`의 `/github/login`은 로그인 초안이며 저장소 선택·자동 백업·복구와 휴대폰 화면은 별도 연결이 필요하다. 실제 GitHub 계정·승인은 시험하지 않았다.

기존 S35 문서의 제안 경로 `/providers/github/login`보다 실제 줄3 초안 `/github/login`이 우선이다. 화면 담당은 기존 AI 로그인 카드의 값과 문구를 재사용하되 호출 경로와 SSE 미지원 차이를 반영한다. 오류 원문·기기 비밀 코드·토큰은 화면이나 AI·기록으로 내보내지 않는다.

## S48 검증 기록

`test_signed_release_real_systemd_start_and_rollback`은 개발 VM에서 임시 서비스 하나를 실제 `systemctl`로 켠다. 가짜 배포처의 서명된 새 서버·화면 묶음을 적용하고, 그다음 판의 건강 검사를 실패시켜 직전 서버 판과 시험 자료가 돌아온 것을 확인한다. 접수 차단과 Cloud Run은 시험용 가짜다. 전체 12개 시험 통과(2026-11-22, `e951e3656`). 실제 몸소 server의 DB 이행이나 실제 Cloud Run은 확인한 것이 아니다. 시험 종료 시 만든 systemd 유닛을 중지하고 등록을 해제한다.
