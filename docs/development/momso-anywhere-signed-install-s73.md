정리본: [momso-anywhere-guide.md](momso-anywhere-guide.md)

# 설치의 서명 확인 S73

설치와 업데이트는 S38 목록의 같은 원본 바이트에 대한 Ed25519 서명을 확인한다. 필수 필드는 `sequence`, `version`, `bundleUrl`, `bundleSha256`, `gatewayImage`다. 목록은 HTTPS로 받으며 서명 파일은 같은 주소의 경로 뒤에 `.sig`를 붙인 곳의 64바이트 원본이다. 설치는 서명이 맞기 전 클라우드 자원을 만들지 않는다. 지문만 확인하는 것은 서명을 확인하는 것이 아니다.

PC 설치기와 S38 업데이트는 `momso-server-update/release_manifest.py`의 같은 Python 검증 함수를 호출한다. Windows에는 Ed25519를 지원하는 OpenSSL 3이 필요하다. PATH에 없으면 Git for Windows의 OpenSSL을 찾아본다. PC 설치기는 목록의 묶음을 받아 SHA-256을 확인한 뒤 입력된 서버 파일과 화면 tar의 모든 파일이 그 묶음과 일치하는지도 확인한다. 선택적 `--gateway-image`가 있으면 서명된 값과 같아야 한다. 목록 또는 묶음이 틀리면 서버·화면·입구를 설치하지 않는다.

휴대폰의 `momso_release_manifest.mjs`는 같은 목록·공개키·서명 원본을 WebCrypto Ed25519로 확인한다. 브라우저와 Python은 서로 다른 실행 환경이므로 암호화 코드는 공유할 수 없으며, 시험용 동일 서명을 양쪽에 대조한다. VM은 서명된 묶음 지문을 다시 확인한다. WebCrypto Ed25519가 없는 브라우저에서는 안전하게 설치를 중단하며 해당 기기 지원 여부는 실제 휴대폰에서 확인해야 한다. [Web Crypto Ed25519](https://developer.mozilla.org/en-US/docs/Web/API/SubtleCrypto/importKey), [OpenSSL Ed25519](https://docs.openssl.org/3.0/man1/openssl-pkeyutl/).

## 공개키와 교체

진짜 개인키는 개발자만 보관하고 AI와 시험은 읽지 않는다. 아직 실제 공개키와 서명된 배포 목록은 준비되지 않았다. PC 배포물을 만들 때 검증한 Ed25519 공개키 PEM을 설치기 옆 `scripts/momso_release_public_key.pem`으로 포함한다. 개발·시험에서는 `--release-public-key`로 신뢰할 공개키 파일을 명시할 수 있다. 이 인자는 신뢰하는 운영자가 고르는 것이며, 공격자가 지정한 키를 신뢰하라는 뜻이 아니다.

시작 페이지 묶음은 `--release-public-key`로 받은 같은 공개키를 `config.json`의 `release.publicKeySpki`에 고정하고 `release.manifestUrl`, `release.loginOrigin`, `release.channel`을 함께 담는다. 개인키나 토큰은 넣지 않는다. 운영과 개발 공개키·채널의 분리는 [S76](momso-release-signing-s76.md)을 따른다. 공개키가 실린 설치기와 정적 페이지의 배포 권한이 공격자에게 넘어가면 키도 바뀔 수 있으므로 그 배포 경로를 보호해야 한다.

키 교체 시 먼저 신뢰할 경로로 새 공개키가 고정된 설치기·시작 페이지·업데이트 구성을 배포하고, 사용자들이 새 구성을 받을 수 있는지 확인한 다음 새 개인키로 서명한 목록으로 전환한다. 현재 각 구성은 공개키 하나만 신뢰하므로 옛 구성은 새 키의 목록을 거절한다. 옛 구성을 남겨두고 원활히 옮기는 다중키 교체는 아직 없다. 유출된 키의 폐기·복구에도 신뢰할 별도 배포 경로가 필요하다.

## 시험과 한계

`scripts/test_momso_release_manifest.py`는 그 자리에서 만든 시험 키를 종료 시 버린다. 같은 서명을 Python과 브라우저 구현으로 확인하고, 잘못된 서명과 바뀐 서버를 거절하며 PC 설치가 자원을 만들기 전에 멈추는지 본다. S68의 가짜 Google 흐름도 잘못된 서명에서 클라우드 요청이 없는지 확인한다. 진짜 개인키·계정·서명된 운영 목록은 쓰지 않는다.

목록의 서명은 묶음과 입구 이미지 지문을 묶지만, 이미지 안의 로그인 전 아이콘이 화면 묶음과 일치하는지는 검증하지 않는다. 이미지 제작 시 확인해야 한다. 실제 배포 경로·공개키 고정·서명된 운영 목록과 휴대폰 브라우저 호환성은 본부의 실제 확인이 남는다.
