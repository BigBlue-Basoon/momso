# S90: GitHub 릴리스 찾기

`github_releases.py`는 GitHub 공개 릴리스 목록을 인증 없이 조회한다. `development`는 사전 공개, `production`은 정식 공개만 고르며 초안과 서명 목록이 없는 릴리스는 제외한다. 공개 시각이 가장 늦은 릴리스의 `release.json`과 `release.sig` 또는 `release.json.sig`를 기존 `Updater`에 전달한다. 목록을 한 번에 100개씩 최대 10쪽 읽고 그보다 많으면 잘못 최신이라고 판단하지 않고 오류로 둔다. 서명·채널·묶음 지문과 설치된 순번의 비교는 기존 `Updater`가 담당한다. 서버가 바쁘면 기존 유지보수 대기 규칙을 따른다.

`ReleasePoller.run_forever`의 기본 간격은 1시간이고, 실패하면 다음 간격에 다시 시도한다. 재시작 뒤에는 먼저 중단된 업데이트를 복구한다. 기본 간격의 공개 API 요청은 한 VM당 보통 하루 24회다. 공유 IP의 다른 요청 때문에 GitHub 제한에 걸릴 수 있으며 이때는 실패로 표시하고 다음 주기에 재시도한다. [GitHub 릴리스 목록](https://docs.github.com/en/rest/releases/releases#list-releases), [GitHub REST 요청 한도](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api).

화면에 전달할 진행 값은 기존 S38 계약의 `checking`, `up_to_date`, `downloading`, `waiting_for_idle`, `backing_up`, `installing`, `updating_gateway`, `completed`, `rolling_back`, `rolled_back`, `recovery_required`, `failed`와 `{state, reason}`을 그대로 사용한다. 이유는 `release_discovery_unavailable`, `release_list_invalid`, `release_list_too_large` 등 기계가 읽는 값이다. 토큰이나 응답 본문은 넣지 않는다.

## 실제 설치 연결 상태

`layout_migration.py`는 서명된 묶음의 서버·화면과 실제 설치된 파일이 일치할 때만 해당 순번으로 기준 사본을 준비하며, 실행 중인 원래 파일은 바꾸지 않는다. `install_worker.py`는 이 사본을 systemd에서 사용하도록 준비하고 실패 시 기존 서비스 설정을 복원한다. `worker.py`는 독립 서비스에서 확인·복구·주기적 갱신을 수행한다. PC 설치기는 `scripts/momso_anywhere_update_install.py`로 코드와 채널·공개 키를 VM에 전달하고 작업자를 등록한다. 수동 업데이트도 활성 버전의 파일과 순번을 사용하며 실패 시 이전 순번을 복원한다.

`GET /update/status`는 인증된 입구에서 `{state, reason}`을 읽으며 서버가 잠시 꺼져도 독립 프록시가 켜져 있으면 답한다. 인증되지 않은 요청은 거절한다. 새 프록시와 서비스 설정은 위 설치 부품이 적용할 때 비로소 동작한다.

휴대폰에서 새 VM을 만드는 시작 스크립트도 서명된 묶음에 포함된 작업자와 고정된 채널·공개 키를 설치하고, VM 서비스 계정에 해당 Cloud Run 서비스 갱신과 서비스 계정 사용 권한을 부여한다. 다른 프로젝트의 비공개 이미지 저장소 읽기 권한은 고객 프로젝트의 설치기만으로 부여할 수 없다. 해당 저장소 소유자가 접근을 허용해야 한다. [Cloud Run 배포의 공식 권한](https://cloud.google.com/run/docs/deploying#required-roles). 실제 고객 VM이나 실제 이미지 저장소에 대한 배포는 검증하지 않았다.

개발용 VM에서 `gcloud`가 설치되어 있는 것은 확인했다. 이것은 고객 VM의 도구 설치나 실제 입구 배포 권한을 확인한 결과가 아니다. Linux의 격리 시험에서는 기준 사본 준비·중단 뒤 준비 재개·서명된 기존 판 대조, 인증된 진행 상태 조회와 실제 systemd를 이용한 가짜 릴리스 교체·복구를 확인했다. 네트워크가 차단된 가짜 VM 파일 시스템에서는 휴대폰 시작 스크립트로 서명된 묶음을 설치하고 업데이트 서비스를 준비했다. 고객 VM이나 실제 배포처는 건드리지 않았다.
