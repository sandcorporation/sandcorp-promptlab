//! Monaco 편집기 다리(#97): 앱에 구운 자산을 웹뷰 경로 `/monaco/…`로 내주고, 편집기 칸이 떠 있는 동안 `editor.js`와
//! 말을 주고받는다(Rust → JS: `Ui::to_editor`, JS → Rust: `Ui::on_editor`). 규칙은 lib(`promptlab_app::editor`).

use std::borrow::Cow;

use dioxus::desktop::use_asset_handler;
use dioxus::desktop::wry::http::Response;
use dioxus::prelude::*;
use promptlab_app::editor_assets;
use serde_json::{Value, json};

use super::Ui;

const EDITOR_JS: &str = include_str!("editor.js");

/// 웹뷰의 콘텐츠 보안 정책: 앱 자신(구운 자산·dioxus)과 dioxus의 로컬 웹소켓만. 외부 스크립트·글꼴·연결을 막는다
/// (서버 API는 Rust가 부른다 — 웹뷰는 밖에 나가지 않는다). 웹캠 영상은 getUserMedia·RTCPeerConnection이라 정책 밖이다.
pub const CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; media-src 'self' blob: mediastream:; worker-src 'self' blob:; connect-src 'self' ws://127.0.0.1:*";

/// `<head>`에 넣는 CSP 메타.
pub fn csp_head() -> String {
    format!(r#"<meta http-equiv="Content-Security-Policy" content="{CSP}">"#)
}

/// `/monaco/…` 요청을 구운 자산으로 답한다.
pub fn use_monaco_assets() {
    use_asset_handler(editor_assets::PREFIX, |request, responder| {
        let path = request.uri().path().to_owned();
        let response = match editor_assets::get(&path) {
            Some((bytes, mime)) => Response::builder()
                .status(200)
                .header("Content-Type", mime)
                .header("Cache-Control", "no-store")
                .body(Cow::Borrowed(bytes)),
            None => Response::builder()
                .status(404)
                .body(Cow::Borrowed(b"not found".as_slice())),
        };
        responder.respond(response.expect("응답"));
    });
}

/// 편집기 칸: Monaco를 띄우고 다리를 잇는다. 칸이 사라지면 다리를 닫고 클립보드 가로채기를 푼다.
#[component]
pub fn EditorHost() -> Element {
    let ui = use_context::<Ui>();
    {
        let ui = ui.clone();
        use_future(move || {
            let ui = ui.clone();
            async move {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
                let mut editor_tx = ui.editor_tx;
                editor_tx.set(Some(tx));
                let init = ui.editor.read().as_ref().map(|e| {
                    json!({
                        "language": e.language().as_str(),
                        "code": e.code(),
                        "guard": ui.guarded(),
                        "readOnly": false,
                    })
                });
                let Some(init) = init else { return };
                let mut eval = document::eval(EDITOR_JS);
                if eval.send(init).is_err() {
                    return;
                }
                loop {
                    tokio::select! {
                        Some(m) = rx.recv() => {
                            if eval.send(m).is_err() {
                                break;
                            }
                        }
                        got = eval.recv::<Value>() => match got {
                            Ok(m) => ui.on_editor(m),
                            Err(_) => break,
                        },
                    }
                }
            }
        });
    }
    use_drop(move || {
        let mut editor_tx = ui.editor_tx;
        editor_tx.set(None);
        document::eval("window.__plGuard = false; window.__plEditor = null;");
    });
    rsx! {
        div { id: "pl-editor", class: "editor-host", role: "region", "aria-label": "코드 편집기",
            p { class: "sub editor-loading", "편집기를 여는 중…" }
        }
    }
}
