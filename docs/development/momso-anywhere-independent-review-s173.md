# anywhere 독립 검토 S173

- 기준: `3d6058a3a31e634a032a4bd3c5aee2da301c6e3c` (`hq/s165-done` 없음 확인). 통합 가지가 이후 바뀔 수 있으므로 병합 전에 다시 대조한다.
- 범위: PC 설치·갱신, VM 자동 업데이트, 휴대폰 시작·삭제와 묶음·유휴 종료. 실제 고객 계정과 VM은 변경하지 않는다.
- 절차: 경로와 반복 정의를 대조하고, 실제 사용 경로를 확인하는지 살핀다. 확인된 사실과 의심을 구분하고, 다섯 기존 결함은 격리 사본의 변이 시험으로 시험의 방어력을 확인한다.
- 상태: 아래 확인과 격리 시험을 마쳤다. 설계가 필요한 미해결 건은 담당을 적었다. 가짜 서비스의 결과는 실제 고객 VM에서의 검증이 아니다.

## 발견과 처리

| 구분·심각도 | 사실과 조건 | 조치·담당 |
|---|---|---|
| 라, 접속 불가 | `momso-server-update/server_admission.py:41-69`는 접수 중지 POST의 응답이 유실되면 자기가 잠금을 가진 상태를 유지한다. 그러나 `update.py:242-246`은 예외를 돌려주고, 복구 기록은 그 뒤 `update.py:272`에서야 쓴다. 기록이 없는 상태에서 `update.py:355`의 복구는 할 일이 없다고 보고 다음 획득은 `busy`가 될 수 있다. 가짜 호스트에서 이 순서를 재현했다. 접수 중지 성공 후 기록 작성 전 해제에 실패할 때도 같은 복구 기록 공백이 있다. | **남음, 줄3·줄1.** POST 이전에 소유권을 증명할 지속 기록과 재시작 복구를 설계·시험해야 한다. 다른 작업이 소유한 접수 중지는 임의로 풀면 안 된다. 고객 VM에서 발생했다고 확인한 것은 아니다. |
| 마, 요금 샘 | 휴대폰의 `scripts/momso_anywhere_google_browser.mjs:903-947`은 서브넷·네트워크의 일부 오류만 남은 목록에 넣고, 라우터 등 다른 오류는 예외를 던져 뒤의 입구 등 정리를 건너뛴다. `scripts/momso_anywhere_start.mjs:238-239`에서 인프라 삭제가 예외를 내면 백업 삭제도 호출하지 않는다. 가짜 라우터 삭제 오류에서 VM 삭제 뒤 입구가 남는 것을 확인했다. | **남음, 줄2.** 소유권 미확인과 VM 삭제 미확인은 계속 중단해야 하므로, 독립 자원별 실패·종속성·잔여 목록을 나누는 삭제 설계와 비용 검증이 필요하다. 여기서 무조건 삭제를 계속하도록 바꾸지 않았다. |
| 라, 요금 샘·접속 불가 | 기준의 `momso-idle-stop/idle_stop.py:148-170`은 중지 요청 전후 예외 뒤 같은 부팅의 표식이 남으면 `stop_pending`으로 되풀이한다. 접수가 닫혔거나 VM이 계속 켜져 있을 수 있다. | **기준에 남음, 줄1.** 별도 `hq/s162-done`에 재확인·해제·재시도가 있다. 그 판의 실제 시험에서 옛 표식 동작을 되살리면 실패함을 리눅스 VM에서 확인했다. 통합 후 최신 끝에서 재검증할 것. |
| 가, 그 밖(업데이트 중단) | 기준의 `momso-server-update/managed_helpers.py:13-16` 작업자 파일 목록에서 `trust_rotation.py`가 빠졌지만 `update.py:15`, `github_releases.py:14`, `worker.py:15`는 이를 import한다. 기존 상위 폴더에 남은 오래된 파일에 의존할 수 있다. | **고침.** 파일을 버전별 작업자에 포함하고 관리 지문을 갱신했다. 실제 모듈 import 시험과 목록·import 관계 시험을 추가했다. |
| 나, 그 밖(브라우저 사용 불가 의심) | 휴대폰의 Debian 새 VM 시작 목록은 브라우저를 설치하지 않았지만 `apps/alpha/server/src/browser_bridge.rs:245-253`은 Chrome·Chromium 실행 파일을 필요로 한다. 이미지에 브라우저가 없는 경우 실패한다. PC 설치는 `scripts/momso_server_install.py:261-264`에서 확인한다. | **고침.** `scripts/momso_anywhere_vm_startup.mjs:30-34`에서 네 실행 파일을 확인하고 없을 때만 Chromium을 설치한다. 가짜 패키지 목록 시험을 추가했다. 실제 새 Google VM에서의 설치는 미시험. |
| 마, 그 밖(디스크 고갈) | `momso-server-update/update.py:242-268`에서 입구·현재 판 확인 또는 접수 해제 실패 시 임시 묶음이 남았고, 성공적으로 이전 판으로 되돌린 뒤에도 실패한 묶음이 남았다. 반복 실패하면 VM 디스크를 차지한다. | **고침.** 작업 접수 해제가 실패하더라도 아직 적용하지 않은 임시 묶음을 지운다. 이전 판 복구가 성공한 경우에도 실패한 묶음을 지우고, 지우기 오류는 `stage_cleanup_failed`로 기록한다. 복구 실패 시에는 원인 조사·복구용 사본을 보존한다. |
| 다, 접속 불가 가능 | `update.py:125-140`은 DB 복구가 실패하고 DB·실행 파일의 짝이 같은지 증명할 수 없으면 서버를 다시 켜지 않는다. `test_failed_data_restore_does_not_restart_changed_pair`가 이 동작을 명시한다. | **설계상 남음, 줄1·줄3.** 불일치한 DB를 새 서버로 여는 위험을 피하는 동작이다. S106의 '항상 서버 켜기'와 자료 보호가 충돌하는 상황으로, 안전한 복구 경로와 사용자 안내를 결정해야 한다. 실제 장애 발생은 확인하지 않았다. |

