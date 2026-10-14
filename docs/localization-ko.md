# LightCraft 한국어 표시

LightCraft는 한국어 화면을 지원합니다. 언어 설정은 `ui.json`의 `language`에 `ko`로 저장합니다.

- 「편집 → 언어」 또는 「설정 → 일반 → 언어」에서 바꿉니다. 시스템 언어가 한국어(`ko`, `ko-KR`,
  `ko_KR.UTF-8`…)이면 처음부터 한국어로 표시합니다.
- 메뉴, 사진 편집, 마스크, 자르기, 설정, 가져오기, 내보내기, 주요 작업 현황을 번역했습니다.
- 한글은 [storytold/craft-fonts](https://github.com/storytold/craft-fonts)의 Noto Sans CJK SC(SIL OFL)로
  그립니다. 이 글꼴은 CJK 전체 글리프를 담고 있어 한글 음절도 모두 포함합니다. craft-fonts에 한국어
  전용 글꼴이 추가되기 전까지 `Kore` 문자 체계는 이 글꼴이 대신합니다(`CraftFont::serves`).
  `CRAFT_FONTS_DIR=../craft-fonts`를 지정해 빌드해야(공식 릴리스는 모두 지정) 한글이 보입니다.
- 번역은 표시 단계에서만 합니다. 명령 ID, 사진 파일 이름, 입력한 메타데이터는 바꾸지 않습니다.
- 번역되지 않은 기술적 오류 메시지와 일부 릴리스 노트는 영어로 표시합니다.
- 다른 언어를 추가하는 방법은 [`localization.md`](localization.md)를 참고하세요.

## 용어

Lightroom 한국어판과 국내 사진 편집 사용자에게 익숙한 용어를 씁니다. 예: 노출, 대비, 밝은 영역,
어두운 영역, 흰색 계열, 검정 계열, 텍스처, 부분 대비, 디헤이즈, 생동감, 채도, 색온도, 틴트, 색조(Hue),
광도(Luminance), 톤 곡선, 색상 혼합, 컬러 그레이딩, 렌즈 교정, 사전 설정, 채택/제외.
문장은 합니다체, 메뉴와 버튼은 짧은 명사형으로 씁니다. 사진은 「장」, 항목은 「개」로 셉니다.

## 번역 유지보수

고정 문구는 `crates/ui-egui/locales/ko.json`, 값이 들어가는 문구는
`crates/ui-egui/locales/ko-formats.json`에 있습니다. 영어 원문이 키입니다.

값이 들어가는 문구는 빌드할 때 Rust `format!` 검사를 거칩니다. 값은 이름(`{n}`)이나 위치(`{0}`,
`{1}`)로 참조하고, `{:.1}` 같은 서식 지정은 영어 원문과 같게 씁니다. 한국어에는 복수형이 없으므로
영어 복수 어미(`"s"` / `""`)에 해당하는 인수는 `{:.0}`으로 소비합니다.
`LIGHTCRAFT_LANGUAGE=ko lightcraft-cli snapshot ...`으로 한국어 화면을 그려 볼 수 있습니다.

표시, 글꼴, 언어 전환, 설정 저장, 명령 ID 유지는 `cargo test -p lightcraft-ui-egui i18n::tests`로
검증합니다(글리프 검사는 `CRAFT_FONTS_DIR`을 지정했을 때만 실행).
