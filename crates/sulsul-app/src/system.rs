//! OS 기본 앱으로 파일·폴더·웹 주소 열기.

use std::path::Path;

/// 파일을 OS 기본 앱으로, 폴더를 파일 관리자로 엽니다.
pub fn open_path(path: &Path) -> Result<(), String> {
    opener::open(path).map_err(|e| {
        tracing::warn!("failed to open {}: {e}", path.display());
        e.to_string()
    })
}

/// 웹 주소를 기본 브라우저로 엽니다.
pub fn open_url(url: &str) -> Result<(), String> {
    opener::open_browser(url).map_err(|e| {
        // 주소에는 로그인에 쓰는 값이 들어 있을 수 있어 로그에 남기지 않습니다.
        tracing::warn!("failed to open the browser: {e}");
        e.to_string()
    })
}

/// 폴더가 없으면 만든 뒤 엽니다.
pub fn open_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| {
        tracing::warn!("failed to create {}: {e}", path.display());
        e.to_string()
    })?;
    open_path(path)
}
