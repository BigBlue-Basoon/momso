# momso anywhere: 고객 프로젝트의 구글 비용 조사

확인일: 2026-11-25. 금액은 미국 달러, 세금·환율·계약 할인 전이다. **아래는 확정 청구액이 아니다.** 서울 VM과 표준 백업 저장 단가는 확인했지만 실제 저장량과 사용량을 아직 확인하지 못했으므로 월 전체 합계도 미확인이다. 고객의 다른 프로젝트가 같은 결제 계정의 무료량을 쓰면 결과가 달라진다. AI 회사의 별도 이용료와 인터넷 전송료도 추가된다.

## 설치되는 것과 상태

PC 설치 코드 `scripts/momso_server_install.py`의 `prepare_network`, `prepare_vm`, `scripts/momso_anywhere_install.py`의 입구 설치, `scripts/momso_anywhere_backup_provision.py`를 기준으로 읽었다. 휴대폰 설치는 `scripts/momso_anywhere_google_browser.mjs`와 `scripts/momso_anywhere_vm_startup.mjs`도 확인해야 한다. 설치가 중간에 실패하거나 이전 버전에서 바뀐 자원은 실제 조회 결과가 우선이다.

| 자원·설정 | 사용 중 | VM 유휴 종료 | 쉬기 성공 뒤 | 전부 지우기 성공 뒤 |
|---|---|---|---|---|
| 서울 e2-small, Debian 12 VM, 외부 IP 없음 | 실행 | VM은 존재하나 정지 | 삭제 | 삭제 |
| 부팅 디스크 20GB | 존재 | 계속 존재·과금 | VM과 함께 삭제를 의도, 실제 삭제 결과 확인 | 삭제 확인 필요 |
| Cloud Run 입구 | 존재·요청 처리 | 존재·요청 처리 | 삭제 시도 | 삭제 시도 |
| VPC, 서브넷, 방화벽, Cloud Router·자동 할당 Public NAT | 존재 | 존재; 본부의 실제 설치 확인에서 NAT 자동 IP는 VM 정지 뒤 반환됨. 최종 청구서 확인 필요 | 삭제 시도, 주소 회수 대기로 남을 수 있음 | 삭제 시도, 주소 회수 대기로 남을 수 있음 |
| 암호화 백업 버킷, KMS 키와 버전 | 존재 | 존재 | 보존 | 삭제·파기 요청; 보존 기간 및 파기 대기 상태 확인 |
| Secret Manager 비밀값 | 통로·세션·금고용 | 존재 | 복원에 필요한 금고 값은 보존 | 삭제 시도 |
| 서비스 계정·맞춤 역할·IAM 연결 | 설치된 권한 사용 | 존재 | 복원에 필요한 권한 일부 보존 가능; 실제 결과 확인 | 삭제 시도 |
| 로깅·저장소 요청·네트워크 송신 | 사용량에 따라 발생 | 입구·점검·저장된 로그에 따라 발생 | 남은 백업과 로그에 따라 발생 | 과거 로그·보존된 객체가 있으면 발생 가능 |

자원 목록 및 삭제 순서는 `scripts/momso_anywhere_install.py`, `scripts/momso_anywhere_backup_provision.py`, `scripts/momso_server_install.py`, `scripts/momso_anywhere_vm_services.py`, `scripts/momso_anywhere_google_browser.mjs`를 함께 볼 것. 삭제 결과에서 `remaining`, `billable`, `unverified`를 확인하기 전에는 어느 상태도 0원으로 단정하지 않는다.

## 직접 확인한 공식 가격과 무료 조건

