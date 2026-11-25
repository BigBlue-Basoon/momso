# momso anywhere 운영 채널 준비

이 문서는 운영 릴리스를 준비하는 절차다. 실제 운영 개인키 생성·공개키 등록·이미지 발행·릴리스 발행은 아직 하지 않았다. 개발 채널 시험이 운영 채널의 실제 검증을 대신하지 않는다.

## 개발자가 직접 실행할 두 명령

다음 명령은 지정한 운영 열쇠 보관 폴더 아래 `primary`, `recovery-1`, `recovery-2` 폴더에 각각 개인키와 공개키를 만든다. 폴더가 이미 있으면 덮어쓰지 않고 실패한다. 예비 두 폴더는 각각 이 PC가 아닌 다른 곳에 보관해야 한다. 개인키 값은 출력하지 않는다. 레포 안 경로는 거부한다. 실제 운영 키와 운영 릴리스는 아직 만들지 않았다.

```powershell
python scripts/momso_release_sign.py init-production --directory "<운영 열쇠 보관 폴더 경로>"
```

생성된 개인키는 암호화되지 않은 파일이다. 폴더는 Windows에서 현재 사용자와 시스템만 접근하도록 ACL을 설정한다. 개발자는 개인키를 안전하게 보관·백업하고 AI, 저장소, 대화, 로그에 넣지 않는다. 공개키 셋만 본부에 전달한다. 예비 키는 평상시 릴리스 서명에는 필요 없다.

아래는 운영 공개키가 저장소에 반영된 뒤 실행하는 한 줄이다. 꺾쇠 안은 실제 경로와 값으로 바꾸고, `--private-key`에는 파일 경로만 넣는다. 운영 서명 명령은 Docker 없이 GHCR의 익명 HTTP API로 공개 이미지의 단일 manifest와 지문을 반드시 확인한다. 네트워크·레지스트리 검사가 실패하면 서명하지 않는다.

```powershell
python scripts/momso_release_sign.py sign-production --bundle "<묶음.tar.gz>" --server "<momso-server>" --web "<화면.tar.gz>" --bundle-url "<공개 HTTPS 묶음 주소>" --image "<GHCR 이미지@sha256:지문>" --sequence <운영 순번> --version 0.1.1 --private-key "<운영 서명 열쇠 파일 경로>" --public-key "scripts/momso_release_public_key.pem" --output "<새 release.json 경로>"
```

명령은 묶음 안 서버·화면과 주어진 파일의 일치, 묶음 SHA-256, HTTPS 주소, 순번·버전·채널, 고정 운영 공개키와 개인키의 일치, 이미지 형식 및 공개 레지스트리의 단일 이미지 manifest를 검사한다. `release.json`과 `release.json.sig`가 이미 있으면 덮어쓰지 않는다. 일반 `sign` 명령의 레지스트리 검사는 선택이지만, 이 운영 전용 명령에서는 필수다. 이 검사는 이전 릴리스의 순번을 조회하지 않으므로 본부가 운영 직전 순번과 대조해야 한다.

검사는 익명 토큰과 GHCR manifest API의 HTTP 200, `Content-Type`, `Docker-Content-Digest`를 확인한다. 비공개·권한 거절은 `gateway_registry_private`, 통신·서버 실패는 `gateway_registry_unavailable`, 잘못된 토큰 응답은 `gateway_registry_response_invalid`, index 또는 미지원 형식은 `gateway_image_not_single_manifest`, 지문 불일치는 `gateway_image_mismatch`, 잘못된 GHCR 주소는 `gateway_image_invalid`다. 토큰은 출력하지 않는다. 실제 공개 GHCR 검증은 발행 후 별도로 해야 한다.

## 운영 신뢰와 순번

- 운영 공개키 정본은 `scripts/momso_release_public_key.pem`이다. 정적 사이트 workflow `anywhere-static-pages.yml`은 이 파일을 요구한다. 시작 페이지 묶음의 `release` 설정에 공개키와 `production` 채널이 들어가고 브라우저의 `momso_release_manifest.mjs`가 확인한다.
- PC 설치기 `momso_server_install.py`의 기본 공개키·채널은 이 파일과 `production`이다. `momso_release_keys.py`는 전달받은 운영 공개키가 정본과 같은지 확인하고 알려진 개발 공개키를 거절한다. 설치기와 시작 흐름은 VM 업데이트 작업자에 검증키·목록 주소·채널을 전달한다. 서명 목록 자체에도 `channel`이 있고 검증한다.
- 개발과 운영은 서명키·목록 채널이 다르다. **전역적으로 별도 순번을 발급·관리하는 시스템은 없다.** 각 설치의 업데이트 순번은 설치별로 저장되며 뒤로 갈 수 없다. 새 운영 설치의 운영 순번은 개발 설치와 독립적으로 정할 수 있지만, 다른 채널에서 넘어갈 때 순번을 자동으로 바꿔 주지는 않는다.
- PC 설치기는 기존 기록의 `release_channel`과 다른 채널을 거절한다. 개발 설치를 운영 설치로 자동 전환하는 지원 경로는 없다. 현재 운영은 새 설치를 기준으로 한다. 기존 자료를 옮기려면 소유자 승인, 자료 백업·복원, 운영 키와 목록으로 신뢰 변경, 순번 처리 및 실패 복구 설계와 시험이 추가로 필요하다. 기록이나 키를 수동으로 바꿔 우회하지 않는다.
- `scripts/momso-release-channels.py`는 `anywhere-`로 시작하는 태그를 검증하고 `--verify-tag --latest=false`로 발행한다. 개발 시험은 `--prerelease`를 사용한다. 운영용 고유 `anywhere-...` 태그를 정하고 중복을 확인한다. 구체적인 운영 태그 형식이나 첫 순번은 아직 정해지지 않았다. `releases/latest`는 데스크톱 안정 릴리스용이며 anywhere 릴리스가 그것을 차지하지 않는다.