## 네 경로 대조

| 단계 | PC 새 설치 | PC 기존 VM 갱신 | VM 자체 업데이트 | 휴대폰 새 VM 시작 |
|---|---|---|---|---|
| 바탕·의존성 | `momso_server_install.py:244-264`, `momso_anywhere_install.py:34-48,263-276` | `momso_server_install.py:666-720`의 기존 VM·갱신 경로 | `update.py:199-240`은 서명 묶음을 받음. 새 OS 패키지를 설치하는 단계는 보이지 않음 | `momso_anywhere_vm_startup.mjs:20-34`; 이번에 브라우저 추가 |
| 파일·신뢰 | 서버·화면 지문, 프록시 파일 목록, 관리 지문 | 서명된 릴리스·관리 지문과 체크포인트 | `update.py:171-180,220-240` 서명·지문·안전 추출, `managed_helpers.py` 관리 파일 | 시작 스크립트가 묶음 지문 확인, 프록시·복원·업데이터 파일 복사 |
| 작업 중인 자료 | 최초 설치, 복원 시 별도 사전 복원 절차 | 접수 중지·자료 스냅샷·복구 `momso_server_install.py:346-508` | 접수 중지 후 서버 정지·자료 스냅샷 `update.py:242-277` | 복원 시 표식과 복원 절차를 사용; 기존 디스크 덮기 금지 |
| 완료 판단 | 로컬 프록시와 공개 입구 경유 확인 `momso_server_install.py:1082-1090` | 입구 전환 검증과 프록시 확인 `momso_server_install.py:716-720` | 서버·프록시·공개 입구 경유 확인 `update.py:283-298` | VM 로컬 프록시 확인 `momso_anywhere_vm_startup.mjs:215-222`, 브라우저 설치 흐름이 공개 입구 경유 확인 `momso_anywhere_google_browser.mjs:695,726-755` |

OS 패키지 목록은 새 설치 경로에는 있지만 VM 자체 업데이트에는 일반적인 패키지 이행 단계가 보이지 않는다. 현재 릴리스에 새 패키지가 필요하다는 뜻은 아니다. **의심, 줄1·줄2:** 미래 판이 새 시스템 도구를 요구할 때 자동 업데이트만으로 준비되는지 릴리스 절차에 확인이 필요하다.

## 반복 정의와 예외 검토

