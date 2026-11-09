//! 응시 앱에 구운 Monaco 편집기 자산(#97). 판·받는 법은 `scripts/fetch-monaco.sh`(sha256 검증, MIT — 고지는
//! `assets/monaco/LICENSE`·`ThirdPartyNotices.txt`). 화면이 웹뷰 경로 `/monaco/…`를 이 표로 내준다 — 외부 요청 없음.

/// 웹뷰 경로 앞머리(`/monaco/vs/loader.js`).
pub const PREFIX: &str = "monaco";
/// Monaco 판(`assets/monaco/VERSION`).
pub const MONACO_VERSION: &str = include_str!("../assets/monaco/VERSION");

macro_rules! asset {
    ($path:literal) => {
        (
            $path,
            include_bytes!(concat!("../assets/monaco/", $path)).as_slice(),
        )
    };
}

/// 구운 파일들(경로는 `vs/` 밑).
const FILES: &[(&str, &[u8])] = &[
    asset!("vs/loader.js"),
    asset!("vs/nls.messages.ko.js"),
    asset!("vs/editor/editor.main.js"),
    asset!("vs/editor/editor.main.css"),
    asset!("vs/base/browser/ui/codicons/codicon/codicon.ttf"),
    asset!("vs/base/worker/workerMain.js"),
    asset!("vs/basic-languages/python/python.js"),
    asset!("vs/basic-languages/javascript/javascript.js"),
    asset!("vs/basic-languages/java/java.js"),
    asset!("vs/basic-languages/cpp/cpp.js"),
    asset!("LICENSE"),
    asset!("ThirdPartyNotices.txt"),
];

/// JavaScript 언어 서비스(TypeScript worker, 5MB)는 굽지 않는다 — 구문 강조는 `basic-languages/javascript`가 하고,
/// Monaco가 javascript 모델을 열 때 부르는 이 모듈은 아무것도 하지 않는 대역으로 답한다.
const TS_MODE_STUB: &[u8] =
    b"define('vs/language/typescript/tsMode',[],function(){return{setupTypeScript:function(){},setupJavaScript:function(){},getJavaScriptWorker:function(){return Promise.reject(new Error('off'))},getTypeScriptWorker:function(){return Promise.reject(new Error('off'))}}});";

/// `vs/…` 경로(앞머리 `/monaco/` 뒤)의 내용과 MIME. 없으면 `None`.
pub fn get(path: &str) -> Option<(&'static [u8], &'static str)> {
    let path = path.trim_start_matches('/');
    let path = path.strip_prefix("monaco/").unwrap_or(path);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    if path == "vs/language/typescript/tsMode.js" {
        return Some((TS_MODE_STUB, "text/javascript"));
    }
    FILES
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(p, bytes)| (*bytes, mime(p)))
}

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("ttf") => "font/ttf",
        _ => "text/plain; charset=utf-8",
    }
}

/// 구운 파일 경로 전부(시험·설치 확인용).
pub fn paths() -> impl Iterator<Item = &'static str> {
    FILES.iter().map(|(p, _)| *p)
}
