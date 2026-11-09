//! 제출 ContentDialog: 되돌릴 수 없는 제출만 확인을 받는다(나머지 안내는 창 안 InfoBar).

use dioxus::events::KeyboardEvent;
use dioxus::prelude::*;
use promptlab_app::workspace::thousands;
use promptlab_shared::log::LocalRunKind;

use super::{RunView, Ui};

#[component]
pub fn SubmitDialog() -> Element {
    let ui = use_context::<Ui>();
    let mut dialog = ui.dialog;
    let Some((language, bytes)) = ui
        .editor
        .read()
        .as_ref()
        .map(|e| (e.language(), e.code().len()))
    else {
        return rsx! {};
    };
    let position = ui.session.read().exam_item().map(|i| i.position);
    let title = match position {
        Some(p) => format!("문항 {}을 제출할까요?", p + 1),
        None => "이 코드를 제출할까요?".to_owned(),
    };
    let examples = match &*ui.last_examples.read() {
        Some(RunView::Done(Ok(r))) if r.kind == LocalRunKind::Examples => {
            format!("{} (마지막 예제 테스트)", r.summary())
        }
        _ => "아직 예제 테스트를 하지 않음".to_owned(),
    };
    let after = match position {
        Some(p) => format!(
            "제출하면 문항 {}의 코드를 더 고칠 수 없습니다. 숨은 테스트로 채점하고, 남은 시간은 다음 문항에 쓸 수 있습니다.",
            p + 1
        ),
        None => "제출하면 서버가 숨은 테스트로 채점합니다.".to_owned(),
    };
    let ui_submit = ui.clone();
    rsx! {
        div { class: "smoke",
            onkeydown: move |e: KeyboardEvent| {
                if e.key().to_string() == "Escape" {
                    dialog.set(false);
                }
            },
            div { class: "dlg", role: "dialog", "aria-modal": "true", "aria-labelledby": "dlg-title",
                div { class: "dlgb",
                    h2 { id: "dlg-title", "{title}" }
                    div { class: "dlg-info",
                        div { span { class: "k", "언어" } span { "{language.display_name()}" } }
                        div { span { class: "k", "코드" } span { "{thousands(bytes as u32)}바이트" } }
                        div { span { class: "k", "공개 예제" } span { "{examples}" } }
                    }
                    p { class: "muted", "{after}" }
                }
                div { class: "dlgf",
                    button { class: "accent", onclick: move |_| ui_submit.submit_now(), "제출" }
                    button { onclick: move |_| dialog.set(false), "취소" }
                }
            }
        }
    }
}