| 위치 | 판단 |
|---|---|
| 작업자 목록: `scripts/momso_anywhere_vm_startup.mjs:91`, `scripts/momso_anywhere_update_install.py:43,115`, `scripts/prepare_momso_anywhere_bundle.py:35-43`, `momso-server-update/managed_helpers.py:13-16` | 네 군데에 따로 있다. 이번 누락을 수정하고 시작 목록 대조·실제 import 시험을 추가했지만 단일 목록으로 통합된 것은 아니다. **남음, 줄1·줄2.** |
| 역할 권한: `scripts/momso_anywhere_google_browser.mjs:577,628,657,857`, `scripts/momso_anywhere_vm_services.py:38-53`, `scripts/momso_anywhere_update_install.py:86` | 생성과 삭제의 권한 목록이 여러 군데 있다. 한 곳만 변경하면 소유권 확인이나 삭제가 막힐 수 있다. **의심, 줄2:** 실제 불일치는 이번에 확인하지 않았다. |
| 서비스·경로: `scripts/momso_anywhere_vm_startup.mjs:204-235`, `scripts/momso_anywhere_update_install.py:119-121`, `momso-server-update/worker.py:18-27`, `momso-server-update/install_worker.py:52-55` | 서비스명·설정 경로가 반복된다. 현재 눈으로 비교한 이름은 같지만 모든 조합에 대한 단일 정본은 아니다. **의심, 줄2·줄3.** |
| `scripts/momso_anywhere_deletion_preview.py:57-80` | 조회 오류를 삼키지만 기본 결과를 `unknown`으로 남긴다. 성공·비용 0으로 바꾸지는 않는다. 오류 원인은 이용자가 구별하기 어렵다. |
| `momso-server-update/update.py:105` | 화면 진행 이벤트 오류를 무시하지만 바로 앞에 고정 코드로 기록하고 롤백을 계속한다. 이 한 곳은 성공을 거짓으로 반환하는 오류 삼킴으로 판단하지 않았다. |
| 생성 중 실패: `scripts/momso_anywhere_install.py:191-237,263-277` | 비밀·방화벽·입구를 단계별로 만든다. 신규 입구 오류에서는 입구 삭제를 시도하지만, 앞서 만든 모든 자원을 이 함수가 일괄 삭제하지는 않는다. 재시도와 소유권 보존을 고려해야 한다. **의심, 줄2:** 실패별 남은 자원·과금 재고를 전체 경로에서 재검증할 것. |

## 다섯 기존 결함의 변이 시험

`scripts/audit_momso_anywhere_s173.py`는 파일을 임시 사본에 복사해 한 항목씩 바꾼다. 정상 코드의 종료값 0, 변이의 0이 아닌 종료값을 모두 확인했다. 실제 계정에는 접속하지 않는다.

| 결함 | 실행한 시험과 되돌린 내용 | 결과·한계 |
|---|---|---|
| 프록시 의존 파일 누락 | `test_proxy_local_import_closure_is_in_manifest`: 목록에서 실제 import 파일 하나 제거 | 잡음. 별도 리눅스 시험은 세 전달 경로의 실제 import도 확인. |
| 입구·프록시 건강 확인 누락 | `test_failed_private_health_probe_restores_old_revision`: 갱신 시 `health_check()` 제거 | 잡음. 이 변이는 PC **갱신** 경로이고 실제 공개 Google 입구 전체를 검증한 것은 아니다. |
| 정지 VM을 깨우기 전 SSH | `test_stopped_vm_starts_before_ssh`: 시작 호출 제거 | 잡음. 가짜 공급자의 호출 순서 시험. |
| 삭제 중 한 단계 실패 | `explicit discard survives authorization and removes only owned fixture resources`: 전부 지우기에서 서브넷 실패 시 남은 목록 대신 예외 | 기존 시험은 `pending`만 확인해 **변이를 잡지 못했다**. 남은 서브넷·네트워크와 뒤의 입구 정리를 확인하도록 보강한 뒤 변이를 잡았다. 위 라우터 오류는 여전히 별도 문제다. |
| 유휴 종료 예외 뒤 정지 | `hq/s162-done`의 `test_failed_gate_releases_and_next_idle_attempt_stops`: 같은 부팅 표식을 옛 `stop_pending` 동작으로 변경 | 개발용 리눅스 VM에서 정상 통과, 변이 실패. 기준 `3d6058a3`에는 이 고침이 아직 없다. |

추가로 새 작업자 import 누락과 휴대폰 브라우저 설치 누락도 변이 시험에서 각각 잡았다. 라우터 오류 사례는 제품을 바꾼 변이가 아니라 가짜 공급자의 실패 위치를 바꾼 별도 관찰이다.

## 실행과 한계

- Windows: 변이 시험 원본·변이 6쌍 및 라우터 실패 관찰 성공; 휴대폰 시작 시험 10개, 삭제 시험 9개, PC 설치 관련 다른 시험은 프록시 2개(Windows에서 Linux 전용 2개 생략), anywhere 설치 18개, VM 깨우기 8개(4개 Linux 전용 생략) 통과.
- Windows `test_momso_server_install.py` 44개 중 하나는 `WinError 1314`로 심볼릭 링크를 만들 수 없어 실패했다. 권한 설정은 변경하지 않았다. 같은 모음은 개발용 리눅스 VM에서 44개 모두 통과했다. 임시 시험 파일을 읽는 곳의 `ResourceWarning`도 출력됐다.
- 개발용 리눅스 VM: 프록시 전달·작업자 import 4개, 관리 작업자 6개, 서명된 작업자 및 상속 시험 25개가 통과했다. 업데이트 정리 변경을 포함한 후속 시험 30개도 통과했다. 유휴 표식과 임시 묶음 정리는 정상 통과·변이 실패를 확인했다.
- 공개 입구의 실제 OAuth 로그인, 실제 고객 Google 자원, 실제 비용과 장애 후 회복은 실행하지 않았다. 검토는 지정 범위의 정적 대조와 위 시험이며, 모든 분기·모든 구름 서비스 실패를 증명하지 않는다.

