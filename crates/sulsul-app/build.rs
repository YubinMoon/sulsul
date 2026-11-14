//! 번역 파일은 `rust_i18n::i18n!`이 컴파일할 때 읽어 실행 파일에 넣습니다.
//! 매크로는 파일 변경을 추적하지 않으므로, 번역을 고치면 다시 빌드되도록 여기서 알려 줍니다.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../locales");
}
