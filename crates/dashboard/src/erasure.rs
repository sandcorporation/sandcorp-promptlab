//! 응시자 데이터 삭제(#55): 조직 관리자가 시험 초대 현황에서 응시자 한 명(배정)의 데이터를 지우고([`ErasePanel`]),
//! 조직 화면에서 삭제 감사 로그를 본다([`ErasureLogPanel`]). 규칙은 `promptlab_shared::erasure`.
//! 되돌릴 수 없으니 응시자 이메일을 그대로 다시 입력해야 버튼이 켜진다.

use dioxus::prelude::*;
use promptlab_shared::erasure::{ErasureRecord, REASON_MAX_CHARS};
use promptlab_shared::retention::Erased;

use crate::live::kst_input;
use crate::{api, notice};

/// 지운 건수를 사람 말로(0건은 뺀다).
fn summary(n: &Erased) -> String {
    let parts = [
        ("응시 배정", n.assignments),
        ("제출", n.submissions),
        ("문항 시작", n.item_starts),
        ("과정 로그 사건", n.log_events),
        ("과정 로그 키", n.log_keys),
        ("로그 거부", n.log_rejections),
        ("무결성 거부", n.integrity_rejections),
        ("웹캠 사진", n.webcam_photos),
        ("웹캠 동의", n.webcam_consents),
        ("AI 리포트", n.reports),
    ];
    let text: Vec<String> = parts
        .iter()
        .filter(|(_, c)| *c > 0)
        .map(|(label, c)| format!("{label} {c}"))
        .collect();
    if text.is_empty() {
        "지운 것 없음".into()
    } else {
        text.join(" · ")
    }
}

