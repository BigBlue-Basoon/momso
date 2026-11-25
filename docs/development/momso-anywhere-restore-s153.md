# S153 쉬기 뒤 복원

## 변경 전 확인

- **버킷·KMS·금고: 경우에 따라 생성.** 기존 자산의 소유권이 맞으면 재사용했지만, 빠진 버킷·KMS·금고를 새로 만들 수 있었다. 금고에 유효한 원래 비밀값이 없는 상황에서 초기 값을 만들면 기존 백업을 읽을 수 없게 될 위험이 있었다.
- **설치 ID:** PC는 로컬 설치 상태 파일의 12자리, 휴대폰은 탭이 아닌 기기의 로컬 저장소에 있는 24자리를 썼다. 해당 기록이 없는 다른 기기에서는 쉬는 설치를 자동으로 찾지 못했다.

## 설치 전 확인과 복원

- 프로젝트 전체에서 소유 라벨과 이름이 맞는 금고·버킷·KMS 키 및 VM을 조회한다. 보존된 설치가 하나면 그 전체 ID를 이어 쓰고, 둘 이상이면 `restore_selection_required`와 후보를 반환한다. PC는 `--restore-installation-id`, 휴대폰은 `startFromGesture`의 `installationId`로 후보를 지정한다.
- PC와 휴대폰은 12자리 또는 24자리 **전체 ID**를 라벨과 소유 표식으로 검사한다. 자원 이름의 짧은 부분은 앞 12자리다. 접두사가 같다는 이유로 다른 전체 ID의 자원을 채택하지 않는다.
- 미리 보기 결과는 `restoreAvailable`, `lastBackupAt`을 포함한다. 세 자산 중 일부만 있거나 원래 금고 비밀값의 1번 판이 없으면 `restore_assets_incomplete`, 확인할 수 없으면 `restore_assets_unverified`다. 보존 자산 전용 모드에서는 빠진 자산을 만들지 않는다.
- 서명된 VM 묶음의 복원 작업자와 백업 관련 파일은 `scripts/momso_anywhere_restore_files.json`이 정본이다. PC는 이미 서명과 지문을 확인한 묶음에서 이 파일들을 읽는다. VM에서 원래 금고 비밀값을 systemd `LoadCredential`로 전달하고 `User=momso`로 복원한다.
- 서버·자동 갱신·유휴 종료·백업 서비스와 백업 타이머는 복원 성공 전에 시작하지 않는다. 성공 결과에는 `restored`, `restoredBackupId`, `restoredAt`이 들어간다. 실패는 `restore_failed`와 확인 가능한 `detail`을 남긴다. 동일 디스크의 완료 표식은 재부팅 시 예전 백업을 다시 적용하지 않게 한다.
- 이미 같은 전체 ID의 VM이 다른 영역에 있으면 새 VM의 복원을 진행하지 않는다. PC의 재시도는 설치 기록에 보관한 VM의 불변 ID를 다시 확인한다.
- VM 계정에는 버킷과 객체 읽기, KMS 키 확인·복호화, 금고 비밀값 읽기 권한이 필요하다. Google 공식 문서에서 [버킷 읽기 역할](https://docs.cloud.google.com/storage/docs/access-control/iam-roles)과 [KMS 키 읽기·복호화 역할](https://docs.cloud.google.com/kms/docs/reference/permissions-and-roles)을 확인했다.

## 시험과 남은 확인

- 가짜 공급자와 리눅스 시험으로 소유권·부분 자산·파일 목록·복원 작업자의 단위 동작을 확인한다. 이것은 실제 Google Cloud에서 쉬기와 재설치가 끝까지 성공했다는 뜻이 아니다.
- `scripts/momso_anywhere_rehearsal_s147.py`는 설치·시험 자료 쓰기·쉬기·재설치·비교·정리 순서를 제공한다. 정리 결과의 `deleted_unverified`는 확인하지 못한 종류를 `leftovers.unverified`에 남기며, 예약 파기 중인 KMS는 시각과 과금 상태를 따로 남긴다. 실제 실행은 본부가 한다.
- 이미 같은 설치 ID의 VM이 있으면 복원하지 않는다. PC는 원래 입구와 소유권·주소까지 확인되는 경우 기존 VM의 갱신 경로를 사용하며, 이를 확인하지 못하면 `restore_vm_exists`로 멈춘다.
- 복원 작업자가 성공했다고 기록한 직후 완료 표식을 남기기 전에 프로세스가 중단된 드문 경우에는 다시 덮어쓰지 않고 중단한다. 이 경우 복원 결과와 VM 상태의 수동 확인이 필요하다.
- S147b의 제한은 그대로다. 백업 슬롯 일곱 개가 모두 찬 상태에서 복원 후 내용이 달라지면 첫 백업은 `restore_first_backup_no_free_slot`로 실패한다. 기존 백업은 보존되지만 새 백업과 자동 종료가 막힐 수 있다.
