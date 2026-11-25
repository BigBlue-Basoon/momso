# PC 설치기: 보관함과 자동 종료 (S56)

PC 설치기의 새 설치와 기존 설치 갱신은 VM 소유 표식을 확인한 다음 전용 서비스 계정과 보관함 열쇠를 준비한다. 이미 다른 서비스 계정이 붙은 VM은 덮어쓰지 않고 중단한다. 계정을 처음 붙일 때 작업 접수를 잠시 멈추고 유휴 상태를 확인한 후 같은 VM을 한 번 껐다 켠다. 작업 중이면 중단하고 나중에 재시도한다. 설치가 중간에 끊겼을 때 소유 표식이 있는 중지 요청만 재시도에서 해제한다.

열쇠는 VM에서 생성해 고객 프로젝트의 Secret Manager에 저장하고 systemd `LoadCredential`로 전달한다. 재부팅과 설치기 재실행 시 기존 1번 버전을 사용한다. 열쇠가 없더라도 기존 `.age` 파일이 있으면 새 열쇠로 덮어쓰지 않는다. 저장된 로그인과 AI 키는 암호화된 자료이며, 복원에는 이 Secret Manager 열쇠도 함께 필요하다. 열쇠 자체를 일반 백업에 평문으로 넣지 않는다.

VM 전용 계정에는 해당 보관함 하나의 읽기와 해당 VM 하나의 `compute.instances.stop`만 지속적으로 부여한다. 열쇠의 첫 버전을 만들 권한은 설치 중에만 부여하고 제거한다. 시스템의 `momso-idle-stop`은 30분 동안 사용하지 않았더라도 `/activity`의 `shutdownReady`가 참이고 작업 접수를 중지한 뒤에도 유휴 상태일 때만 종료한다. 현재 서버가 `backup_not_integrated`를 주면 종료하지 않는다. 이 서비스는 백업이 완료되었다고 임의로 판단하지 않는다.

휴대폰 시작 흐름 담당: `momso_anywhere_vm_startup.mjs`에도 동일한 VM 전용 계정·보관함 비밀 버전·systemd credential·유휴 종료 설치와 재실행 보호를 연결해야 한다. 이 변경에서는 해당 파일을 고치지 않았다.

근거: [Secret Manager 최소 권한](https://docs.cloud.google.com/secret-manager/docs/access-control), [VM 서비스 계정 변경](https://docs.cloud.google.com/compute/docs/instances/change-service-account), [systemd 서비스 자격 증명](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html#Credentials).

기존 설치를 갱신할 때 S59 자료 복구를 먼저 실행하고 새 서버가 정상인지 확인한 다음 보관함·자동 종료 서비스를 설치한다. 옛 서버는 작업 상태를 조회해 기다리거나, 상태 조회가 없으면 서버를 먼저 멈춘다. 자세한 조건은 server-upgrade-data-s59.md를 따른다.

## S63: 업그레이드 중 입구와 설치 파일

코덱스의 `codex/tmp`는 임시 파일이라 자료 스냅샷에서 제외한다. 다른 위치의 외부 심볼릭 링크는 계속 거절한다. 서버가 멈추면 systemd의 `Requires=` 관계로 사설 proxy도 멈추므로 성공과 복구 실패를 포함해 종료할 때 proxy를 다시 시작하고, 로컬 인증서와 전송 토큰으로 proxy 너머 `/health`를 확인한다. systemd 관계의 근거: https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html .

설치기는 서버 업데이트·보관함·자동 끄기에 필요한 파일을 작업 시작 전에 확인한다. 파일이 없으면 전체 설치 묶음을 다시 받으라고 안내한다. 새 VM 종료 역할을 만든 뒤 권한 전파가 늦을 수 있어 VM 역할 연결을 최대 12번, 5초 간격으로 재시도한다.