/// 응시자 한 명(배정)의 데이터 삭제 확인 칸.
#[component]
pub fn ErasePanel(
    org_id: String,
    exam_id: String,
    invite_id: String,
    email: String,
    on_erased: EventHandler<()>,
    on_close: EventHandler<()>,
) -> Element {
    let mut reason = use_signal(String::new);
    let mut typed = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut done = use_signal(|| None::<ErasureRecord>);

    let expected = email.trim().to_lowercase();
    let matches = typed.read().trim().to_lowercase() == expected;
    let erase = move |_| {
        let (org_id, exam_id, invite_id) = (org_id.clone(), exam_id.clone(), invite_id.clone());
        let reason = Some(reason.read().trim().to_owned()).filter(|r| !r.is_empty());
        error.set(None);
        busy.set(true);
        spawn(async move {
            match api::erase_assignment(&org_id, &exam_id, &invite_id, reason).await {
                Ok(record) => {
                    done.set(Some(record));
                    on_erased.call(());
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "sc-panel-section erase",
            div { class: "row",
                button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| on_close.call(()), "닫기" }
            }
            if let Some(record) = done.read().clone() {
                p { "삭제했습니다. 감사 로그에 기록됐습니다(조직 화면 「응시자 데이터 삭제 기록」)." }
                p { class: "muted small", "{summary(&record.erased)}" }
            } else {
                p { "대상: " span { class: "email", "{email}" } }
                p { class: "muted small",
                    "이 시험에서 이 응시자의 데이터를 지금 바로 영구 삭제합니다. 되돌릴 수 없습니다. 지우는 것:"
                }
                ul { class: "scope",
                    li { "초대·배정(이메일), 제출 코드와 채점 결과, 문항 시작 기록" }
                    li { "과정 로그, 의심 신호(무결성 거부), 편집 기록·AI 대화, 웹캠 사진, AI 리포트·평가" }
                    li { "웹캠 동의·기준 사진·스냅샷(30일 보유를 기다리지 않고 즉시)" }
                }
                p { class: "muted small",
                    "푸는 중이거나 채점을 기다리는 응시도 지웁니다 — 응시자는 더 이어서 풀 수 없습니다. "
                    "결과·CSV·회차 비교에서 빠지고, 사용량의 응시 수·비용 숫자만 익명으로 남습니다. "
                    "감사 로그에는 누가·언제·무엇을(건수)·사유가 남고, 이메일은 해시로만 남습니다."
                }
                label { class: "muted small", "사유(선택, 이름·이메일은 적지 마세요)" }
                textarea {
                    class: "sc-input",
                    rows: "2",
                    maxlength: "{REASON_MAX_CHARS}",
                    placeholder: "예: 응시자 삭제 요청(2026-11-06 메일)",
                    value: "{reason}",
                    oninput: move |e| reason.set(e.value()),
                }
                label { class: "muted small", "확인을 위해 응시자 이메일을 그대로 입력하세요" }
                input {
                    class: "sc-input confirm",
                    r#type: "email",
                    autocomplete: "off",
                    placeholder: "{email}",
                    value: "{typed}",
                    oninput: move |e| typed.set(e.value()),
                }
                if let Some(e) = error() {
                    {notice(e)}
                }
                div { class: "row",
                    button {
                        class: "sc-btn sc-btn--danger sc-btn--sm",
                        disabled: !matches || busy(),
                        onclick: erase,
                        if busy() { "삭제하는 중…" } else { "영구 삭제" }
                    }
                }
            }
        }
    }
}

/// 조직의 삭제 감사 로그(관리자).
#[component]
pub fn ErasureLogPanel(org_id: String) -> Element {
    let mut query = use_signal(String::new);
    let mut applied = use_signal(|| None::<String>);
    let id = org_id.clone();
    let mut log = use_resource(move || {
        let org_id = id.clone();
        let email = applied();
        async move { api::erasures(&org_id, email.as_deref()).await }
    });

    let body = match &*log.read() {
        None => rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(log)) => {
            let records = log.records.clone();
            let years = log.retention_days / 365;
            rsx! {
                p { class: "muted small",
                    "관리자가 지운 응시자 데이터의 기록입니다. 지운 내용과 이메일 원문은 남지 않고, 응시자는 해시로만 표시됩니다. "
                    "기록은 {years}년 보관 뒤 자동으로 삭제됩니다."
                }
                if records.is_empty() {
                    p { class: "muted", if applied().is_some() { "이 이메일로 지운 기록이 없습니다." } else { "아직 지운 기록이 없습니다." } }
                } else {
                    ul { class: "records",
                        for r in records {
                            li { key: "{r.id}",
                                div {
                                    "{kst_input(r.at).replace('T', \" \")} · {r.by_name.clone().unwrap_or(r.by_sub.clone())} · "
                                    "{r.exam_title.clone().unwrap_or_else(|| \"(지워진 시험)\".into())}"
                                }
                                div { class: "muted small",
                                    "응시자 "
                                    code { "{&r.subject_hash[..12.min(r.subject_hash.len())]}" }
                                    " · {summary(&r.erased)}"
                                }
                                if let Some(reason) = &r.reason {
                                    div { class: "muted small", "사유: {reason}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    rsx! {
        section { class: "sc-panel sc-panel-section erasures",
            div { class: "row spread",
                h3 { "응시자 데이터 삭제 기록" }
                button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| log.restart(), "새로고침" }
            }
            div { class: "search",
                input {
                    class: "sc-input",
                    r#type: "email",
                    placeholder: "응시자 이메일로 찾기",
                    value: "{query}",
                    oninput: move |e| query.set(e.value()),
                }
                button {
                    class: "sc-btn sc-btn--secondary sc-btn--sm",
                    onclick: move |_| {
                        let q = query.read().trim().to_owned();
                        applied.set(Some(q).filter(|q| !q.is_empty()));
                    },
                    "찾기"
                }
                if applied().is_some() {
                    button {
                        class: "sc-btn sc-btn--secondary sc-btn--sm",
                        onclick: move |_| {
                            query.set(String::new());
                            applied.set(None);
                        },
                        "전체"
                    }
                }
            }
            {body}
        }
    }
}