## 첫 운영 릴리스 순서

1. 개인키 소유자가 위 첫 명령을 실행하고 예비 두 폴더를 각각 이 PC가 아닌 서로 다른 곳에 보관한다. 본부에는 **공개키 셋만** 전달한다. 본부는 개인키를 열거나 받지 않는다.
2. 본부가 주 공개키를 `scripts/momso_release_public_key.pem`에 넣고 세 공개키로 `python scripts/momso_release_sign.py create-production-root --public-key "<주 공개키>" --public-key "<예비1 공개키>" --public-key "<예비2 공개키>" --expires "<YYYY-MM-DDTHH:MM:SSZ>" --output scripts/momso_release_root.json`을 실행해 순번 0 root를 만들어 PR을 준비한다. 세 키 식별값을 소유자와 다른 경로로 대조하고 운영 사이트가 개발 키를 거절하는 시험을 확인한다. PR 승인·병합은 별도 절차다.
3. 본부가 GHCR workflow로 `linux/amd64` 단일 manifest 이미지를 발행하고 workflow의 지문을 기록한다. index 지문이 아닌지 다시 확인한다.
4. 본부가 서버·화면·묶음과 공개 HTTPS 주소, 이미지 지문, 기존 운영 순번보다 큰 새 순번을 준비한다. 개인키 소유자가 위 두 번째 명령을 실행한다. 본부는 공개키로 서명·채널·묶음 지문·이미지 지문을 확인한다. 개인키 파일은 받지 않는다.
5. 본부가 서명 목록과 `.sig`, 묶음을 고유 anywhere 태그에 `--latest=false`로 발행하고, 공개 주소의 바이트와 지문, 데스크톱 `releases/latest` 유지 여부를 확인한다. 운영 테스트 설치와 실제 고객 경로는 별도로 확인한다.

## 운영 키 교체

새 키가 필요하면 소유자가 저장소 밖 새 폴더에 `python scripts/momso_release_sign.py init-replacement --directory "<새 열쇠 폴더>"`를 실행한다. 다음 한 줄은 기존 root에서 신뢰하는 세 키 중 둘(주 키와 예비 하나 또는 예비 둘)로 새 root를 서명한다. `--public-key` 첫 번째는 새 주 키이며, 서명한 두 키는 기존 root와 새 root 양쪽에서 각각 정족수 2를 만족해야 한다. 두 개인키의 **경로만** 넘긴다.

```powershell
python scripts/momso_release_sign.py rotate-production-root --current "<현재 검증한 root.json>" --public-key "<새 주 공개키>" --public-key "<남길 공개키 1>" --public-key "<남길 공개키 2>" --signing-key "<서명 개인키 1>" --signing-key "<서명 개인키 2>" --expires "<YYYY-MM-DDTHH:MM:SSZ>" --output "<배포 폴더>/trust-root-<다음 순번>.json"
```

두 결과 파일을 매 운영 릴리스 자산에 빠짐없이 포함한다. 본부는 교체 이력을 검증한 뒤 새 릴리스를 준비한다. 교체된 키로 운영 릴리스를 서명할 때 `sign-production`에 `--active-root "<확인한 최신 root.json>"`와 새 공개키·개인키 경로를 지정한다. 이 옵션은 전달된 root의 형식·현재 주 키와 키 쌍은 확인하지만 **그 root가 이전 root에서 올바르게 교체됐는지는 독립적으로 확인하지 않는다**. 반드시 검증된 교체 이력을 사용한다. PC·VM·브라우저는 배포된 교체 서명을 별도로 검증한다. 교체 이력·브라우저 저장소가 지워지거나 공격자가 최신 교체를 숨기는 경우는 [S142 한계](momso-anywhere-key-rotation-s142.md)를 따른다.

S151 이후 교체 명령은 서명 대상 root에 UTC 발행 시각 `issuedAt`을 자동으로 넣는다. 교체 시각은 직전 root의 만료 전이어야 하고 이전 발행 시각보다 이르거나 검증 시점의 미래이면 거절된다. 중간 root가 지금은 만료됐어도 교체 당시 유효했다면 이력 검증을 통과할 수 있고, 현재 root는 지금 만료되지 않아야 한다. 서명은 `issuedAt` 값의 변경을 막지만, 이 값이 실제 서명 시각이라는 외부 증명은 아니다. 최초 root 순번 0의 과거 형식은 `issuedAt`이 없어도 읽지만 교체 root는 필수다.

관련 정본: [서명 S76](momso-release-signing-s76.md), [서명된 설치 S73](momso-anywhere-signed-install-s73.md), [입구 이미지 S111](momso-anywhere-public-gateway-s111.md), `scripts/momso-release-channels.py`.
