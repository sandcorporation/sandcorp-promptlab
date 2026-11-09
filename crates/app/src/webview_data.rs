//! WebView2(Edge) 데이터 폴더 고르기(#114). 쓸 수 없는 폴더를 Edge에 맡기면 「데이터 디렉터리를 만들 수 없습니다」
//! 오류 대화상자가 뜨고 「확인」까지 남는다(서비스 계정·권한 없는 프로필). 그래서 시작 전에 쓰기를 시험해 명시한다.
//!
//! 순서: 실행 env `WEBVIEW2_USER_DATA_FOLDER`(있으면 Edge가 그대로 쓰므로 건드리지 않는다) → 앱 데이터 폴더
//! (`PROMPTLAB_DATA_DIR` → `%LOCALAPPDATA%\promptlab`, [`crate::proclog::data_dir`])의 `webview` → `%TEMP%\promptlab-webview`.
//! 셋 다 안 되면 `None`(Edge 기본에 맡긴다).

use std::path::{Path, PathBuf};

/// 앱 데이터 폴더 아래 웹뷰 폴더 이름.
pub const WEBVIEW_DIR: &str = "webview";
/// 임시 폴더 아래 대체 폴더 이름.
pub const FALLBACK_DIR: &str = "promptlab-webview";

/// 후보를 차례로 시험해 처음 쓸 수 있는 폴더를 낸다. `env_override`가 있으면 `None`(Edge에 맡김).
pub fn choose(
    env_override: bool,
    data_dir: &Path,
    temp_dir: &Path,
    writable: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if env_override {
        return None;
    }
    [data_dir.join(WEBVIEW_DIR), temp_dir.join(FALLBACK_DIR)]
        .into_iter()
        .find(|p| writable(p))
}

/// 폴더를 만들고 파일 하나를 써 본다.
pub fn probe_writable(dir: &Path) -> bool {
    let probe = dir.join(".write-test");
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&probe, b"ok"))
        .and_then(|()| std::fs::remove_file(&probe))
        .is_ok()
}

/// 이 프로세스 환경으로 고른다.
pub fn choose_here() -> Option<PathBuf> {
    let env_set = std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_some_and(|v| !v.is_empty());
    choose(
        env_set,
        &crate::proclog::data_dir(),
        &std::env::temp_dir(),
        probe_writable,
    )
}
