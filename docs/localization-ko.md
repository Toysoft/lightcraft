# LightCraft 한국어 표시

LightCraft는 한국어(`ko`) 인터페이스를 지원합니다. 언어는 편집 메뉴 또는 설정의 일반 섹션에서 바꿀 수
있으며, 선택한 BCP-47 코드가 `ui.json`에 저장되어 다음 실행에도 유지됩니다. 저장된 설정이 없는 경우
`LIGHTCRAFT_LANGUAGE`가 우선 적용되고, 그렇지 않으면 시스템의 `LC_ALL`, `LC_MESSAGES`, `LANG` 값을
확인합니다. `ko-KR`, `ko_KR.UTF-8`도 한국어로 인식합니다.

한국어 카탈로그는 2,410개 메시지 키를 모두 포함합니다. 이 가운데 1,807개(75%)가 번역되어 있으며,
나머지는 영어 원문으로 표시됩니다. 메뉴와 패널, 편집 도구, 가져오기·내보내기, 메타데이터, 설정 및
주요 작업 상태를 번역했습니다. 번역에서 RAW, ISO, XMP, GPS, HDR 같은 사진 기술 약어와 단축키는
원문 그대로 둡니다. 주요 용어는 `노출`, `화이트 밸런스`, `하이라이트`, `그림자`, `색조`, `채도`,
`생동감`, `텍스처`, `명료도`, `안개 제거`, `비네팅`, `그레인`, `프리셋`, `마스크`, `앨범`, `키워드`,
`메타데이터`로 통일했습니다.

## 한글 글꼴

한글에는 SIL OFL 1.1로 배포되는 Pretendard를 사용합니다. 폰트 파일은 애플리케이션 저장소에 포함하지
않고 [storytold/craft-fonts#25](https://github.com/storytold/craft-fonts/pull/25)의 craft-fonts 입력을
사용합니다. 해당 PR이 병합된 뒤 `CRAFT_FONTS_DIR=../craft-fonts`로 빌드하면 데스크톱은 일반·굵은
Pretendard를 사용하고 웹 빌드는 일반 폰트를 WASM 모듈에 포함합니다. 폰트 입력 없이 빌드하면
한글 글리프가 없어 문자가 네모로 표시될 수 있습니다. 공식 릴리스는 이 입력을 필수로 사용합니다.

이 번역은 표시 계층에만 적용됩니다. 명령 ID, 단축키, 파일 이름, 사진 메타데이터 및 사용자 입력은
변경하지 않습니다. 아직 한국어 원어민의 공식 검수는 이루어지지 않았으며, 카탈로그에 없는 기술 오류와
일부 긴 도움말·릴리스 노트는 영어로 표시됩니다.

## 번역 유지 및 검증

정적 문자열은 `crates/ui-egui/locales/ko.json`, 런타임 값이 포함된 문자열과 날짜 형식은
`crates/ui-egui/locales/ko-formats.json`에 있습니다. 영어 원문 키는 변경하지 않습니다. 포맷 문자열은
Rust 빌드 시 인자와 대조되며, 테스트는 모든 기존 카탈로그 키의 포함 여부와 플레이스홀더 보존을
확인합니다.

```sh
LIGHTCRAFT_LANGUAGE=ko lightcraft-cli snapshot --demo -o ko.png --size 1600x1000
CRAFT_FONTS_DIR=../craft-fonts cargo test -p lightcraft-ui-egui i18n::tests
```

자세한 추가·유지 방법은 [`localization.md`](localization.md), 언어별 번역 범위는
[`localization-parity.md`](localization-parity.md)를 참조하세요.
