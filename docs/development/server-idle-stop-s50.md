# S50: 쉬는 VM 종료 — 설치 담당 계약

## 지금 상태

`apps/alpha/desktop/src-tauri/momso-idle-stop/idle_stop.py`는 VM 안에서 도는 독립 Python 3 서비스다. 설치기·server 파일은 바꾸지 않았다. 현재 server의 `shutdownReady:false`, `shutdownReason:"backup_not_integrated"`에서는 **종료하지 않는다**. S46의 활동 값과 백업 완료를 server가 실제로 제공하고 아래 공통 잠금까지 연결하기 전에는 제품에서 켜지 않는다. 시험은 가짜 `/activity`와 가짜 Compute API로만 한다.

## 설치할 것

- 스크립트를 server와 별개인 장기 실행 서비스로 설치한다. server 토큰은 명령 인자나 환경 변수의 값에 넣지 말고, server 전용 OS 계정만 읽는 파일을 systemd `LoadCredential` 등으로 전달한다. 명령은 `python3 /opt/momso/idle_stop.py --token-file <전용 자격증명 파일> --state-dir /var/lib/momso/lifecycle`이다. 기본 30분은 `--idle-seconds 1800`, 확인 주기는 `--poll-seconds 60`이며 설치 설정으로 바꿀 수 있다. 포트는 기본 loopback 8765다.
- `/var/lib/momso/lifecycle`은 재부팅 뒤에도 남고 해당 서비스와 updater만 쓸 수 있게 한다. `lifecycle.lock`은 종료기와 업데이트 조정기가 **함께** 잡아야 한다. 두 조정기는 서로의 복구 대기 표식도 확인하고 진행 중이면 시작하지 않는다. 현재 S38 updater는 아직 이 공통 잠금에 연결되지 않았다. S46 `/maintenance`는 기존 요청에도 성공을 돌려주므로 이 조정 없이 둘을 동시에 켜면 서로의 접수 중지를 잘못 풀 수 있다.
- VM의 전용 서비스 계정에 이 **인스턴스 하나**에만 `compute.instances.stop`을 가진 사용자 지정 역할을 준다. Compute API 접근 scope도 필요하다. 종료기는 메타데이터에서 자기 프로젝트·영역·VM 이름과 짧은 수명 토큰을 읽고 그 VM의 stop API만 부른다. 토큰은 파일·로그·명령 인자·화면에 내보내지 않는다. 서비스 계정이 다른 VM을 멈추지 못하는지는 실제 IAM 정책으로 확인해야 한다. [필요 권한](https://docs.cloud.google.com/compute/docs/instances/stop-start-instance), [메타데이터 토큰](https://docs.cloud.google.com/compute/docs/access/authenticate-workloads), [stop API](https://docs.cloud.google.com/compute/docs/reference/rest/v1/instances/stop).

## 정지와 재시작의 약속

인증된 `/activity`에서 `shutdownReady:true`, `busy:false`, 진행 턴·승인·입력·쓰기·GitHub 로그인·접수된 요청 0개, AI 로그인 없음, `lastActivityAt`으로부터 설정 시간이 지났는지를 확인한다. 빠진 값이나 오류는 종료 불가다. 이후 공통 잠금 아래 `/maintenance`로 새 요청을 막고 같은 조건과 `quiescent:true`를 다시 확인한다. 그 사이 활동이 생기면 접수를 다시 연다. 백업 준비는 server의 `shutdownReady`가 대표하므로 백업 구현자가 모든 백업 작업을 이 값에 반영해야 한다.

정지 요청이 성공해도 Compute의 수락은 VM이 이미 멈췄다는 뜻은 아니다. 응답이 끊기는 등 결과가 불확실하면 접수를 계속 막고 `idle-stop.pending`을 보존한다. **같은 부팅에서 서비스만 다시 시작해도 잠금을 풀지 않는다.** 다음 VM 부팅에서 부팅 ID가 바뀐 것을 확인한 뒤 `/maintenance/release`를 호출하고 표식을 지운다. 부팅 ID를 확인할 수 없거나 release가 실패하면 계속 접수를 막고 오류를 남긴다. 정지 요청이 수락되지 않았는데 응답도 잃은 경우에는 관리자 확인이나 VM 재부팅이 필요할 수 있다.

입구는 중지된 VM을 다시 켜고 server 준비를 기다린다. 재시작 시 종료기의 복구가 끝나기 전에는 휴대폰 요청을 재개하지 않아야 한다. 정지 중 새 요청은 기존 S46 계약대로 일시적인 `maintenance_in_progress` 응답이 될 수 있으므로, 입구·화면은 진행 중 표시와 안전한 재시도를 제공한다. **끊긴 요청이 자동으로 한 번만 처리된다고 이 부품이 보장하지 않는다.** 요청 식별자와 재시도 정책은 server·화면 담당이 확인한다.

VM 중지는 디스크를 지우지 않으므로 같은 영구 디스크에 있는 채팅 DB·몸소 폴더·보관소는 재시작 후 그대로 남는 설계다. VM 또는 디스크 자체가 사라졌을 때의 복원은 GitHub와 S30 암호화 백업이 담당하며, 이 종료 시험만으로 실제 복원을 증명하지 않는다. 디스크·네트워크 등 정지 후에도 남는 비용이 있을 수 있다.
