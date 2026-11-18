//! 번역 파일은 `rust_i18n::i18n!`이 컴파일할 때 읽어 실행 파일에 넣습니다.
//! 매크로는 파일 변경을 추적하지 않으므로, 번역을 고치면 다시 빌드되도록 여기서 알려 줍니다.
//!
//! Windows 실행 파일에는 아이콘과 파일 정보를 넣습니다. 탐색기, 시작 메뉴 바로가기, 작업 관리자,
//! 설치 프로그램의 "앱 제거" 목록이 이 값을 씁니다. 아이콘(`assets/icon/sulsul.ico`)은 트레이 아이콘
//! (`src/tray.rs`의 `icon_rgba`, 대기 상태)과 같은 그림입니다.

const ICON: &str = "assets/icon/sulsul.ico";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../locales");
    println!("cargo:rerun-if-changed={ICON}");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon(ICON)
            .set("FileDescription", "술술")
            .set("ProductName", "술술");
        if let Err(error) = resource.compile() {
            panic!("could not embed the Windows resources: {error}");
        }
    }
}
