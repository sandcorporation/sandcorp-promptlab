//! 보관 기간(#54): 조직 기본값(관리자가 바꾼다)과 시험별 값(시험 관리 역할이 바꾼다), 바뀐 이력.
//! 규칙은 `promptlab_shared::retention` — 시험별 → 조직 기본 → 365일, 30~1095일.

use dioxus::prelude::*;
use promptlab_shared::retention::RetentionView;

use crate::live::kst_input;
use crate::{api, notice};

fn days_text(days: Option<u32>) -> String {
    days.map(|d| format!("{d}일"))
        .unwrap_or_else(|| "미설정".into())
}

/// 지금 값이 어디서 왔나.
fn source(v: &RetentionView, exam: bool) -> String {
    match (exam, v.exam_days, v.org_days) {
        (true, Some(_), _) => "이 시험에 정한 값".into(),
        (true, None, Some(_)) => "조직 기본값을 따름".into(),
        (false, _, Some(_)) => "조직 기본값".into(),
        _ => format!("기본값({}일)", v.default_days),
    }
}

/// 보관 기간 칸. `exam_id`가 없으면 조직 기본값.
#[component]
pub fn RetentionPanel(org_id: String, exam_id: Option<String>, can_edit: bool) -> Element {
    let ids = (org_id.clone(), exam_id.clone());
    let mut view = use_resource(move || {
        let (org_id, exam_id) = ids.clone();
        async move { api::retention(&org_id, exam_id.as_deref()).await }
    });
    let mut input = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);
    let is_exam = exam_id.is_some();

    let save = move |days: Option<u32>| {
        let (org_id, exam_id) = (org_id.clone(), exam_id.clone());
        error.set(None);
        saving.set(true);
        spawn(async move {
            match api::set_retention(&org_id, exam_id.as_deref(), days).await {
                Ok(_) => {
                    input.set(String::new());
                    view.restart();
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    let v = match &*view.read() {
        None => {
            return rsx! { section { class: "sc-panel sc-panel-section retention", p { class: "muted", "보관 기간을 불러오는 중…" } } };
        }
        Some(Err(e)) => return notice(e.to_string()),
        Some(Ok(v)) => v.clone(),
    };
    let own = if is_exam { v.exam_days } else { v.org_days };
    let (min, max) = (v.min_days, v.max_days);
    let mut save_typed = save.clone();
    let submit = move |_| match input().trim().parse::<u32>() {
        Ok(d) if (min..=max).contains(&d) => save_typed(Some(d)),
        _ => error.set(Some(format!("{min}~{max} 사이의 날 수를 넣으세요"))),
    };
    let mut reset = save.clone();
    let history: Vec<_> = v.history.iter().take(5).cloned().collect();

    rsx! {
        section { class: "sc-panel sc-panel-section retention",
            div { class: "row spread",
                h3 { if is_exam { "이 시험의 보관 기간" } else { "응시 기록 보관 기간" } }
                span { class: "sc-badge sc-badge--neutral", "{v.effective_days}일" }
            }
            p { class: "muted small",
                "응시 기록·제출 프롬프트·과정 로그·AI 리포트는 응시가 끝난 뒤(응시 가능 시간이 정해져 있으면 그 끝 뒤) 이 기간이 지나면 자동으로 삭제됩니다. "
                "웹캠 사진은 따로 30일입니다. 지금 값: {source(&v, is_exam)}."
            }
            if can_edit {
                div { class: "row",
                    input {
                        class: "sc-input days",
                        r#type: "number",
                        min: "{min}",
                        max: "{max}",
                        placeholder: "{v.effective_days}",
                        value: "{input}",
                        oninput: move |e| input.set(e.value()),
                    }
                    span { class: "muted small", "일 ({min}~{max})" }
                    button { class: "sc-btn sc-btn--accent sc-btn--sm", disabled: saving(), onclick: submit, "저장" }
                    if own.is_some() {
                        button {
                            class: "sc-btn sc-btn--secondary sc-btn--sm",
                            disabled: saving(),
                            onclick: move |_| reset(None),
                            if is_exam { "조직 기본값 따르기" } else { "기본값(365일)으로" }
                        }
                    }
                }
                p { class: "muted small", "줄이면 다음 정리(1시간 안)에 이미 기간이 지난 응시가 삭제되며 되돌릴 수 없습니다." }
            }
            if let Some(e) = error() {
                p { class: "sc-alert sc-alert--error", role: "alert", "{e}" }
            }
            if !history.is_empty() {
                ul { class: "history",
                    for c in history {
                        li {
                            "{kst_input(c.at).replace('T', \" \")} · {c.by_name.clone().unwrap_or(c.by_sub.clone())} · "
                            if c.exam_id.is_some() && !is_exam { "시험별 " } else if c.exam_id.is_none() && is_exam { "조직 기본 " } else { "" }
                            "{days_text(c.old_days)} → {days_text(c.new_days)}"
                        }
                    }
                }
            }
        }
    }
}
