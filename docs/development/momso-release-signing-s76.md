정리본: [momso-anywhere-guide.md](momso-anywhere-guide.md)

# 서명된 배포 목록 S76

운영 목록을 낼 때 개발자가 직접 실행할 한 줄(괄호는 본인이 채움):

```powershell
python scripts/momso_release_sign.py sign --channel production --bundle "<서버와 화면을 담은 tar.gz>" --server "<Linux 서버 파일>" --web "<화면 tar.gz>" --bundle-url "<공개 HTTPS 묶음 주소>" --image "<입구 이미지@sha256:지문>" --sequence <이전보다 큰 정수> --version 0.1.1 --private-key "<본인 개인키 경로>" --public-key "scripts/momso_release_public_key.pem" --output "<새 release.json 경로>"
```

이 명령은 목록과 같은 이름 뒤에 `.sig`가 붙은 원본 서명 파일을 만든다. 기존 파일은 덮지 않는다. 개인키 내용과 암호는 출력하지 않는다. 암호화된 개인키라면 실행하는 사람이 미리 `MOMSO_SIGNING_PASSWORD` 환경변수에 암호를 넣고 실행 후 지운다. AI에게 개인키나 암호를 보여 주지 않는다. OpenSSL 3과 Python이 필요하다. 서명 전에 입력 서버와 화면의 내용이 묶음과 같은지, 공개키가 고정된 운영 공개키인지, 개인키가 그 공개키에 맞는지 확인한다. [OpenSSL Ed25519 서명](https://docs.openssl.org/3.0/man1/openssl-pkeyutl/).

## 개발용 키 만들기

본부가 한 번 실행한다. 이미 폴더가 있으면 덮지 않고 실패한다.

```powershell
python scripts/momso_release_sign.py init-development --directory "$env:USERPROFILE\Documents\dev\momso-vm-install\dev-signing"
```

`dev-private.pem`과 `dev-public.pem`이 생긴다. 저장소 안에는 만들 수 없다. 새 폴더는 Windows에서 현재 사용자와 시스템만 접근하도록 ACL을 설정한다. 개발 개인키는 암호화하지 않으므로 이 PC와 폴더 권한을 보호하고, 공유·커밋·출력하지 않는다. 명령 실패로 폴더가 남으면 그 내용과 소유자를 확인한 뒤 정리한다. 실제 개발 키는 이 작업에서 생성하지 않았다.

## 개발 목록과 기존 VM 올리기

1. 서버·화면·입구 이미지를 준비하고 서버와 화면이 들어간 묶음을 `prepare_momso_anywhere_bundle.py`로 만든다. 파일과 이미지 지문을 확인한다.
2. 위 운영 명령과 같은 `sign` 명령에서 `--channel development`, `--private-key "<dev-signing의 dev-private.pem>"`, `--public-key "<dev-signing의 dev-public.pem>"`을 쓴다. 목록과 `.sig`, 묶음을 신뢰할 수 있는 HTTPS 시험 위치에 올린다. 목록 URL과 묶음 URL은 공개 값이며 비밀을 넣지 않는다.
3. **기존 설치 기록**을 그대로 지정한다. PC `momso_server_install.py anywhere`에 `--release-manifest-url "<개발 목록 HTTPS 주소>" --release-public-key "<dev-public.pem 경로>" --release-channel development`와 기존 계정·프로젝트·설치 기록·로그인 설정, 같은 서버·화면 파일 및 SHA-256을 넘긴다. 상세 인자는 [설치 명령](momso-anywhere-onboarding-s26.md)을 따른다. 이전에 서명 확인 없이 설치해 채널 표시가 없는 기록은 처음 검증한 채널을 기록하고 기존 설치의 올리기 경로를 탄다. 다른 설치를 새로 만들지 않는다.
4. 성공 여부와 기존 자료·입구 주소를 본부가 직접 확인한다. 이 작업에서는 실제 고객 VM이나 계정에 명령을 보내지 않았다.

개발 목록 서명 명령의 예:

```powershell
python scripts/momso_release_sign.py sign --channel development --bundle "<서버와 화면 tar.gz>" --server "<Linux 서버 파일>" --web "<화면 tar.gz>" --bundle-url "<개발 묶음 HTTPS 주소>" --image "<시험 입구 이미지@sha256:지문>" --sequence <정수> --version 0.1.1 --private-key "$env:USERPROFILE\Documents\dev\momso-vm-install\dev-signing\dev-private.pem" --public-key "$env:USERPROFILE\Documents\dev\momso-vm-install\dev-signing\dev-public.pem" --output "<새 개발 release.json 경로>"
```

휴대폰 시험 페이지는 `prepare_momso_anywhere_start_site.py`에 같은 개발 공개키와 `--release-channel development`를 넘겨 **시험 출처**에 따로 만든다. 운영 출처에 이 결과를 올리지 않는다.

## 운영과 개발 분리

목록의 `channel`은 서명된 내용이다. 개발 설치는 `development`만, 운영 설치는 `production`만 받는다. 예전 S38 목록에 `channel`이 없으면 운영으로 취급한다. 한 번 채널이 기록된 PC 설치 기록을 다른 채널로 바꿀 수 없다. 업데이트 부품을 개발 설치에 연결할 때는 해당 개발 공개키와 `channel='development'`를 명시해야 한다. 그렇지 않으면 기본 운영 채널로 검사해 거절한다.

운영 시작 페이지와 운영 서명은 저장소의 `scripts/momso_release_public_key.pem`에 미리 고정된 **운영 공개키와 같은 키**만 받는다. 운영 공개키가 없으면 실패한다. 알려진 `dev-signing/dev-public.pem`과 같은 키도 운영용으로 거절하고, 그 키의 DER·base64 표현이 화면 빌드 파일에 들어 있어도 운영 묶음을 거절한다. 개발 페이지와 개발 서명은 고정된 운영 공개키를 거절한다. PEM을 다른 이름으로 복사해도 키 바이트로 비교한다. 검사는 알려진 개발 공개키와 일반적인 표현을 대상으로 하므로, 알려지지 않은 다른 키나 의도적으로 난독화된 표현까지 판별하지는 못한다. 운영 공개키 파일과 배포 도구 자체를 바꾸는 권한도 신뢰해야 하며, 이 파일이 잘못 지정되거나 공격자에게 교체되는 상황까지 스스로 막지는 못한다.

운영 공개키는 아직 확정되지 않았다. 본인이 공개키를 고정·검수한 뒤 운영 시작 페이지를 만들어야 한다. 키 교체는 [S73](momso-anywhere-signed-install-s73.md)의 순서를 따른다. 공개키는 공개 가능하지만 개인키는 반드시 본인이 보관한다.