## S175 후속 (2026-11-25, 가지 `claude/anywhere-fixes`)

위 표에서 "남음"이던 두 건을 고쳤다.

- **업데이트 접수 중지 응답 유실(라, 접속 불가)**: `momso-server-update/server_admission.py` 가 `POST /maintenance` 를 보내기 **전에** `lifecycle/admission.pending` 을 남기고, 문 열기가 확인된 뒤에만 지운다. 닫힌 문은 이 표시가 있을 때만 자기 것으로 거둔다(표시 없이 닫힌 문은 예전처럼 건드리지 않는다). 표시가 있는데 문이 열려 있으면 표시만 지운다. 같은 프로세스가 앞선 실패 뒤 잠금을 쥔 채여도 이어서 진행한다. `update.py::Updater.recover` 는 전환 기록이 없을 때 이런 문을 거둬 다시 연다(`update stage=recovered reason=stale_admission_released`, 화면 상태는 바꾸지 않음). 표시가 남아 있으면 작업자가 60초 안에 다시 시도한다. ServerAdmission 을 쓰는 쉬기 흐름(PC·VM)도 같은 표시를 쓴다. 거둔 문은 `closedAt` 을 모르므로 쉬기 흐름은 예전처럼 `maintenance_time_unavailable` 로 멈추고 문을 연다. 남은 한계: 응답이 사라진 뒤 표시를 남긴 쪽이 다시 오기 전에 다른 도구가 잠금을 잡고 문을 닫은 채 죽으면, 그 문도 거둬 다시 연다.
  - 시험 `test_server_admission.py` 7개 추가(응답 유실 뒤 같은 프로세스·재시작 뒤 거두기, 표시 없는 문은 안 건드림, 낡은 표시 지움, 작업자 복구 두 경우, 남의 문은 그대로). 고침 전 코드로 돌리면 7개 모두 실패. 개발 VM 에서 `test_server_admission`·`test_update`·`test_systemd_host`·`test_github_releases`·`test_worker`·쉬기 시험 다섯 묶음 통과.
- **휴대폰 전부 지우기 중 라우터 삭제 실패(마, 요금 샘)**: `scripts/momso_anywhere_google_browser.mjs::destroy` 가 VM 삭제 미확인·소유권 미확인·인증 만료·취소일 때만 멈추고, 그 밖의 구글 오류는 그 자원만 `remaining` 에 남기고 계속한다. 라우터·방화벽·서브넷 가운데 하나라도 남으면 네트워크는 건너뛰어 남긴다. 그래서 입구·비밀값·계정이 지워지고 시작 페이지가 백업 삭제(`discardBackup`)까지 부른다.
  - 시험 `test_momso_anywhere_destroy.mjs` 2개 추가(라우터 실패 뒤 입구·비밀값·계정·방화벽 삭제와 다음 시도 완료, 전부 지우기에서 백업 삭제 호출). 고침 전 코드로 돌리면 둘 다 실패. 가짜 공급자 시험이며 실제 구글 자원에서는 **미확인**.

"의심"으로 남은 것을 훑은 결과:

- 역할 권한: 정지(`compute.instances.stop`)·깨우기(`compute.instances.get`, `compute.instances.start`)·업데이트(`run.*` 다섯)가 휴대폰 생성(`momso_anywhere_google_browser.mjs:577,628,657`)·삭제(`:857`)·PC(`momso_anywhere_vm_services.py`, `momso_wake_install.py`, `momso_anywhere_update_install.py`)에서 모두 같다. 지금 결함은 아니다.
- 서비스 이름·설정 경로(`momso-server.service`, `momso-private-proxy.service`, `momso-update.service`, `/etc/momso-server.env`, `/etc/momso-update.json`)도 서로 맞다. 단일 정본으로 묶는 일은 하지 않았다.
- 작업자 파일 목록 네 곳, VM 자체 업데이트의 OS 패키지, 생성 중 실패 뒤 남는 자원은 지금 확인된 결함이 아니라 그대로 둔다.
- 기준의 유휴 표식 건은 S162·S174 고침(`momso-idle-stop-s174.md`)으로 덮였다.
- `scripts/test_momso_anywhere_google_end_to_end.mjs` 의 세 시험은 이 고침 전에도 Windows 에서 실패한다(원래 실패, 이 일과 무관).
