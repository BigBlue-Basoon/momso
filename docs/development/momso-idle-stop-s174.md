# 유휴 종료 시 백업 대기 (S174)

## 진짜에서 본 것

순번 7·8 의 진짜 설치에서 유휴 기준(30분)에 닿은 첫 시도가 두 번 연속 `idle_stop_unavailable` 로 실패했다. 실패 뒤 server 의 문은 닫힌 채(`maintenance: true`) 남아 입구가 `maintenance_in_progress` 만 돌려줬다. 끄기 호출(`Compute().stop_self()`)만 따로 돌리면 VM 이 꺼졌으므로 끄기 호출은 원인이 아니었다(본부 기록 `s174-idle-stop-evidence.txt`).

## 원인

순번 8의 공개 묶음과 서버 실행 파일을 각각 SHA-256으로 대조하고, 개발 Linux VM에서 실제 서버와 느린 가짜 백업 작업자를 함께 띄웠다. 실제 계정·실제 VM 종료는 사용하지 않았다. 60초 유휴 뒤 `/maintenance`는 `maintenance: true`를 돌려줬지만, 곧바로 `/activity`는 `activeBackups: 1`, `busy: true`, `quiescent: false`, `shutdownReady: false`였다. 기존 스크립트가 바로 `/maintenance/release`를 호출하자 `409`가 돌아와 `stop_or_gate_unconfirmed`로 끝났고 종료 호출에는 이르지 못했다.

까닭은 server 의 `POST /maintenance`(`apps/alpha/server/src/maintenance.rs::acquire`)가 문을 닫으면서 백업 세대를 하나 올리고 백업을 깨우는 데 있다(업데이트 전에 새 백업을 받기 위한 동작). 진짜 설치는 개체 백업(`MOMSO_BACKUP_AGENT`)이 켜져 있어 문을 닫을 때마다 새 백업이 돈다. server 는 백업 중에는 문 열기를 `409 backup_in_progress` 로 거절한다.

S162 의 재시도만으로는 끝나지 않는다. 다음 반복에서 문을 다시 열면 server 가 `lastActivityAt` 을 지금으로 바꾸므로 유휴 시간을 처음부터 다시 잰 뒤 같은 자리에서 또 실패한다. 개발 VM 에서 순번 8 server 실행 파일과 S162 스크립트를 실제 프로세스로 띄워(유휴 60초, 백업 4초) 200초 지켜본 결과, 60초마다 `release:HTTPError` → `resumed` 를 세 번 되풀이하고 끄기 호출은 한 번도 없었다. 같은 까닭으로, 문 닫기가 시작한 백업이 기다림 한도보다 길면 고친 뒤에도 같은 되풀이가 난다. 그래서 한도를 넉넉히 두고 문 열기도 재시도한다(아래).

기존 시험의 가짜 서버는 유지보수 직후 `quiescent`와 `shutdownReady`를 곧바로 참으로 돌려줬고 문 열기도 언제나 성공했기 때문에 이 경합을 만들지 못했다.

## 고침 (`apps/alpha/desktop/src-tauri/momso-idle-stop/idle_stop.py`)

- 문을 닫은 뒤 `/activity` 가 아직 준비 안 됨이어도, 사람의 일이 없고(`activeTurns`·`pendingApprovals`·`pendingInputs`·`activeWrites`·`githubLogins`·`admittedRequests`·`pendingPurges` 가 0, `providerLogin` false) `lastActivityAt` 이 문 닫기 전과 같으면 "가라앉는 중"으로 보고 2초 간격으로 다시 읽는다. 준비되면 끈다. 승인이 대기하거나 활동 시각이 바뀌는 등 다른 일이 있으면 기다리지 않고 문을 열고 `activity_changed`.
- 기다리는 한도는 `--settle-seconds`(기본 300초). 넘으면 문을 다시 열고 `settle_timeout` 으로 기록한다. 백업이 끝나지 않으면 종료하지 않는다.
- 문 열기가 `409`(백업 중, 처리 중인 요청)로 거절되면 같은 한도 안에서 다시 시도한다. 그래도 안 되면 예외를 내고 표시를 남겨 S162 경로로 다음 반복에서 다시 연다.
- 실패 까닭에 HTTP 상태와 server 의 고정 오류 코드를 남긴다(예: `release:HTTP409:backup_in_progress`, `reopen:HTTP409:backup_in_progress`). 응답 본문이 정해진 모양(`[a-z_]`)이 아니면 상태 번호만 남긴다. 토큰·본문은 기록하지 않는다.
- 평소 결과(`not_ready` 등)가 아닌 결과는 `idle_stop_result result=…` 한 줄로 journald 에 남긴다. 상태 파일과 `/activity` 의 `idleStop` 칸도 같은 값을 보인다.

기다리는 동안 문은 닫혀 있으므로 그 사이에 돌아온 사람의 요청은 `maintenance_in_progress` 를 받는다(최대 `--settle-seconds`). 백업이 실패한 채 남으면(`shutdownReady` 가 계속 false) 끄지 않고 문을 다시 연다.

## 확인

- 가짜 시험 `test_idle_stop.py`(개발 VM, 리눅스): 가짜 server 가 이제 문 닫기 때 백업을 시작하고 백업 중 문 열기를 409 로 거절한다. 문 닫기가 시작한 백업 기다리기, 백업 중 진짜 활동, 한도를 넘는 백업의 실패 까닭과 다음 반복의 문 열기, 백업이 끝내 준비 안 됨, 까닭에 본문을 옮기지 않음을 본다.
- 실제 server 프로세스 시험 둘:
  - `test_real_server_s174.py` + `scripts/momso_s174_reproduce.sh`: 순번 8 공개 파일 지문을 확인하고, 실제 server + 느린 가짜 백업 + 가짜 종료 호출로 `step()` 한 번을 본다. 고객 환경을 쓰지 않는다.
  - `test_real_idle_stop.py`(선택형, `MOMSO_IDLE_REAL_SERVER`·`MOMSO_IDLE_REAL_AGENT`): 실제 server 프로세스 + 시험용 백업 대행(`fake_backup_agent.py`, root 소유 폴더에 둬야 server 가 실행한다) + 실제 idle-stop 프로세스(끄기 호출만 가짜). ① 문 닫기가 시작한 백업(4초)을 기다린 뒤 유휴 60초 만에 끄기 호출 도달. ② 백업(25초)이 기다림 한도(8초)와 문 열기 재시도를 넘기면 첫 시도는 `release:HTTP409:backup_in_progress` 로 실패하고, 다음 반복에서 문을 다시 열고(`resumed`), 그다음 시도에서 끄기 호출 도달.
- 진짜 구글 설치에서의 확인: **미확인**(본부 몫).

`test_real_idle_stop.py` 를 개발 VM 에서 다시 돌리는 법:

```
sudo install -d -o root -g root -m 755 /opt/momso-idle-test
sudo install -o root -g root -m 644 fake_backup_agent.py /opt/momso-idle-test/agent.py
MOMSO_IDLE_REAL_SERVER=<리눅스 momso-server> MOMSO_IDLE_REAL_AGENT=/opt/momso-idle-test/agent.py \
  python3 -m unittest test_real_idle_stop -v
```
