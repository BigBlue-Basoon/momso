정리본: [momso-anywhere-guide.md](momso-anywhere-guide.md)

# 휴대폰에서 momso server 연결

확인일: 2026-11-21. 첫 시험의 권고는 **고객 Cloudflare 계정의 Tunnel + Access**다. VM에 공인 IP를 열지 않고, 휴대폰은 브라우저에서 허용된 계정으로 로그인한다. Cloudflare는 고객이 선택한 외부 중계·인증 사업자이며, 우리가 고객 서버를 대신 운영하는 것은 아니다. 접속 트래픽은 Cloudflare를 지나므로 개인정보와 사업자 의존성을 고객에게 알려야 한다. [공식 웹 앱 안내](https://developers.cloudflare.com/cloudflare-one/setup/secure-private-apps/private-web-app/), [Tunnel 설명](https://developers.cloudflare.com/tunnel/).

## 후보 비교

| 방법 | 추가 계정·요금 | 사람이 할 일 / 몸소가 할 일 | 휴대폰 앱 | 꺼진 VM / 위험 |
| --- | --- | --- | --- | --- |
| Cloudflare Tunnel + Access | Cloudflare 계정·Zero Trust 조직·Cloudflare에 연결된 도메인. Free는 최대 50명 무료이나 무료 조직 생성에도 결제 정보 입력이 필요하다. 도메인·VM·NAT는 별도. [요금](https://www.cloudflare.com/plans/zero-trust-services/), [조직 만들기](https://developers.cloudflare.com/cloudflare-one/setup/) | 사람이 가입·결제 정보·로그인을 승인한다. 몸소가 터널·Access·DNS를 구성할 수 있다. 지금 시험 코드는 권한이 제한된 API 토큰을 본부가 환경 변수로 제공해야 한다. | 불필요 | VM이 꺼지면 안의 커넥터도 꺼져 스스로 켜지 못한다. Cloudflare 정책·계정·도메인에 의존하며, 잘못 구성하면 공개될 수 있다. |
| Google IAP 웹 + HTTPS 로드 밸런서 | 결제된 Google Cloud 프로젝트와 로그인할 Google 계정. IAP 기본 기능은 추가 요금이 없지만 로드 밸런서·네트워크·VM은 별도다. [IAP 요금](https://cloud.google.com/iap/pricing), [웹 IAP 전제](https://docs.cloud.google.com/iap/docs/enabling-compute-howto) | 사람이 Google 로그인·권한·결제를 승인한다. 몸소가 로드 밸런서·인증 정책·DNS를 만들 수 있다. | 불필요 | IAP만으로 꺼진 VM은 켜지지 않는다. HTTPS 로드 밸런서와 도메인·인증서가 필요하고 비용과 구성이 늘어난다. 로드 밸런서를 우회하는 경로도 막아야 한다. |
| Tailscale Serve 사설망 | Tailscale 계정과 기기 인증. Personal은 최대 6명 무료다. [요금](https://tailscale.com/pricing), [Serve](https://tailscale.com/docs/features/tailscale-serve) | 사람이 계정과 기기를 승인한다. 몸소가 VM 연결과 접근 규칙을 설정할 수 있다. | 일반 사설망 접속에는 필요 | VM이 꺼지면 기기 자체가 응답하지 않는다. 기기 설치가 첫 목표와 다르다. Funnel은 앱 없이 접속할 수 있지만 공개 경로이므로 별도 인증이 필수다. [Funnel](https://tailscale.com/docs/features/tailscale-funnel) |
| 직접 HTTPS + 자체 인증 | 도메인과 VM·공인 접속 경로가 필요하다. Let's Encrypt 인증서는 무료이나 도메인·VM·트래픽 비용은 별도다. [인증서](https://letsencrypt.org/docs/faq/) | 사람이 도메인·결제·로그인을 승인한다. 몸소가 인증서 발급과 갱신을 자동화할 수 있지만 자체 인증·보안 갱신·복구까지 책임져야 한다. | 불필요 | 꺼진 VM은 직접 응답하지 않는다. 공인 진입점, 인증 구현 오류와 인증서 갱신 실패 위험이 있다. |

위 금액·조건은 확인일 기준이며 서비스와 지역에 따라 바뀔 수 있다. 새 Cloudflare Zero Trust 조직은 2026-06-18부터 Cloudflare 계정이 기본 로그인 방식이다. 이메일 일회용 PIN은 자동으로 추가되지 않으므로 따로 설정하거나 이미 연결된 신원 공급자를 써야 한다. [공식 변경 공지](https://developers.cloudflare.com/changelog/post/2026-06-18-cloudflare-idp-default/).

## 첫 시험 구성

S18의 `mobile` 명령은 VM 설치 뒤 고객 계정에 단일 이메일 허용 Access 앱, 전용 터널과 DNS를 구성한다. 기존 DNS나 다른 Access 앱이 같은 주소를 사용하면 건드리지 않는다. 터널 설정의 `access.required`, 조직과 앱 audience를 다시 읽어 확인한 다음에만 VM의 터널을 시작한다. 인증 없는 공개 `/health`가 응답하면 터널을 중지한다. [Cloudflare의 터널 측 Access 검증](https://developers.cloudflare.com/tunnel/reference/origin-parameters/).

VM에서는 검증된 [cloudflared 2026.10.6](https://github.com/cloudflare/cloudflared/releases/tag/2026.10.6)을 고정 SHA-256으로 확인한다. Access를 통과한 요청은 전용 Unix 소켓을 통해 로컬 프록시로 간다. 프록시는 서버 토큰을 브라우저에 주지 않고 systemd의 별도 자격 증명 파일에서 읽어 `127.0.0.1:8765`에 전달한다. 요청의 호스트와 쓰기 요청의 Origin을 확인하고 응답 캐시를 금지한다. 터널 토큰은 IAP SSH 표준 입력으로만 전달하며 파일 권한을 제한한다. [Unix 소켓 라우팅](https://developers.cloudflare.com/tunnel/concepts/routing/), [systemd 자격 증명](https://www.freedesktop.org/software/systemd/man/252/systemd.exec.html), [터널 토큰](https://developers.cloudflare.com/tunnel/reference/tunnel-tokens/).

Linux x86-64 파일의 고정 SHA-256: `1d356dac769d0dbf62ebe588c4034824bbff98d5790c2f844cc99e0e2f57462d`.

먼저 고객이 Cloudflare 로그인과 Free 조직·도메인 연결을 승인하고, 본부가 해당 계정의 최소 권한 API 토큰을 `MOMSO_CLOUDFLARE_API_TOKEN` 환경 변수에 안전하게 넣는다. 토큰을 명령줄 인자·기록 파일·채팅에 적지 않는다. 토큰에는 Access 앱·정책, 터널, 해당 영역의 DNS를 읽고 쓰는 권한이 필요하다. 이 토큰을 일반 고객에게 직접 만들게 하는 것은 최종 사용 흐름이 아니며, 향후 몸소의 승인된 계정 연결이 대신해야 한다.

PowerShell에서 저장소 루트 기준, 환경 변수를 준비한 뒤 한 줄:

```powershell
python scripts/momso_server_install.py mobile --account "<Google 계정>" --project "<프로젝트 ID>" --state "<설치 기록 경로>" --artifact "<Linux momso-server 파일>" --sha256 "<서버 파일 SHA-256>" --cf-account "<Cloudflare 계정 ID>" --cf-zone "<도메인 영역 ID>" --hostname "<사용하지 않는 접속 주소>" --email "<허용할 로그인 이메일>"
```

같은 기록과 인자로 다시 실행하면 이미 만든 전용 자원을 확인해 이어 간다. 사용 후 환경 변수의 토큰을 지운다. 첫 사용자 확인은 휴대폰에서 Access 로그인 후 채팅 화면이 뜨고 말을 한 번 보내 답이 오는 것까지다. `/health`는 보조 점검이다. 실제 계정·휴대폰·Linux 실행 파일 시험은 아직 하지 않았다. 프록시는 일반 JSON 및 SSE를 전달하며 chunked 요청 본문과 WebSocket은 아직 지원하지 않는다.

`destroy`는 같은 `--cf-account`, `--cf-zone`, `--hostname`, `--email`과 토큰을 다시 받아 설치 기록의 ID 및 표식을 확인한다. DNS를 먼저 없애고 VM을 제거한 후 터널 연결·터널·해당 앱 전용 정책과 앱을 제거한다. 정책이 추가되었거나 소유권이 다르면 건드리지 않는다. S19 당시 기록에 정책 ID가 없다면 같은 `mobile` 명령을 다시 실행해 정책을 확인·기록하고 삭제한다. 삭제 작업도 같은 인자로 재시도할 수 있다.

## 기기 인증과 깨우기 설계

현재 S17의 고정 서버 토큰은 VM 내부 프록시와 서버 사이에만 쓴다. 다음 단계에서는 Access 로그인 이후 짧은 수명의 일회성 짝짓기, 기기별 공개키 또는 WebAuthn, 만료·폐기·분실 기기 제거와 재인증을 서버에 설계한다. 이 문서는 그 기능을 구현하지 않는다. [WebAuthn 명세](https://www.w3.org/TR/webauthn/).

## S19b: 같은 출처 화면과 깨우기

서버가 화면 파일도 같은 출처에서 제공한다. 화면 담당 계약: 브라우저는 현재 출처의 상대 경로로 화면 파일·API·SSE를 요청하고, 토큰 입력 화면이나 `Authorization` 값을 요구하지 않는다. 스냅샷 `2d44336`의 `server-client.ts`는 현재 토큰을 sessionStorage에 저장하고 요청에 붙이므로 이 연결 방식에 맞게 바꿔야 한다. 이 작업은 화면을 수정하지 않았다. Cloudflare Access 로그인은 앞단에서 처리하고 프록시가 내부 토큰을 붙인다. 로컬 가짜 서버로 화면 HTML·JS·일반 요청·시간 간격을 둔 SSE를 확인했지만 몇 분짜리 실네트워크 연결이나 실제 채팅은 검증하지 않았다.

위 `mobile` 명령에 `--wake-user "<Google 로그인 이메일>"`을 더하면 고객 프로젝트에 Cloud Run 깨우기 서비스를 만든다. 고객이 Google IAP에 로그인한 뒤 단추를 누르면 서비스의 전용 계정이 지정된 VM의 상태 확인과 시작만 한다. 전용 사용자 정의 역할을 해당 VM에만 붙인다. 휴대폰에 클라우드 키를 주지 않는다. 출력되는 `run.app` 주소를 보관하고, VM이 켜질 때까지 기다린 뒤 기존 Cloudflare 주소로 들어간다. 현재 자동 준비 확인·자동 이동은 없다. `destroy`는 표식이 맞는 깨우기 서비스·서비스 계정·역할도 제거한다. Cloud Build와 Artifact Registry가 만든 공유 자원은 자동 삭제하지 않는다.

프로젝트 결제와 Cloud Run·Cloud Build·Artifact Registry·IAM·Compute·IAP API 및 권한이 필요하다. 조직이 없는 개인 프로젝트에서 IAP를 처음 설정할 때 Google은 콘솔의 최초 설정이나 별도 OAuth 클라이언트가 필요할 수 있다고 명시한다. 자동 명령이 그 단계를 대신했다고 가정하지 않는다. Cloud Run과 VM 디스크·NAT 등은 꺼진 VM과 별도로 요금이 생길 수 있어 **안 쓸 때 비용 0원은 보장하지 않는다**. [Cloud Run IAP 설정](https://docs.cloud.google.com/run/docs/securing/identity-aware-proxy-cloud-run), [소스 배포](https://docs.cloud.google.com/run/docs/deploying-source-code), [VM 시작](https://docs.cloud.google.com/compute/docs/instances/stop-start-instance), [Cloud Run 요금](https://cloud.google.com/run/pricing).

서버 담당 계약: 유휴 시간의 기준을 사용자가 정할 수 있어야 하고, 진행 중인 채팅·파일 쓰기·백업이 없으며 새 요청이 없는 것을 확인한 뒤 안전하게 저장하고 VM 종료를 요청해야 한다. 자동 종료나 과금 없는 상태 판정은 구현하지 않았다. 종료 권한과 다시 켠 뒤 준비 상태·실패 안내도 서버 쪽에서 설계해야 한다.

**도메인 없이:** Cloudflare Quick Tunnel은 자기 도메인 없이 임시 `trycloudflare.com` 주소를 제공하며 이메일 인증도 지원하지만 공식 안내는 시험·개발용이라고 한다. 일반적인 지속형 Tunnel 공개에는 Cloudflare에 연결된 도메인이 필요하다. Cloud Run 기본 `run.app` 주소에 IAP를 직접 붙이는 것은 도메인 없이 가능하지만 사설 VM까지 안전하게 프록시하는 별도 구성이 필요하며 아직 구현하지 않았다. 현재 선택한 Cloudflare 경로에서 가장 작은 부담은 도메인 한 개를 등록하거나 이미 가진 도메인의 하위 주소를 쓰는 것이다. [Quick Tunnel](https://developers.cloudflare.com/tunnel/get-started/quick-tunnels/), [Tunnel 전제](https://developers.cloudflare.com/tunnel/get-started/), [Cloud Run IAP](https://docs.cloud.google.com/run/docs/securing/identity-aware-proxy-cloud-run).
