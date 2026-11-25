# 새 VM 시작 전 백업 복원 계약

설치기 담당은 서명된 묶음의 `momso_anywhere_prestart_restore.py`, `momso_anywhere_backup.py`, `momso_anywhere_backup_agent.py`, `momso_anywhere_backup_google.py`를 같은 폴더에 풀고, 보존된 금고 비밀값의 원래 1번 판을 `/run/momso-vault/momso-vault-identity`에 준비한다. 복원 작업자와 백업 모듈·에이전트는 같은 묶음의 판으로 함께 설치하고, 기존에 사람이 고친 파일은 덮어쓰지 않는다. `momso-restore.service`를 `User=momso`로 실행하고 `LoadCredential=momso-vault-identity:/run/momso-vault/momso-vault-identity`를 지정한다. 그러면 작업자가 복원한 파일을 서버 사용자도 읽을 수 있다. **호출부터 종료까지 서버·업데이트·유휴 종료·백업 서비스와 백업 타이머가 시작될 수 없게 보장하고**, 종료 코드 0과 `ok: true`를 확인한 후에만 서버와 백업 작업자를 켠다. 기존 설치기의 호출은 이 변경에 포함되지 않는다.

보존된 금고 비밀값이 있는 재설치에서는 그것을 교체하거나 새로 만들지 않는다. 완전히 새로운 설치에서는 기존 금고 준비 절차로 처음 비밀값을 만든다. **비밀값을 만들기 전에** 버킷 접근과 백업 부재를 확인해야 하며, 백업이 있거나 백업 유무를 확인할 수 없는데 비밀값을 확인할 수 없다면 새 비밀값으로 대체하지 말고 실패해야 한다. 보존된 비밀값 준비와 이 사전 확인은 설치기 연결 범위다.

설치기는 이 작업을 **새로 만든 VM 디스크의 첫 시작에만** 실행하고, 성공 후에는 같은 설치 ID와 디스크에 묶인 완료 기록을 남겨 일반적인 VM 재부팅에서 다시 실행하지 않아야 한다. 서버가 이미 사용한 자료 위에 예전 백업을 다시 적용해서는 안 된다. 성공 전에 중단된 경우에는 서버를 켜지 않은 채 같은 절차를 다시 실행한다.

```text
python3 momso_anywhere_prestart_restore.py --installation-id <전체 설치 ID> --data-dir /var/lib/momso --bucket <같은 설치의 백업 버킷> --kms-key <같은 설치의 KMS 키 전체 이름> --vault-identity /run/credentials/momso-restore.service/momso-vault-identity --report /var/lib/momso/.momso-prestart-restore.json
```

설치 ID는 PC의 12자리 또는 휴대폰의 24자리다. 버킷·키 이름과 두 자원의 `momso-install-id` 라벨이 그 ID에 맞아야 한다. VM에는 Python 3와 `cryptography`가 필요하다. VM 서비스 계정에는 버킷 메타데이터와 백업 객체를 읽을 권한, KMS 키 메타데이터를 읽고 복호화할 권한, 보존된 금고 비밀값을 읽을 권한이 필요하다. 토큰·금고 열쇠는 인자나 결과에 넣지 않는다. 작업자는 서버·업데이트·유휴 종료·백업 서비스와 백업 타이머가 꺼져 있는지 검사하고 기존 백업 작업자의 **이 VM 안의** 파일 잠금을 잡는다. 호출자는 서비스 시작을 동시에 허용하지 말아야 하며, 같은 백업을 다른 VM이나 작업자가 동시에 쓰지 않는지도 확인해야 한다. 기존 백업 작업자 모두가 클라우드 공통 잠금을 따르는 것은 아니므로 로컬 잠금이나 전후 세대 확인만으로 다른 VM과의 배타적 접근을 보장하지 않는다.

필요한 권한은 `storage.buckets.get`, `storage.objects.get`, `storage.objects.create`, `storage.objects.delete`, `cloudkms.cryptoKeys.get`, `cloudkms.cryptoKeyVersions.useToDecrypt`, `secretmanager.versions.access`다. 정식 칸이 모두 손상된 경우에만 검증된 임시 칸을 정식 칸으로 옮기므로 복원 작업자도 객체 생성·삭제 권한이 필요하다. 서버가 새 백업을 올리는 권한은 기존 백업 역할에서 별도로 유지한다.

성공 결과는 `{"ok":true,"restored":true|false,"restoredBackupId":"slot-N@세대"|null,"restoredAt":"시각"|null}`이다. 백업 버킷의 존재와 소유권을 확인한 뒤 일곱 정식 칸과 임시 칸이 모두 없으면 `restored:false`다. 정식 칸을 최신부터 확인해 사용할 수 있는 백업을 복원하며, 복원 중 슬롯이나 세대가 달라지면 실패한다. 기존 로컬 파일의 충돌도 실패한다. 실패 결과는 종료 코드 1과 `{"ok":false,"code":"restore_failed","detail":"…"}`이고, 설치기는 이 값을 전달하면서 서비스를 시작하지 않는다. `detail`에는 `backup_unreadable`, `key_unavailable`, `checksum_mismatch`, `restore_conflict`, `backup_changed`, `backup_busy`, `ownership_unverified`, `services_not_stopped` 등이 있다.

평소와 복원 후 모두 새 백업을 임시 여덟째 칸에 저장하고 실제 복원으로 조각까지 확인한 뒤, 빈 칸에 넣거나 가장 오래된 정식 칸을 세대 조건으로 원자적으로 교체한다. 확인이 실패하면 임시 칸만 지우고 정식 칸은 그대로 둔다. 복원에 쓰인 칸과 세대는 `.momso-restore-pending-first-backup`에 기록해 새 백업이 확인될 때까지 교체 대상에서 제외한다. 내용이 같아 새 백업을 만들지 않은 경우에도 보호 표시는 유지한다. 교체 뒤 중단되면 다음 백업이 임시 칸을 확인하고 이미 정식 칸에 같은 내용이 있으면 중복 교체 없이 임시 칸만 정리한다. 백업 칸은 여덟을 넘지 않는다.

복원할 때는 검증된 정식 칸을 먼저 고른다. 정식 칸을 모두 사용할 수 없고 임시 칸만 검증되면, 임시 칸을 정식 칸으로 안전하게 옮겨 복원하고 결과에 `restoredFromTemporary: true`를 덧붙인다. 이 예외에서는 복원 작업자가 백업 객체를 갱신한다. 서버에 세부 오류가 표시되는지는 별도 확인이 필요하다. S147 연습 명령의 실제 실행 차단은 설치기 연결과 실제 경로 시험이 끝날 때까지 유지한다.
