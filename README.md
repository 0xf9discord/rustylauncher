# Rusty Launcher

RustyLauncher is an independent Minecraft: Java Edition launcher for Windows, built with Rust and GPUI. It downloads official game files and supports Vanilla, Fabric, Forge, Quilt, and NeoForge profiles with separate modded game directories.

Authentication uses Microsoft's device-code flow, Xbox Live, XSTS, and Minecraft Services. Users sign in on Microsoft's own website; the launcher does not collect passwords. Authentication tokens remain in process memory. Each deployment uses its own Microsoft application ID, and Minecraft API access is subject to Microsoft's/Mojang's approval. The current implementation has verified Microsoft device-code issuance; complete Minecraft authentication is not yet verified with an approved application ID.

Build: `cargo build --release --locked`. Requires Windows x64, Rust 1.95+, Visual Studio C++ Build Tools and Windows SDK. This project is not affiliated with or endorsed by Mojang or Microsoft.

Rust + GPUI로 만든 Windows x64용 Minecraft Java Edition 런처입니다. WebView와 Electron 없이 네이티브 UI를 사용하며, Minecraft 클라이언트나 모드는 포함하지 않습니다.

## 실행

Rust 1.95 이상과 Visual Studio C++ Build Tools / Windows SDK가 필요합니다.

```powershell
cargo run --release --locked
```

실행 파일은 `target/release/rustylauncher.exe`입니다. GPUI 렌더링을 지원하는 GPU와 드라이버가 필요합니다.

로고는 잔디 블록과 Rust 마스코트 Ferris를 합친 디자인입니다. 화면 로고와 Windows 실행 파일 아이콘에 내장되어 별도 이미지 파일 없이 동작합니다. 원본과 출처는 `assets/logo.svg`, `assets/NOTICE.md`를 확인하세요.

## 사용

1. 버전 라이브러리에서 게임 버전과 **Vanilla / Fabric / Forge / Quilt / NeoForge**를 고릅니다. 모드 로더는 호환되는 로더 버전도 선택한 뒤 **설치 / 파일 검사**를 누릅니다. 기본 선택은 최신 정식 바닐라 버전입니다.
2. 해당 버전에 필요한 Java를 설치합니다. 런처가 `JAVA_HOME`, `PATH`, 일반적인 Java 설치 폴더를 탐색합니다. 버전 메타데이터에 명시된 Java 버전을 우선 선택합니다.
3. 아래의 Microsoft 앱 ID 설정을 완료하고 **Microsoft 로그인**을 누릅니다.
4. 표시된 코드를 복사하고 **브라우저 열기**에서 직접 로그인합니다.
5. **플레이**를 누릅니다. 종료 상태는 런처에서, 게임 출력은 **활동 로그 → 게임 로그 열기**에서 확인합니다.

플레이 버튼은 미설치 상태에서는 설치, 미인증 상태에서는 로그인으로 연결됩니다. 각 단계가 완료되면 다시 눌러 진행합니다. Tab / Shift+Tab으로 버튼을 이동하고 Enter / Space로 실행할 수 있습니다.

## Microsoft 로그인 설정

공용으로 사용할 Microsoft 앱 ID는 이 저장소에 포함되어 있지 않습니다. **설정 → 설정 파일 편집**에서 `microsoft_client_id`에 본인의 Microsoft Entra **애플리케이션(클라이언트) ID**를 지정하세요. 로그인 버튼은 클릭할 때 파일의 최신 앱 ID를 읽습니다. 콜백 주소(`myapp://auth` 등)나 클라이언트 비밀값을 넣지 마세요. 다른 설정은 **설정 다시 불러오기**로 반영합니다.