| 항목 | 가격·조건 | SKU와 근거 |
|---|---|---|
| e2-small | 서울 종량제 시간당 $0.02149143. e2-small은 2개의 공유 vCPU와 2GiB 메모리이며 메모리를 별도 계산하지 않는 공유 코어 가격이다. 공식 가격표의 서울 선택값에 연결된 시간당 표를 확인했다. | 서울 e2-small 세부 SKU 미확인. [머신 종류](https://docs.cloud.google.com/compute/docs/general-purpose-machines), [범용 VM 가격의 서울 항목](https://cloud.google.com/products/compute/pricing/general-purpose). 무료 VM은 미국 일부 지역의 e2-micro 조건이므로 이 설치에 적용하지 않는다. |
| 20GB 디스크 | 서울 표준 영구 디스크 사용 시 $0.000071233/GiB·시간(월 표시 $0.052/GiB). 본부가 실제 설치에서 pd-standard 20GB를 확인했다. VM이 멈춰도 디스크는 남는다. | SKU 미확인. [디스크 가격의 서울 선택값](https://cloud.google.com/compute/disks-image-pricing), [gcloud 기본값](https://docs.cloud.google.com/sdk/gcloud/reference/compute/instances/create), `prepare_vm` |
| Public NAT | VM 32개 이하는 게이트웨이를 쓰는 VM당 시간당 $0.0014, 처리량 GiB당 $0.045, NAT 외부 IP당 시간당 $0.005. 인터넷 전송료는 별도다. | Google의 SKU **그룹**: IP `8515-9425-D2CE`, 처리 `015F-5732-FFF0`, 가동 `32E2-4EFC-EF9F`. 개별 청구 SKU는 미확인. [NAT 가격](https://cloud.google.com/nat/pricing) |
| Cloud Run 입구 | 요청 기반 과금 무료 기준은 월 200만 요청, 180,000 vCPU초, 360,000 GiB초다. 결제 계정 전체 합산이며 Tier 1 단가에 상당하는 할인이다. 서울은 Tier 2이므로 표시량 이내라도 비용이 남을 수 있다. 본부가 실제 입구에서 최소 인스턴스 설정이 없음을 확인했다. 요청이 없으면 인스턴스는 0으로 줄어든다. | SKU 미확인. [Cloud Run 가격](https://cloud.google.com/run/pricing), [무료 조건](https://docs.cloud.google.com/free/docs/free-cloud-features) |
| 백업 버킷 | 서울 표준 저장은 $0.000031507/GiB·시간(월 표시 $0.023/GiB). 보관량·기간, 읽기/쓰기 요청, 전송, 삭제 보관 정책에 따라 과금. 무료 5GB-month 저장은 미국 지정 3개 리전만으로 서울 버킷에 적용하지 않는다. | Standard Storage Seoul SKU `59CF-0E01-97FE` **단, 실제 버킷 저장 등급 확인 필요**. [SKU 목록](https://cloud.google.com/skus/sku-groups/standard-storage-regional), [가격의 서울 선택값](https://cloud.google.com/storage/pricing), [무료 조건](https://docs.cloud.google.com/free/docs/free-cloud-features) |
| KMS 소프트웨어 대칭 키 버전 | 활성 버전당 시간당 $0.000082192, 암호 작업 10,000회당 $0.03. 사용 중지 및 파기 예약 버전도 활성으로 청구된다. 무료 100개 버전·1만 암호 작업은 Autokey가 만든 키에만 적용; 설치기는 일반 키를 생성한다. | SKU 미확인. [KMS 가격](https://cloud.google.com/kms/pricing), `BackupProvisioner` |
| Secret Manager | 결제 계정 월 활성 버전 6개·접근 10,000회 무료, 이를 넘는 비용 발생. 고객의 다른 프로젝트도 합산한다. | SKU 미확인. [가격](https://cloud.google.com/secret-manager/pricing), [무료 조건](https://docs.cloud.google.com/free/docs/free-cloud-features) |
| 네트워크와 주소 | VM 외부 IPv4를 쓰면 사용 중 시간당 $0.005. NAT 외부 IP도 $0.005이며 같은 주소를 NAT와 VPC에 중복 계산하지 않는다. 서버리스 내부 예약 주소는 외부 IP 비용과 혼동하지 않는다. | [VPC 가격](https://cloud.google.com/vpc/pricing), [NAT 가격](https://cloud.google.com/nat/pricing). 서비스 계정·IAM 역할 자체의 월 고정료는 이 조사에서 찾지 못함. |

위 가격 페이지의 자동 표시 지역이 서울이라고 가정하지 않았다. VM과 저장소는 공식 가격 페이지가 내보내는 지역별 표에서 서울 행을 별도 확인했다. Cloud Run 요청에 따른 실제 사용량, 비밀값 버전 수, 로그·백업 용량은 각각 확인이 필요하다.

## 월 계산: 30일, 720시간이라는 가정

`D` = 서울의 실제 20GB 디스크 월액, `B` = 서울 백업 저장 월액, `E` = 요청·암호 작업·전송·로그 등 변동액. 아직 미확인이다. 표준 백업이 `G` GiB로 720시간 일정하다면 저장량 요금 `B = G × 0.02268504` 달러다. 아래 수치는 월 합계가 아니라 **명시한 가정에 해당하는 확인된 일부 + 미확인 항목의 식**이다. 본부의 1회 실제 설치 관찰에 따라 NAT IP는 VM이 켜진 시간에만 있고, 사용 중인 VM만 NAT 이용 대상으로 시간당 계산되며, 소프트웨어 KMS 버전 하나가 720시간 활성이라고 가정한다. 실제 청구서와 KMS 버전 수는 확인이 필요하다.

| 상태 | 이 가정에서의 월 비용 식 USD | 확인된 일부 |
|---|---|---|
| 하루 2시간 사용 | `D + B + E + 1.7327` | VM $1.2895 + IP $0.30 + NAT 이용 $0.084 + KMS $0.0592 |
| 하루 8시간 사용 | `D + B + E + 6.7531` | VM $5.1579 + IP $1.20 + NAT 이용 $0.336 + KMS $0.0592 |
| 하루 24시간 사용 | `D + B + E + 20.1410` | VM $15.4738 + IP $3.60 + NAT 이용 $1.008 + KMS $0.0592 |
| 한 달 내내 VM 정지 | `D + B + E + 0.0592` | 본부가 VM 정지 뒤 자동 NAT IP가 사라짐을 확인했다. 그 관찰과 NAT VM 이용료가 없다는 가정이며 최종 청구서는 미확인이다. |
| 한 달 내내 쉬기 | `B + E + 0.0592` | KMS 한 버전이 계속 활성이라는 조건. 버킷과 금고 값은 남는다. |
| 전부 삭제 | 삭제와 보존 기간이 모두 끝나고 과금 자원이 없으면 $0 | 완료 전이면 미확인. 프로젝트 자체의 다른 사용분은 별개. |

KMS 시간당 가격 × 720 = $0.05917824. 월의 실제 시간과 환율·부가세에 따라 청구액은 다르다. 버킷에 보관되는 세대, 삭제 보호 기간, 다른 Cloud Run 설정과 청구 계정의 무료량 사용분을 알아야 숫자로 된 전체 합계를 만들 수 있다.

**계산 예시:** 디스크가 표준형 20GiB이고 백업이 표준형 1GiB로 720시간 일정하며, 위 NAT 조건과 KMS 1개가 맞고 `E=0`인 경우 디스크는 $1.0258, 백업은 $0.0227이다. 그러면 하루 2시간 $2.7811, 8시간 $7.8016, 24시간 $21.1894, 한 달 내내 VM 정지 $1.1076, 쉬기 $0.0819다. 요청·전송·백업 증가·무료량 소진 등이 있으면 더해진다. 이것은 실제 사용자의 청구액이나 0원 보장이 아니다.

## 줄이는 선택지

1. **무작정 NAT를 떼지 않는다.** 본부의 1회 관찰에서는 VM 정지 직후 자동 NAT IP가 반환되므로 정지 기간의 IP 요금을 따로 없앨 여지는 없다. 최종 청구서는 아직 확인하지 않았다. 실제 가용 시간·요금·NAT 재할당이 달라질 수 있어 절감액은 측정 전 미확인. NAT 게이트웨이 이용료와 트래픽은 별도다. VM 기동 시 NAT 재생성과 주소 확보가 성공해야 GitHub 릴리스·GHCR·AI API·사이트에 나갈 수 있다. 재생성 지연·실패와 재시도, 다른 작업과 경합, 입구를 통한 준비 상태를 실측해야 하며 추가 기동 시간은 **미확인**이다. 현재 서브넷의 Private Google Access는 본부 확인상 꺼져 있다. 별도로 켜면 Google API 일부는 [Private Google Access](https://cloud.google.com/vpc/docs/private-google-access)로 접속할 수 있지만 GitHub·GHCR·AI 사이트 전체를 대신하지 않는다.
2. **실행 중에만 VM 임시 외부 IP**는 해당 VM이 실행될 때 외부 IP 사용료가 생기고 정지 시 해제된다. NAT를 대체할 수 있는지 라우팅·방화벽과 실제 목적지별 시험이 필요하다. 외부 IP가 생기면 인터넷에서 도달 가능한 표면이 늘어나므로 방화벽, SSH 접근, 포트 노출을 별도로 검토해야 한다. [외부 IP 문서](https://docs.cloud.google.com/compute/docs/ip-addresses), [VPC 가격](https://cloud.google.com/vpc/pricing).
3. **선택형 빠른 유휴 종료**: `MOMSO_IDLE_SECONDS`를 서비스 환경 설정에 양의 정수 초로 넣으면 해당 시간 후 안전 조건이 충족될 때 종료를 시도한다. 기본은 기존 1800초이고 명령줄 `--idle-seconds`가 우선한다. 잘못된 환경 값은 종료 작업자가 시작을 거절한다. 진행 중 작업과 백업 확인, 유지보수 문은 그대로 적용된다. 설정을 지우면 기본으로 되돌릴 수 있다. 이 설정은 VM이 불필요하게 켜진 시간을 줄일 뿐 NAT IP 반환 후 실제 청구 여부는 청구서 확인이 필요하다. 실제 운영에 켜거나 휴대폰 설정 화면에 노출하지 않았다.

화면에서 정할 것: 상태별 예상 금액에 백업 용량·조건·확인 시각과 미확인 비용을 어디에 어떻게 보여 줄지 정한다.

## 가격 충실도 대조 기록

| 수치 | 원문과 대조 | 문서의 조건 |
|---|---|---|
| NAT $0.0014 / $0.045 / $0.005 | [공식 NAT 가격표](https://cloud.google.com/nat/pricing): 최대 32 VM, 게이트웨이 이용 VM·GiB·외부 IP 기준 각각 일치 | 본부의 1회 관찰에서는 VM 실행 때 할당하고 정지 뒤 반환. 실제 청구서는 미확인. IP 비용 이중 합산 없음. |
| KMS $0.000082192 / $0.03 | [공식 KMS 가격표](https://cloud.google.com/kms/pricing): 소프트웨어 대칭 버전 시간당·암호 작업 1만 회당 일치 | 무료 암호 작업은 Autokey 키에만 해당, 파기 예약도 활성. |
| Run 200만 / 180,000 / 360,000 | [무료 프로그램](https://docs.cloud.google.com/free/docs/free-cloud-features): 요청 기반의 요청·vCPU초·메모리초 일치 | [가격표](https://cloud.google.com/run/pricing)는 결제 계정 합산·Tier 1 단가 할인, 서울 Tier 2 명시. |
| Secret Manager 6 / 10,000 | [무료 프로그램](https://docs.cloud.google.com/free/docs/free-cloud-features): 월 활성 버전 및 접근 횟수 일치 | 결제 계정의 다른 프로젝트 사용량 포함. |
| Storage 5GB-month | [무료 프로그램](https://docs.cloud.google.com/free/docs/free-cloud-features): 미국 세 리전만 | 서울에 적용하지 않음. |
| 외부 IP $0.005 | [VPC 가격표](https://cloud.google.com/vpc/pricing): 일반 VM과 NAT 사용 외부 IP 각각 일치 | NAT 항목과 같은 주소 중복 계산 안 함. |
| 서울 e2-small $0.02149143 | [범용 VM 공식 가격](https://cloud.google.com/products/compute/pricing/general-purpose)의 서울 선택 표: `e2-small`, 기본 종량제 시간당 값 일치 | 공식 페이지의 지역별 원자료에서 서울 표와 값 연결을 대조했다. |
| 서울 표준 저장 $0.000031507/GiB·시간, $0.023/GiB·월 | [공식 저장소 가격](https://cloud.google.com/storage/pricing)의 서울 선택 표와 일치 | 720시간 계산은 시간당 단가를 사용하며, 월 표시값을 720시간에 그대로 적용하지 않는다. |
| 서울 표준 영구 디스크 $0.000071233/GiB·시간, $0.052/GiB·월 | [공식 디스크 가격](https://cloud.google.com/compute/disks-image-pricing)의 서울 `Standard provisioned space` 표와 일치 | 본부가 실제 설치에서 pd-standard 20GB 확인. |
| 60 / 240 / 720시간 | 30일 × 하루 2 / 8 / 24시간 산술 | 실제 월 길이 아님. |

미확인: e2-small 세부 SKU, 고객의 백업 크기와 저장 등급, Secret 버전 수, NAT IP 반환 뒤 실제 청구, NAT 재구성에 드는 시간, Cloud Run 실제 사용량, 고객별 무료량 및 환율·세금. 이를 확인하기 전에는 숫자로 된 전체 월 요금을 약속하지 않는다.

본부의 실제 관찰(1회): VM을 켤 때 자동 NAT IP가 생기고 끈 직후 주소 목록과 NAT IP 조회에서 사라졌다. 삭제한 입구가 쓰던 SERVERLESS 주소는 삭제 후 110~125분 사이에 반환됐다. 이는 관찰 범위이며 Google의 반환 시간 보장이 아니다.
