# S154 PC 작업 전 VM 준비

PC의 기존 설치 갱신, 백업을 확인하는 쉬기, 업데이트 작업자 복구는 먼저 설치 ID 라벨을 확인한다. VM이 `TERMINATED` 또는 `STOPPED`이면 시작하고, `SUSPENDED`이면 재개한다. `RUNNING`과 IAP SSH 응답을 기다린 뒤에만 작업한다. 상태 확인 실패는 `vm_state_unknown`, 시작 실패는 `vm_start_failed`, SSH 시간 초과는 `vm_unreachable`이다. 전부 지우기처럼 VM에 접속할 필요가 없는 삭제는 깨우지 않는다.

유휴 종료 작업자와 기존 갱신·쉬기 잠금이 서로 교착하지 않도록, PC 작업 보호기는 기존 `lifecycle.lock`을 잠시 잡고 유휴 종료 서비스를 멈춘 뒤 별도 PC 작업 잠금을 유지한다. 작업 전 켜져 있었거나 설치 중 켜기로 한 유휴 종료 서비스는 보호기 종료 후 다시 켠다. PC는 60초마다 보호기를 갱신하고, 갱신이 5분간 없으면 보호기가 끝나 systemd 종료 후 복구가 실행된다. 복구를 5분 간격으로 다시 시도하는 타이머와 systemd의 24시간 상한도 둔다. VM이 재부팅되면 원래 활성화된 유휴 종료 서비스는 부팅 시 다시 켜진다. systemd의 [서비스 시간 제한과 종료 후 처리](https://www.freedesktop.org/software/systemd/man/256/systemd.service.html), [임시 서비스](https://www.freedesktop.org/software/systemd/man/254/systemd-run.html)를 따른다.

보호기 복구를 확인하지 못하면 성공으로 보고하지 않고 `vm_unreachable`을 돌려준다. VM 삭제가 확인된 경우에는 VM 안의 보호기를 해제할 필요가 없다. 24시간을 넘는 단일 작업은 보호기의 최종 상한에 도달할 수 있으므로 나누어 다시 실행해야 한다.

시험은 가짜 gcloud와 개발용 리눅스 VM의 독립 파일 잠금·임시 systemd 단위로만 한다. 실제 고객 VM 적용과 휴대폰 사용 확인은 포함하지 않는다.
