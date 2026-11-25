정리본: [momso-anywhere-guide.md](momso-anywhere-guide.md)

# S38: momso anywhere 업데이트 부품과 연결 계약

## 지금 된 것과 아직 필요한 것

`apps/alpha/desktop/src-tauri/momso-server-update/update.py`는 server와 설치기에 기대지 않는 Linux 업데이트 조정 부품이다. 가짜 배포처·가짜 server/입구로 검증한다. **실제 server의 작업 중지·자료 스냅샷·systemd 교체, 고객 Cloud Run 입구 배포 어댑터와 공개 서명 키·배포처는 아직 연결되지 않았다.** 따라서 이 커밋만으로 실제 고객 VM이 스스로 업데이트되지는 않는다.

S26의 VM 시작 스크립트는 부팅 때 설치 당시의 URL과 지문으로 `/opt/momso/momso-server`, proxy와 `web`을 다시 교체한다. 새 설치 담당은 초기 설치 이후에도 오래된 묶음을 덮어쓰지 않도록 변경해야 한다. 이 부분을 고치고 기존 VM을 새 구조로 안전하게 옮기기 전에는 업데이트 부품을 실제 서비스에 연결하지 않는다. server와 설치 스크립트는 각 담당의 파일이므로 여기서는 바꾸지 않았다.

## 서명된 판과 공개

