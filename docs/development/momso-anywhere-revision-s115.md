# S115: Cloud Run 새 판 선택과 실제 요청 확인

본부의 실제 상태: `latestCreatedRevisionName`은 방금 만든 0% 판이었고 `latestReadyRevisionName`은 옛 트래픽 판이었다. 기존 코드는 후자를 골라 옛 판으로 100%를 다시 보냈다. 이전 해시 검증은 이를 `gateway_image_mismatch`로 잡아 되돌렸다.

설치기와 VM 작업자는 배포 전 `latestCreatedRevisionName`을 기억하고 배포 후 달라진 이름의 실제 `status.imageDigest`, Ready를 확인한다. 브라우저도 v2의 `latestCreatedRevision`을 사용하고 Ready 조건과 지문을 확인한다. 퇴역 상태여도 Ready인 판은 허용한다. 100% 전환 뒤 트래픽과 지문을 다시 보고, 입구의 새 공개 `GET /__momso/ready`에 실제 HTTPS 요청을 최대 3번 보낸다. 200과 `{"ready":true}`가 아니면 실패로 보고 이전 트래픽 복구를 시도한다. 새 확인 경로는 로그인 전 사용 가능하고 CORS를 허용하지만 사용자 자료를 돌려주지 않고 VM을 깨우지 않는다. **입구 프로세스의 응답만 확인하며 VM·채팅의 정상 작동은 별도 시험이다.** 최초 설치의 아직 비공개인 중간 단계는 공개 HTTPS 검사를 건너뛰고, 다음 공개 단계에서 검사한다.

**본부가 실제 재시험하기 전:** 이 검사는 새 입구 이미지의 `/__momso/ready`를 요구한다. 기존에 서명된 옛 이미지에는 해당 경로가 없다. 이 커밋의 입구 이미지를 새로 만들고 공개한 뒤, 그 이미지 지문을 넣은 새 목록을 열쇠 소유자가 서명해야 한다. 옛 순번 4 목록이나 옛 이미지로 다시 시도하면 안전하게 실패·복구하는 것이 정상이다. 고객 VM·실제 입구는 이 작업에서 건드리지 않았다.

Google은 새 판에 100%를 보낼 때 Cloud Run이 인스턴스를 준비한다고 설명한다([autoscaling](https://docs.cloud.google.com/run/docs/about-instance-autoscaling)). 조건 문서는 `RETIRED`를 퇴역 판으로 구분한다([conditions](https://docs.cloud.google.com/run/docs/reference/rest/v2/Condition)). 0% 퇴역 판의 재활성화와 실패 표현을 모든 상황에서 보장하는 별도 명세는 확인하지 못했으므로, 실제 트래픽·HTTP 검사로 실패를 감지한다. 공개 HTTP 검사는 콜드 스타트·네트워크 실패도 포함해 완료를 막으며, 복구 자체가 실패하면 별도로 보고한다.

0% 판은 자동으로 지우지 않는다. 이 설치가 만든 판인지와 다른 복구 용도인지 확실하지 않으면 남의 판을 지울 수 있기 때문이다. Google은 [서비스당 최대 1,000개를 넘으면 오래된 판을 자동 삭제](https://docs.cloud.google.com/run/docs/managing/revisions)한다고 안내한다. 00007~00013을 이 코드가 삭제하지 않으며, 본부가 소유·태그·트래픽·직전 정상 복구 판을 검토한 뒤 별도로 정리한다.

시험: 가짜 서비스에서 옛 판은 latestReady와 100%, 새 판은 latestCreated와 Ready/Retired이며 나머지 0%. 새 지문이 맞을 때만 새 판으로 전환한다. 요청 실패, 지문 불일치, 트래픽 변경 무시와 복구 실패도 가짜로 확인한다. 실제 Cloud Run·실제 브라우저 CORS·새 서명 이미지 시험은 본부 몫이다.
# S131: 캐시 주소와 지문

Cloud Run은 이미지 주소를 `cache.<region>-docker.pkg.dev/ghcr.io/...`로 기록할 수 있다. PC·VM 작업자와 휴대폰 시작은 주소 접두사가 아니라 SHA-256 지문을 비교한다. OCI index 지문과 그 안의 플랫폼 manifest 지문은 서로 다르므로 둘을 같다고 보지 않는다. 단일 플랫폼 manifest를 서명 목록에 지정해야 한다. 이는 가짜 서비스 시험이며 실제 배포 확인과는 다르다.

# S132c: 첫 설치

Cloud Run CLI는 서비스가 없는 첫 배포에서 `--no-traffic`을 받지 않는다. 첫 배포에는 이 옵션을 넣지 않고, 조회된 기존 서비스의 갱신에서만 넣는다. 새 서비스는 지문·트래픽을 확인하고 공개 후 HTTPS를 확인한다. 첫 설치가 실패하면 설치 ID 소유 표식을 재확인한 후 새 서비스를 삭제하며, 정리를 확인하지 못하면 실패를 보고한다. 이미 있던 서비스는 삭제하지 않는다.

휴대폰 REST 생성은 `traffic`을 지정하지 않는다. Google Cloud Run v2 문서는 비어 있거나 지정하지 않은 트래픽이 최신 준비된 판에 100% 배정된다고 설명한다: [Cloud Run 서비스 REST](https://docs.cloud.google.com/run/docs/reference/rest/v2/projects.locations.services). 그 뒤 기존 서비스 갱신에서 고정 판을 검사한다. 가짜 시험과 실제 클라우드 확인은 별도다.

기존 IAP SSH 재시도는 최대 12회, 사이 10초이며 맞춤 IAM 역할 전파 지연은 최대 12회, 사이 5초다. 이번 변경은 대기 횟수를 늘리지 않는다. 제한 이후에는 실패를 알리고 같은 설치 기록으로 다시 시도한다.
