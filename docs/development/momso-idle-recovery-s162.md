# 유휴 종료 재시도와 진단 (S162)

사람에게 보고된 순번 7의 기록에는 유지보수 표시가 생긴 뒤 실패 로그가 한 줄 있고 VM 정지 요청 감사 기록은 없다. 기존 `idle_stop.py`의 `step`은 표시를 만든 다음 `/maintenance` 호출, 응답 확인, `/activity` 조회, 유지보수 해제, 메타데이터와 토큰 조회, VM 정지 요청에서 예외가 날 수 있었다. 서버 호출은 `127.0.0.1:8765`로 직접 하므로 프록시를 거치지 않는다. 예전 코드는 구체적인 실패 종류를 지워서 실제로 어느 단계였는지 확정할 수 없다. 가능한 원인 두 가지는 **서버 유지보수·상태 요청의 일시적인 실패**, **VM 메타데이터·토큰 조회 실패**다. 당시 원인을 입증한 것은 아니다.

기존 코드는 예외 뒤 표시를 보존하고 `step`은 표시가 존재하면 `stop_pending`만 돌렸다. `ServerAdmission.acquire`도 같은 표시를 보면 대기하므로 업데이트와 유휴 종료가 함께 진행되지 못할 수 있었다. 두 작업은 `lifecycle.lock`을 함께 쓰므로 잠금 점유 중에는 상대 작업의 문을 열지 않는다.

개발 Linux VM에서 이전 판 `883267f52`의 `test_unconfirmed_cloud_request_keeps_gate_until_recovery`로 예외를 주입하자 같은 부팅의 표시가 남고 복구가 `stop_pending`이 되는 현상을 재현했다. [공개 순번 7 코드](https://github.com/BigBlue-Basoon/momso/blob/37dfc50/apps/alpha/desktop/src-tauri/momso-idle-stop/idle_stop.py)와 대조했을 때 재현한 `step`·`recover` 코드는 같았고 다른 부분은 유휴 시간 설정 인자 처리였다. 이 시험은 예외를 주입해 영구 대기하는 결과를 재현한 것이며, 고객 VM에서 당시 어떤 예외가 났는지는 밝혀 주지 않는다.

새 코드는 다음 반복 또는 서비스 재시작에서 같은 부팅의 표시도 회복한다. 문을 다시 여는 응답을 확인한 뒤에만 표시를 지운다. 서버가 기록하는 마지막 활동 시각을 기준으로 유휴 시간을 다시 잰다. 실패는 `idle_stop_unavailable reason=<단계>:<예외 종류>`로 남기며 예외 메시지와 토큰은 기록하지 않는다. 재시도 대기는 두 배씩 늘어나 최대 900초로 제한되며, 성공하면 원래 간격으로 돌아간다.

정지 요청이 전송됐으나 답이 유실될 수 있으므로, 요청을 보내기 전에 기록한 뒤 같은 부팅에서 300초 동안 문을 닫은 채 기다린다. 그 뒤에도 살아 있으면 회복을 시도한다. **300초는 운영 완료를 보장하는 Google 계약이 아니다**. 매우 늦게 처리되는 정지 요청은 남은 위험이며, 실제 Google 작업 상태를 판별하는 개선은 별도 검증이 필요하다.

인증된 서버 `GET /activity`는 기록을 읽을 수 있을 때 `idleStop: {attemptedAt, result}`를 추가한다. `attemptedAt`은 RFC3339, `result`는 `stop_requested`, `resumed`, `error:activity:OSError` 같은 값이다. 기존 파일이 없거나 형식이 잘못되면 이 항목은 생략된다. 서버 재시작과 휴대폰 재접속 뒤에도 마지막 기록을 볼 수 있다. 이 값은 민감한 HTTP 응답이나 자격 증명을 담지 않는다.

`hq/s159-done`을 합쳐 인증된 `/diagnostics/memory`(`browser_bridge.rs::memory_diagnostics`)에도 같은 `idleStop` 값을 넣었다. `/activity`와 같은 검증 함수를 사용한다.

뒤에 원인을 확정했다: 문 닫기가 시작한 백업 때문에 문 열기가 `409 backup_in_progress` 로 거절된 것이다. 이 재시도만으로는 끝없이 되풀이된다. 고침과 실제 server 프로세스 시험은 `momso-idle-stop-s174.md`.