릴리스 담당이 서명한 UTF-8 JSON 원본과 별도 Ed25519 서명 파일을 HTTPS 공개 배포처에 둔다. 설치 때 검증한 공개 키를 VM에 고정하며, 개인 키는 개발자만 가진다. AI는 개인 키를 읽거나 서명하지 않는다. 데스크톱 Tauri의 `.sig`/서명 형식과 S38의 Ed25519 원시 서명은 서로 다른 계약이므로 같은 키를 임의로 변환하거나 재사용하지 않는다. 실제 공개 전 제품 담당이 공개 키 배포·키 교체·폐기·서명 절차와 배포 주소를 정해야 한다. 시험은 그 자리에서 생성한 가짜 키만 사용한다. OpenSSL 3의 `pkeyutl -verify -pubin -rawin`으로 원본 바이트를 검증한다. [OpenSSL 공식 문서](https://docs.openssl.org/3.0/man1/openssl-pkeyutl/).

```json
{"sequence":2,"version":"0.1.2","bundleUrl":"https://release.example/momso.tar.gz","bundleSha256":"<64자리 소문자 SHA-256>","gatewayImage":"registry.example/momso@sha256:<64자리 소문자 SHA-256>"}
```

부품은 서명을 먼저 검증한 뒤 증가하는 `sequence`와 묶음의 SHA-256을 확인한다. 서명 안의 지문으로 서버·화면·VM proxy 묶음과 입구 이미지 지문을 고정한다. 검증 실패, 오래된 판, 변조된 묶음, 잘못된 압축 파일은 적용하지 않는다. 압축은 일반 파일·폴더만 받고 링크와 경로 이탈을 거절한다. 다운로드와 압축 해제 크기에 상한이 있다. HTTPS 요청의 실패는 오류로 알리며 개인 값이나 원문을 기록하지 않는다. 시간에 따른 만료·서명 키 교체와 손상된 최신 정보의 복구는 추가 설계가 필요하다.

## host 연결

`Updater(root, public_key, host).install(manifest_url, signature_url)`을 주기적으로 호출한다. 작업자가 별도 프로세스라면 server가 꺼지는 동안에도 계속 실행돼야 한다. 동시 업데이트는 `update.lock`으로 막는다. `check()`만 호출하면 교체 없이 새 판 정보를 검증한다. `recover()`는 시작할 때 사용자 요청을 받기 전에 호출한다. 업데이트 도중 전원이 끊겨 기록이 `switching`이면 옛 판·자료·입구 복구를 우선한다. 이 부품은 server 실행 중인 프로세스 안에서 자기 자신을 종료하는 방식으로 연결하면 안 된다.

host가 다음을 구현해야 한다. 시험의 host는 가짜다.

| host 함수 | 반드시 지켜야 할 뜻 |
| --- | --- |
| `current_sequence`, `record_sequence`, `current_release`, `activate_release` | 설치된 순번을 영구 저장하고, 기존 파일을 보존한 채 server·proxy·화면을 한 판으로 전환한다. |
| `acquire_maintenance`, `release_maintenance` | 새 요청·작업 접수를 원자적으로 막은 뒤 기존 턴·승인·입력·쓰기·Git 및 오브젝트 백업을 모두 마칠 때까지 기다리고, 복구나 정상 확인이 끝날 때만 다시 연다. S32의 `/activity` 조회 한 번은 관찰 시점의 값이므로 이것만으로 충분하지 않다. |
| `stop_server`, `start_server`, `server_healthy` | 실제 systemd 서비스와 사설 proxy를 함께 다루고, 새 판의 준비 및 자료 판올림 성공을 확인한다. |
| `snapshot_data`, `migrate_data`, `restore_data` | 쓰기가 멈춘 뒤 DB·로그인 보관소·몸소 폴더를 일관되게 보관하고, 판올림 실패 시 전체를 이전 상태로 복원한다. 임시 사본은 server 전용 계정만 읽고 비밀값을 기록하지 않으며, 백업 완료·보관/삭제 정책은 설치 담당이 구현한다. 키 자체를 새로 만들거나 외부에 보내지 않는다. |
| `current_gateway`, `activate_gateway`, `gateway_healthy` | 고객 프로젝트에서 서명된 지문의 Cloud Run 입구를 배포하고, 실패하면 이전 revision으로 되돌린다. 실제 Google API·권한과 인증은 아직 구현되지 않았다. |
| `progress` | `{state,reason}`을 인증된 휴대폰에 전달한다. `checking`, `up_to_date`, `downloading`, `waiting_for_idle`, `backing_up`, `installing`, `updating_gateway`, `completed`, `rolling_back`, `rolled_back`, `recovery_required`, `failed`가 있다. `reason`은 비밀·경로 없는 오류 이름 또는 null이다. server 재시작 동안에도 입구가 마지막 상태를 제공해야 한다. |

데이터 이전과 새 버전의 건강 검사를 끝내기 전에는 외부 쓰기를 다시 열지 않는다. 되돌리기 자체가 실패하면 `recovery_required`로 남기고 접수를 열지 않는다. 재기동 때 `recover()`가 저장된 이전 판 정보를 읽어 다시 복구한다. 입구의 이전 revision으로 트래픽을 돌리는 것은 Cloud Run이 지원하지만, 트래픽 전환은 즉각적이지 않고 되돌릴 때 외부 의존성 상태를 자동으로 재검증하지 않는다. host가 준비 검사를 해야 한다. [Cloud Run 공식 문서](https://docs.cloud.google.com/run/docs/rollouts-rollbacks-traffic-migration).

## 검증과 남은 위험

로컬 HTTP 가짜 배포처에서 직접 만든 서명 키를 사용한다. 정상 교체·이미 최신·잘못된 서명과 지문·작업 중 대기·건강 검사 실패 후 옛 server/입구/자료 복원·중단 뒤 복구를 확인한다. 실제 서명 산출물, systemd, Google Cloud 배포, 데이터베이스 백업과 고객 기기에서는 시험하지 않았다. `data_snapshots.py`는 쓰기를 멈춘 하나의 자료 폴더를 전용 위치에 복사하고 이전 사본을 남긴 채 되돌리는 보조 부품이다. DB·보관소·몸소 폴더가 서로 다른 위치면 host가 각각 묶고 일관성을 보장해야 한다. 시험의 가짜 host와 이 복사 방식은 실제 운영 디스크의 정전 시 내구성·암호화·공간 부족·권한 문제를 증명하지 않는다. 복구 전 자료와 스냅샷은 안전 확인 후 정리할 수 있도록 남는다. 건강 검사가 실제 로그인·대화·백업 호환성을 보증하는지도 통합 시험이 필요하다.
