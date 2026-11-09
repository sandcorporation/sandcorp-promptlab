//! WebView2 데이터 폴더 고르기(#114).

use promptlab_app::webview_data::{choose, probe_writable};
use std::path::{Path, PathBuf};

#[test]
fn env_override_is_left_alone() {
    assert_eq!(
        choose(true, Path::new("/d"), Path::new("/t"), |_| true),
        None
    );
}

#[test]
fn prefers_app_data_dir() {
    assert_eq!(
        choose(false, Path::new("/d"), Path::new("/t"), |_| true),
        Some(PathBuf::from("/d/webview"))
    );
}

#[test]
fn falls_back_to_temp_when_data_dir_unwritable() {
    let got = choose(false, Path::new("/d"), Path::new("/t"), |p| {
        p.starts_with("/t")
    });
    assert_eq!(got, Some(PathBuf::from("/t/promptlab-webview")));
}

#[test]
fn none_when_nothing_writable() {
    assert_eq!(
        choose(false, Path::new("/d"), Path::new("/t"), |_| false),
        None
    );
}

#[test]
fn probe_writes_and_cleans() {
    let base = std::env::temp_dir().join(format!("pl-wv-{}", std::process::id()));
    let dir = base.join("a");
    assert!(probe_writable(&dir));
    assert!(!dir.join(".write-test").exists());
    // 파일이 있는 자리는 폴더로 못 만든다.
    let file = base.join("f");
    std::fs::write(&file, b"x").unwrap();
    assert!(!probe_writable(&file.join("sub")));
}
