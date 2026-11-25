# S120 삭제·비용 계약 작업 기록

## 현재 상태: 미완성

### S120b 진행: 시험용 백업 정리

시험용 S117c 정리기는 버킷과 키의 소유권을 먼저 확인한 뒤 소프트 삭제 정책을 끄고 30초를 기다려 새 삭제를 진행한다. 정책을 끄기 전에 이미 소프트 삭제된 객체는 별도로 기록하며, 정책 변경만으로 사라졌다고 보지 않는다. [Google 공식 안내](https://docs.cloud.google.com/storage/docs/disable-soft-delete)에 따르면 이미 보관된 객체는 기존 보존 기간이 끝나야 하고, 정책 변경 뒤 삭제 전 최소 30초를 기다려야 한다. [KMS 공식 안내](https://docs.cloud.google.com/kms/docs/key-states)에 따르면 파기 대기 기간은 키 생성 때만 정할 수 있으며 일반 키는 최소 24시간이다. 기존 키의 기간을 삭제 시점에 줄일 수 없고, 파기 예정 버전과 보관된 객체는 과금될 수 있다.

PC `destroy --discard-data`는 같은 정리 부품을 호출한다. 버킷과 암호화 키의 이름 및 설치 ID 라벨을 확인하며, 보존 중인 삭제본이 있으면 버킷을 남겨 기한을 기록한다. 키 버전은 파기를 예약하고 `destroyTime`을 기록한다. 보존·파기 대기가 끝난 뒤 같은 명령을 다시 실행하면 확인된 버전, 키와 빈 키링을 순서대로 삭제한다. 실행 중 일시적으로 확인할 수 없으면 `unverified`와 `pending`으로 남긴다. 이 경로는 PC에 Node.js가 필요하며 Google 인증 토큰은 자식 프로세스의 표준 입력으로만 전달한다.

응답의 `resources`는 `deleted`, `retained`, `unverified`를 구분하고, `billable`은 확인된 버킷 보존과 키 버전의 종료 시각을 담는다. `backupBillingZero`와 `backupChargesEndAt`은 백업 버킷과 KMS에 대해서만 말하며, 전체 프로젝트 비용을 뜻하지 않는다. 다른 자원의 잔여 비용은 확인하지 않으므로 `billingZero`는 `null`이다. 소프트 삭제 기한이 지난 뒤에도 목록·권한 확인이 실패하면 0이라고 판정하지 않는다.

휴대폰의 `destroyFromGesture({ ..., discardData: true })`도 VM이 사라졌는지 확인한 뒤 같은 정리 로직을 호출한다. 결과의 `mode`, `resources`, `billable`, `billingZero`, `backupBillingZero`, `backupChargesEndAt`은 PC와 같은 의미다. `status: pending`은 같은 선택으로 재시도해야 한다는 뜻이며, 이 탭의 저장 공간에 설치 ID·결과·기한만 남기고 인증 토큰은 저장하지 않는다. `status: complete`는 이 설치의 확인된 삭제 절차를 뜻할 뿐 `billingZero: null`인 전체 프로젝트 비용이 0이라는 뜻은 아니다. S117c 연습도 이 휴대폰 경로의 결과를 사용하고, 보존·파기 대기나 미확인이면 통과로 기록하지 않는다.

전체 자원의 삭제 확인과 프로젝트 전용 여부 판정은 아직 완료되지 않았다. 화면에서 전체 프로젝트 요금이 0이라고 표시해서는 안 된다. 화면 담당은 `pending`의 남은 종류와 확인된 기한을 표시하고, 기한 이후 같은 삭제를 재시도할 수 있게 한다. `billingZero: null`이나 `backupChargesEndAt: null`은 알 수 없다는 뜻으로 표시한다.

프로젝트 전체 종료도 요금이 곧바로 0이라는 증거로 삼지 않는다. [프로젝트 종료 공식 안내](https://docs.cloud.google.com/resource-manager/docs/delete-restore-projects)는 청구 주기 말까지 비용이 발생할 수 있고, 약정 할인 등이 영향을 받을 수 있다고 명시한다. [Cloud Asset Inventory 검색 안내](https://docs.cloud.google.com/asset-inventory/docs/search-resources)는 검색할 수 없는 자원도 있다고 명시하므로, 검색 결과가 비어 있다는 사실만으로 몸소 전용 프로젝트라고 판정하지 않는다.

### S127: PC의 마지막 백업 확인

PC의 일반 `destroy`는 IAP SSH로 VM 내부의 유지보수 문과 파일 잠금을 잡고, 문을 닫은 뒤 연결된 백업의 성공을 확인한다. 확인 후 VM이 삭제될 때까지 문을 유지하며, VM이 실행 중이 아니거나 접속할 수 없으면 `vm_unreachable`로 중단한다. 백업 미연결은 `backup_not_connected`, 최신성 미확인은 `backup_freshness_unverified`다. 삭제가 실패하고 VM이 계속 실행 중인 것으로 확인되면 문을 다시 열고 실패를 반환한다. 삭제 결과나 문 열림을 확인할 수 없으면 `maintenance_release_unconfirmed`로 남기며 성공이라고 표시하지 않는다. 연결이 끊겼다면 삭제 작업의 종료 여부와 VM 상태를 확인하기 전에는 수동으로 문을 열지 않는다. 원격 임시 상태는 해당 VM의 `/run/momso-pause-<설치 ID>-<임의값>`에 있다.

이 연결은 PC 경로의 가짜 원격 및 잠금 시험 범위다. 휴대폰 일반 쉬기, 실제 Google 자원 삭제, 전부 지우기와 과금 종료 확인은 별도 작업이다. 아래 과거 작업 기록의 PC 차단 설명은 이 절로 대체한다.

PC와 휴대폰의 두 삭제 방식은 아직 완성되지 않았다. 특히 **전부 지우기 실행, 백업 보존 상태의 최신성 확인, 잔여 자원과 과금이 끝나는 시각의 실제 조회·재시도**는 연결하지 않았다. 프로젝트를 자동으로 닫지 않는다. 구현과 확인이 끝나기 전에는 화면에서 이 계약을 완료된 삭제 기능으로 표시하지 않는다.

### 2026-11-24: 삭제 진입점 임시 차단

휴대폰 `destroyFromGesture`는 VM에 닿을 수 없어 `vm_unreachable`로 멈추고, PC 설치기 `destroy`의 anywhere 경로와 `momso_anywhere_destroy_cli.mjs`는 마지막 백업 확인 통로가 완성될 때까지 `backup_freshness_unverified`로 멈춘다. 삭제를 시작하기 전에 멈추므로 이 경로로는 현재 VM을 제거하거나 비용을 줄일 수 없다. 화면은 VM 또는 마지막 백업을 확인할 수 없어 삭제하지 않았다고 알려야 한다. 시험용 정리 경로 등에서 내부 제공자를 직접 호출하면 이 차단을 거치지 않으므로 고객 설치에 사용하면 안 된다. 실제 서버에서 쓰기 중지와 백업 확인을 잡고 VM 제거까지 유지하는 통로는 아직 없다. 이 문서의 미리 보기나 과거 백업 시각은 그 확인을 대신하지 못한다.

명시적으로 VM 자료를 버리는 선택은 PC `--discard-data`, 휴대폰 `discardData: true`다. 두 경로는 위 S120b 절의 백업 정리를 실행한다. 연습 S117a/c는 시험 설치의 자료를 버리도록 이 값을 넘긴다. 화면은 이 선택을 되돌릴 수 없음을 확인받은 뒤에만 전달해야 한다.

S120a 작업 중: 서버의 유지보수 문을 처음 닫으면 기존에 최신이던 백업도 다시 실행하도록 변경했으며, 닫힌 시각을 `closedAt`으로 반환한다. 리눅스의 가짜 원격 통합 시험에서 문을 닫은 뒤 두 백업의 성공 시각이 갱신됨을 확인했다. 공통 절차는 켜진 백업 각각의 성공 시각과 이 시각을 비교한다. VM 내부에서는 기존 `ServerAdmission`을 사용해 같은 파일 잠금과 유지보수 문을 잡는 어댑터를 만들었고 리눅스 실제 파일 잠금 시험이 통과했다. PC·휴대폰 삭제기에서 VM 내부 어댑터를 호출하고 결과를 받는 경로는 아직 없으므로 일반 ‘쉬기’는 계속 거절한다. 시작 화면은 기존 앱의 로그인 세션과 다른 출처여서 인증된 서버의 유지보수 문에 바로 접근할 수 없다. 두 경로에서 같은 잠금을 잡아 삭제가 끝날 때까지 문을 유지하고, 연결이 끊긴 경우의 복구를 확인해야 한다.

기존 휴대폰 삭제는 백업 버킷을 남기면서 복원에 필요한 보관소 열쇠를 삭제했다. 기본 경로에서는 이제 `momso-vault-*`를 보존하고 `retained`에 이유를 돌려준다. 백업 버킷과 KMS는 이 삭제 단계에서 조회하지 않으므로 같은 목록에 `unverified`로 적는다. PC의 기존 경로도 이 열쇠를 삭제하지 않는다. 다만 마지막 백업 이후의 VM 변경이 백업됐는지 확인하는 단계가 아직 없어, 이 변경만으로 자료가 모두 안전하다고 판정할 수는 없다. 시험용 S117c는 별도 소유권 확인 후에만 시험용 열쇠도 지운다.

## 준비된 미리 보기

- 휴대폰: `deletionPreviewFromGesture({clientId, release, projectId, installationId, ...})`. 기존 인증 복귀를 거친 결과는 `action: "deletion_preview"`, 값은 `value`에 있다.
- PC: `python scripts/momso_server_install.py delete-preview --account <계정> --project <프로젝트> --state <기존 설치 기록>`.
- 공통 값: `backupExists`는 `true`, `false`, `null`(미확인), `lastBackupAt`은 마지막 백업 파일의 시각 또는 `null`이다. 파일 시각만으로 복원 가능성과 최신성을 증명하지 못하므로 `lastBackupValidity`를 함께 준다. 소유권이 다른 백업·키는 거절한다.
- `estimatedMonthly`는 달러 기준이다. 확인된 소프트웨어 KMS 키 버전은 시간당 가격 × 730시간으로 계산한다. 휴대폰은 서울 리전의 표준 저장소에서 모든 현재·이전·소프트 삭제 세대의 용량과 등급을 확인한 경우에만, 그 용량이 한 달 내내 유지된다는 가정으로 저장비와 부분 합계도 계산한다. PC는 아직 이전 세대 전체를 확인하지 못해 저장비를 미확인으로 둔다. Secret Manager 열쇠의 보관과 작업, 네트워크 등의 비용은 포함하지 못해 전체 합계는 `null`이다. `projectClosure`는 다른 자원이 없는지 증명하기 전까지 `unverified`다.
- 시험용 S117c의 백업 정리 결과에는 같은 `deletion` 틀을 일부 사용한다. 전체 자원과 소프트 삭제가 증명되지 않아 `billingZero`는 `null`이다. 이것은 고객용 전부 지우기가 완성됐다는 뜻이 아니다.

## 요금과 남는 자료

[Google KMS 요금표](https://cloud.google.com/kms/pricing)는 사용·사용 중지·파기 예약 중인 키 버전을 과금 대상으로 다루고, 파기된 버전은 과금하지 않는다. 소프트웨어 대칭 키 버전 하나의 게시 가격은 시간당 USD 0.000082192이며 730시간으로 약 USD 0.06이다. 실제 금액은 사용 시간과 계약·무료 혜택에 따라 달라질 수 있다.

[Cloud Storage 가격표](https://cloud.google.com/storage/pricing)에서 서울(`asia-northeast3`)·Standard의 게시 가격은 GiB당 시간당 USD 0.000031507, 730시간 기준 약 USD 0.023이다(2026-11-24 확인). 저장 위치·등급·용량, 작업과 네트워크에 따라 실제 요금은 달라진다. [소프트 삭제 안내](https://docs.cloud.google.com/storage/docs/soft-delete)에 따르면 보관 기간 동안 삭제된 객체에도 저장 요금이 나올 수 있고, 보관 기간이나 삭제 중지 정책에 따라 종료 시각을 알 수 없을 수 있다. [Secret Manager 가격표](https://cloud.google.com/secret-manager/pricing)도 보관된 열쇠의 비용 확인에 필요하다. [KMS 삭제 안내](https://docs.cloud.google.com/kms/docs/delete-kms-resources)는 키·키링 삭제에 하위 리소스 등의 조건을 둔다. 따라서 삭제 요청을 보냈다는 이유로 즉시 0원이나 확정 종료 시각을 표시하지 않는다.

## S129a2 입구 호출 계약

앱과 같은 출처에서 소유자 세션으로 `POST /__momso/pause/prepare`, `GET /__momso/pause/status`, `POST /__momso/pause/owner-release`, `POST /__momso/pause/reissue`를 부른다. 세션이 없으면 `401`과 `pause_requires_app_session`을 반환한다. 준비 성공은 `ok`, `ticket`, `expiresIn`을 반환한다. 상태의 `state`는 `idle`, `preparing`, `prepared`, `confirmed`, `recovering`, `releasing`, `failed`다.

공통 로그인 출처는 쿠키 없이 `POST /__momso/pause/confirm` 및 `POST /__momso/pause/release`를 부른다. JSON 본문은 `{"ticket":"..."}`이며, 확정에 성공하면 일회용 `releaseTicket`을 돌려준다. 재접속한 소유자의 `reissue`는 새 확인 표를 반환한다. 표는 주소의 조각 또는 요청 본문에만 두고 서버 기록에 쓰지 않는다. 다른 출처는 거절하며 CORS에 자격 증명을 허용하지 않는다. 까닭은 `backup_freshness_unverified`, `backup_not_connected`, `vm_unreachable`, `maintenance_in_progress`, `maintenance_release_unconfirmed`, `invalid_pause_ticket`, `invalid_pause_state`다.

이 호출만으로 휴대폰 쉬기가 완성된 것은 아니다. 시작 페이지가 조각의 표를 받아 확정하고 VM 삭제 결과에 따라 해제하는 경로는 다음 단계다. 입구 코드가 바뀌었으므로 실제 적용에는 새 입구 이미지와 서명된 배포 순번이 필요하다.

## 화면 담당이 나중에 붙일 것

- 삭제 전에 백업 존재·파일 시각·복원 검증 여부를 보여 준다. `null`은 확인하지 못한 것으로 표시한다.
- ‘쉬기’와 ‘전부 지우기’는 실제 실행 계약이 완성된 뒤 구분한다. 전부 지우기에는 별도 확인이 필요하다.
- 부분 추정 금액과 전체 미확인을 구분하고, 보관 중인 열쇠와 과금 가능한 삭제 보존본을 설명한다.
- `projectExclusivity.status`가 `not_exclusive`면 다른 자원이 발견됐다고 설명하고 프로젝트 닫기를 제안하지 않는다. `unverified`면 판별하지 못했다고 표시한다. 자원 종류(`observedTypes`), 확인 상태(`checks`), 켜진 API와 설치기 목록 밖 API(`enabledServices`, `unattributedServices`), 제한사항(`limitations`)을 함께 보여 준다. 설치기 목록 밖 API가 있다는 사실만으로 다른 사람이 켰다고 단정하지 않는다. 자동으로 프로젝트를 닫지 않는다.

## S120c 프로젝트 확인

PC는 `python scripts/momso_server_install.py project-exclusivity --account <계정> --project <프로젝트> --state <설치기록>`으로 호출한다. 기존 `delete-preview`에도 `projectExclusivity`가 들어간다. 휴대폰은 `deletionPreviewFromGesture`의 결과에서 같은 필드를 읽거나 공급자의 `projectExclusivity(id)`를 호출한다. 프로젝트를 만들거나 API를 켜거나 닫는 명령은 없다. PC는 Google Cloud CLI와 Node.js가 필요하다.

Cloud Asset Inventory 검색으로 실제 반환된 자원 종류와 라벨·이름을 살피고, Service Usage에서 활성 API 목록을 별도로 확인한다. 다른 설치 ID 라벨이나 다른 이름의 VM·버킷은 `not_exclusive`다. 목록에 없는 종류, 검색 지연, IAM 정책과 외부 청구를 확인할 수 없어 나머지는 `unverified`이며, 이 구현은 아직 `exclusive`를 반환하지 않는다. [Cloud Asset Inventory 검색 안내](https://docs.cloud.google.com/asset-inventory/docs/search-resources)는 모든 자원 종류가 검색되는 것은 아니라고 밝힌다. API 목록은 설치기가 켜는 일곱 종류와 대조하되, Google 기본 API 등 목록 밖 API의 활성화 주체는 알 수 없다. 필요한 읽기 권한이나 Asset Inventory API가 없으면 해당 확인은 `unverified`다. 이 확인을 위해 API를 새로 켜지 않는다.

[Google 프로젝트 종료 안내](https://docs.cloud.google.com/resource-manager/docs/delete-restore-projects)에 따르면 종료 요청 후 프로젝트는 즉시 사용할 수 없고 30일 동안 복원 요청이 가능하다. 다만 일부 저장소·메시징 자원은 더 일찍 삭제되어 프로젝트를 복원해도 돌아오지 않을 수 있다. 종료 시 결제 연결은 해제되며 복원해도 자동으로 다시 연결되지 않는다. 청구 주기 말까지 비용이 발생할 수 있고 약정 할인에도 영향이 있을 수 있으므로 ‘즉시 요금 0’이라고 약속하지 않는다. 프로젝트 종료는 사용자의 별도 결정이며 이 기능은 종료 호출을 제공하지 않는다.

## 남은 일

1. VM의 마지막 작업까지 백업했는지 확인하고 안전하게 멈춘 뒤 ‘쉬기’를 실행한다.
2. 명시적 확인 뒤 백업·키 삭제를 재시도 가능하게 연결하고, 보존 기간·소프트 삭제·파기 예약과 남은 비용을 실제로 조회한다.
3. PC와 휴대폰에 같은 삭제 결과를 연결하고, S117a/c 시험 정리도 공통 계약으로 옮긴다.
4. 전용 프로젝트 여부와 다른 자원 유무를 확인하는 안전한 조회와 가짜 시험을 완성한다.
# S132: 삭제된 역할과 다시 시작

쉬기는 맞춤 역할을 무료로 남기고, 전부 지우기는 역할도 지운다. 계정을 지우기 전 프로젝트, VM, 입구, 해당 서비스 계정, 백업 버킷, KMS 키·키링, vault 및 전송 비밀 정책에서 해당 계정과 `deleted:` 잔재만 제거하고 재조회한다. 소유권이나 정책을 확인하지 못하면 계정 삭제를 중단하고 `iam_bindings_unverified`로 남긴다. 다시 설치할 때 새 계정에 권한을 다시 건다.

설치 시 소유한 삭제 역할은 복구하고 권한을 맞춘다. Google은 삭제 후 7일 안에만 역할 복구를 허용하며, 그 뒤 완전 삭제까지 최대 44일 동안 같은 ID를 새로 만들 수 없는 기간이 있다. 그 기간에는 재설치가 실패할 수 있다. 삭제된 서비스 계정을 같은 이름으로 새로 만들면 다른 식별자다. 가짜 시험으로 확인했으며 실제 고객 프로젝트 삭제·재설치는 별도 검증이다.

출처: [Google Cloud 맞춤 역할 생성·삭제·복원](https://docs.cloud.google.com/iam/docs/creating-custom-roles), [서비스 계정 삭제·복원](https://docs.cloud.google.com/iam/docs/service-accounts-delete-undelete).
