정리본: [momso-anywhere-guide.md](momso-anywhere-guide.md)

# 입구 이미지 배포 S72

PC 설치기는 서명된 목록의 `gatewayImage` 지문을 사용한다. `--gateway-image`를 별도로 주면 서명된 값과 같아야 한다. 서명과 입력 파일을 확인하기 전에는 자원을 만들지 않는다. 고객 프로젝트에 공개 저장소를 만들거나 실제 이미지를 업로드하지 않는다. PC 설치와 업데이트는 S48 `CloudRunGateway`로 새 revision을 무트래픽으로 준비하고 준비된 revision으로 전환한다. 갱신 중 문제가 나면 이전 트래픽을 복원한다. 서버·화면 갱신 도중 입구 갱신이 실패하면 서버·화면 복구도 시도한다. 기존 입구의 URL은 확인하며 바꾸지 않는다.

휴대폰은 Python과 gcloud를 실행할 수 없어 `momso_anywhere_cloud_run_revision.mjs`가 같은 revision 준비·트래픽 전환·복귀 계약을 Google REST로 구현한다. 서명된 목록의 `gatewayImage` 지문을 쓰고, 준비된 revision의 이미지도 확인한다. 복구 요청까지 실패하면 `gateway_rollback_failed`를 반환한다. 첫 생성 때는 인증 검사를 적용하는 입구를 비공개로 만든 뒤 URL을 얻고, 준비된 revision을 공개한다. 실제 Cloud Run에서 이 새 경로를 아직 시험하지 않았다.

## 서명과 화면 자산

**지문은 이미지 내용 고정이지 서명 검증이 아니다.** S73에서 PC 설치와 S48 자동 업데이트는 같은 Ed25519 목록 검증 함수를 사용하며, 휴대폰은 같은 공개키와 원본 목록 바이트를 WebCrypto로 검증한다. [S73 계약](momso-anywhere-signed-install-s73.md)을 따른다. 실제 공개 서명 키·서명된 목록·공개 레지스트리는 아직 마련되지 않았다.

입구 이미지는 로그인 전 제공할 manifest와 아이콘을 포함해야 한다. 이전 소스 배포는 선택된 화면 묶음에서 그 파일을 함께 넣었지만, 미리 만든 이미지는 배포할 때 새로 넣지 않는다. 릴리스 제작 단계에서 이미지의 공개 자산이 화면 묶음과 일치하는지 확인해야 한다. 그 확인은 이 설치기에서 아직 자동으로 하지 않는다.

## 고객 프로젝트 권한과 API

- 고객 프로젝트의 Cloud Run 서비스 에이전트 `service-<고객 프로젝트 번호>@serverless-robot-prod.iam.gserviceaccount.com`이 다른 프로젝트의 **비공개** Artifact Registry 이미지를 읽을 수 있도록 이미지 저장소에 `roles/artifactregistry.reader`가 필요하다. 배포자에게도 해당 저장소 읽기 권한이 필요하다. 서비스 에이전트와 VM·입구 실행 계정은 서로 다르다. [Cloud Run과 Artifact Registry](https://docs.cloud.google.com/artifact-registry/docs/integrate-cloud-run), [Cloud Run 배포 권한](https://docs.cloud.google.com/run/docs/deploying).
- 공개 저장소라면 저장소에 `allUsers`의 `roles/artifactregistry.reader`를 부여한다. 이는 저장소의 이미지가 누구에게나 공개된다는 뜻이다. 공개 여부는 아직 결정하지 않았고 여기서 변경하지 않는다. [Artifact Registry 접근 제어](https://docs.cloud.google.com/artifact-registry/docs/access-control).
- 미리 만든 이미지를 쓰므로 고객 프로젝트에서 Cloud Build와 소스 업로드가 필요하지 않다. 휴대폰 설치의 API 활성화 목록에서도 고객 프로젝트의 Artifact Registry API를 뺐다. **이미지 저장소가 있는 프로젝트**에서는 Artifact Registry API가 필요하다. 고객 프로젝트가 곧 저장소 프로젝트인 시험에서는 그 프로젝트에 API가 이미 활성화돼 있어야 한다. 이미 켜진 API나 다른 자원의 권한을 비활성화하지 않는다. [Cloud Run 소스 배포](https://docs.cloud.google.com/run/docs/deploying-source-code), [Artifact Registry API](https://docs.cloud.google.com/artifact-registry/docs/enable-service).
- Cloud Run, Compute Engine, Secret Manager, IAM 및 Service Usage 등 설치에 필요한 나머지 API와 권한은 그대로다. Cloud Build 역할도 이 이미지 배포 단계에서는 추가하지 않는다. 서버·VM의 별도 권한은 변경하지 않는다.

## 시험

`scripts/test_momso_anywhere_s68.sh`는 가짜 Google과 시험용 서버·화면으로 휴대폰 revision 전환과 PC 업데이트의 어댑터 호출을 포함한다. 별도의 어댑터 시험은 지문 없는 이미지 거절, 새 revision 실패 시 이전 트래픽 복귀를 확인한다. 실제 클라우드 레지스트리의 서비스 에이전트 권한·배포·서명은 시험하지 않았다.

2026-11-22 개발용 리눅스 VM에서 코드 `3f09af539`의 연결 흐름과 관련 시험 89개가 통과했다. 가짜 Cloud Run·가짜 레지스트리이며 실제 고객 설치의 검증 결과는 아니다.
