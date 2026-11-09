//! 앱에 구운 Monaco 자산(#97): 모든 경로가 내용·MIME을 갖고, JS 언어 서비스는 대역, 없는 경로는 `None`.

use promptlab_app::editor_assets::{MONACO_VERSION, PREFIX, get, paths};

#[test]
fn 구운_파일은_모두_내용과_mime이_있다() {
    let mut count = 0;
    for p in paths() {
        let (bytes, mime) = get(&format!("/{PREFIX}/{p}")).unwrap_or_else(|| panic!("{p}"));
        assert!(!bytes.is_empty(), "{p}");
        let want = if p.ends_with(".js") {
            "text/javascript"
        } else if p.ends_with(".css") {
            "text/css"
        } else if p.ends_with(".ttf") {
            "font/ttf"
        } else {
            "text/plain; charset=utf-8"
        };
        assert_eq!(mime, want, "{p}");
        count += 1;
    }
    assert!(count >= 10);
    // 편집기 본체·로더·한국어 문구·4개 언어 구문 강조.
    for p in [
        "vs/loader.js",
        "vs/editor/editor.main.js",
        "vs/nls.messages.ko.js",
        "vs/basic-languages/python/python.js",
        "vs/basic-languages/javascript/javascript.js",
        "vs/basic-languages/java/java.js",
        "vs/basic-languages/cpp/cpp.js",
        "LICENSE",
    ] {
        assert!(get(p).is_some(), "{p}");
    }
}

#[test]
fn 판은_고정이고_mit_고지가_있다() {
    assert_eq!(MONACO_VERSION.trim(), "0.52.2");
    let (license, _) = get("LICENSE").unwrap();
    assert!(String::from_utf8_lossy(license).contains("MIT"));
}

#[test]
fn 쿼리는_떼고_ts_언어_서비스는_대역이고_없는_경로는_none이다() {
    assert!(get("/monaco/vs/loader.js?v=1").is_some());
    let (stub, mime) = get("/monaco/vs/language/typescript/tsMode.js").unwrap();
    assert_eq!(mime, "text/javascript");
    assert!(String::from_utf8_lossy(stub).contains("vs/language/typescript/tsMode"));
    assert!(get("/monaco/vs/language/typescript/tsWorker.js").is_none());
    assert!(get("/monaco/../Cargo.toml").is_none());
    assert!(get("/monaco/").is_none());
}