- 개인 Microsoft 계정을 지원하는 앱을 등록하고 public client / device code flow를 활성화해야 합니다.
- `XboxLive.signin`을 사용합니다. Minecraft Services 접근 권한은 Microsoft/Mojang 정책 및 앱 승인 상태에 따라 제한될 수 있습니다. 단순 앱 등록만으로 접근이 보장되지는 않습니다.
- Java Edition 소유권과 Minecraft 프로필이 있는 계정이 필요합니다.
- 로그인 경로: Microsoft 장치 코드 → Xbox Live → XSTS → Minecraft 토큰 → Minecraft 프로필.
- 토큰을 파일에 저장하지 않습니다. 앱 재시작 또는 세션 만료 후에는 다시 로그인해야 합니다.
- Minecraft 인증의 HTTP 403은 원인을 구분합니다. `INVALID_APP_REGISTRATION`이면 서버가 앱 등록을 거절한 것이므로 [Microsoft/Minecraft 앱 등록 안내](https://aka.ms/AppRegInfo)를 확인하세요. `ACCOUNT_SUSPENDED`는 계정 제한, `NON_JSON_RESPONSE`는 서버·게이트웨이의 비정상 응답입니다. 일반적인 403만으로 앱 미승인을 단정하지 않습니다.
- 실패 단계와 정규화한 오류 사유는 `%APPDATA%/RustyLauncher/auth.log`에 저장합니다. 서버 원문, 인증 토큰, 계정 비밀번호는 기록하지 않습니다.
- 계정이 필요한 실제 로그인과 게임 구동은 개발 환경에서 자동 검증하지 않습니다.

[Microsoft 장치 코드 인증 문서](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code)

## 설정 및 데이터

`%APPDATA%/RustyLauncher/settings.json`:

```json
{
  "loader": "vanilla",
  "loader_version": "",
  "memory_mb": 4096,
  "java_path": "",
  "microsoft_client_id": "",
  "selected_version": "",
  "show_snapshots": false
}
```

`java_path`가 비어 있으면 자동 탐색합니다. 직접 지정하는 경우 JSON에서 역슬래시를 두 번 적거나 `C:/Program Files/.../bin/java.exe`처럼 슬래시를 사용하세요. UI에서는 2–16 GB를 선택할 수 있으며 설정 파일은 1–32 GB 범위를 허용합니다.

설정은 변경한 항목만 최신 파일에 병합하고 임시 파일을 동기화한 뒤 교체합니다. 실행 중 외부 편집기로 변경한 앱 ID와 알 수 없는 필드도 보존하며, 여러 창의 저장은 파일 잠금으로 순서를 보장합니다. 메모장의 UTF-8 BOM 파일을 지원합니다. JSON 오류가 있으면 기존 파일을 보존하고 오류를 표시합니다. 인증 토큰의 메모리 보관 정책과 일반 설정의 영구 저장은 별개입니다.

게임은 `%APPDATA%/RustyLauncher/game`에 저장합니다. 기존 `.minecraft`와 별도이며, 버전 간 월드와 리소스 팩은 이 게임 폴더에서 공유합니다. 스냅샷에서 기존 월드를 열기 전에는 백업하세요.

모드 로더의 월드·설정·모드는 `%APPDATA%/RustyLauncher/instances/{로더}-{게임 버전}`에 별도로 저장합니다. 버전 라이브러리의 **모드 폴더** 버튼으로 해당 인스턴스의 `mods` 폴더를 열 수 있습니다. 같은 게임 버전과 로더의 로더 버전을 업데이트할 때는 이 폴더를 재사용합니다.

Fabric·Quilt는 공식 Meta API와 Maven 배포 파일로 프로필을 구성합니다. Forge·NeoForge는 SHA-1을 확인한 공식 설치기 JAR를 Java로 실행해 클라이언트 패치를 처리합니다. 설치기 출력은 `%APPDATA%/RustyLauncher/installer.log`에서 확인할 수 있습니다. 호환 배포가 없는 게임/로더 조합은 설치할 수 없습니다.

로더만 설치하며 Fabric API, Quilted Fabric API 및 개별 모드는 자동으로 설치하지 않습니다. 필요한 모드를 해당 인스턴스의 `mods` 폴더에 넣으세요.

공식 배포처: [Fabric Meta](https://meta.fabricmc.net/), [Quilt Meta](https://meta.quiltmc.org/), [Forge](https://files.minecraftforge.net/), [NeoForge Maven](https://maven.neoforged.net/releases/net/neoforged/neoforge/).

## 구현 범위

- Java Edition **1.18 이상 바닐라** 정식 버전 및 현대 스냅샷 목록
- Fabric / Quilt / Forge / NeoForge 호환 버전 목록, 설치 및 로더 프로필 실행
- 바닐라 인수·에셋 상속, 로더 메인 클래스와 라이브러리 우선 적용, 모드 인스턴스 분리
- 공식 Mojang 메타데이터, 클라이언트 JAR, 라이브러리, 에셋 다운로드
- 최대 12개 병렬 다운로드, SHA-1 검사, 임시 파일 후 교체, 파일별 최대 3회 시도
- 재설치 시 정상 파일 재사용, 손상 파일 재다운로드, 설치 취소
- 메타데이터 기반 JVM/game 인수, Windows 규칙, 네이티브 추출
- 백그라운드 설치·로그인·실행 감시, 이벤트가 올 때 UI 갱신
- 버전 목록 네트워크 오류 시 저장된 목록 사용

Java 자동 다운로드, 모드팩·모드 자동 설치, 계정 영구 저장, 구형 Minecraft, Windows ARM64 및 다른 OS는 현재 구현 범위에 포함되지 않습니다. NeoForge는 현재 `net.neoforged:neoforge` 배포가 있는 버전만 지원합니다. 오프라인 목록 캐시는 오프라인 계정 인증을 의미하지 않습니다. 성능 수치는 별도 벤치마크 전에는 보장하지 않습니다.

## 개발 검증

```powershell
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked -- -D warnings
```

설치 경로 보호, OS/기능 규칙, 공백이 있는 실행 인수, 캐시 체크섬·취소, Java 버전 판별을 테스트합니다.

공식 서비스 연동 테스트는 네트워크가 있을 때 별도로 실행합니다.

```powershell
cargo test --release --locked official_metadata_and_asset_download -- --ignored --nocapture
cargo test --release --locked install_official_loaders -- --ignored --nocapture
cargo test --release --locked configured_device_code -- --ignored --nocapture
```

2026-11-09 검증: 릴리스 빌드, 단위 테스트 5개, Clippy, 포맷 검사를 통과했습니다. 공식 서비스 연동 테스트로 26.3과 1.18.2의 메타데이터·에셋을 검증했습니다. Windows에서 실제 GPUI 화면과 화면 전환, 설치 취소 후 재시작을 확인했고 26.3 전체 설치(5,223개 파일)를 완료했습니다. Microsoft 로그인 및 인증된 게임 실행은 앱 ID와 사용자 계정이 필요하므로 검증하지 않았습니다.

로더 확장 검증: 단위 테스트 9개 및 공식 서비스 연동 테스트에서 Minecraft 1.21.1용 Fabric 0.20.1, Quilt 0.30.2, Forge 52.1.21, NeoForge 21.1.265의 실제 설치와 Java 실행 명령 구성을 확인했습니다. 실행 명령 검증은 인증 정보를 전송하거나 게임을 구동하지 않습니다.

로그인·설정 회귀 테스트는 설정 재로드, 외부 앱 ID 변경 보존, 여러 창의 동시 저장, UTF-8 BOM, 잘못된 JSON 보존 및 인증 오류 응답의 민감 정보 비노출을 확인합니다. `configured_device_code`는 로컬 앱 ID로 Microsoft 코드 발급을 검증하며, 비밀번호나 계정 인증 정보는 전송하지 않습니다.

로그인·설정 수정 검증: 일반 테스트 15개와 로컬 설정의 앱 ID를 사용한 Microsoft 코드 발급 테스트를 통과했습니다. 계정 소유자의 요청에 따라 대화형 로그인 검증은 코드 발급 단계에서 중지했으며, Xbox/Minecraft 계정 인증 완료는 확인하지 않았습니다.

Minecraft는 Mojang/Microsoft의 상표이며 이 프로젝트는 비공식 런처입니다.
