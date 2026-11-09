//! 조직 문제 화면(#16·#73·#75 B): 출제자·관리자가 자기 조직 문제를 표로 보고(목록) 열어 고친다(편집 화면은
//! [`crate::problem_editor`] — 서명 편집기·케이스 JSON·모범 답안·4개 언어 틀 미리보기 + 점검 패널, 저장 → 점검 → 발행).
//!
//! 히든 테스트·모범 답안은 이 화면(시험 관리 역할)에서만 보인다 — 서버가 역할로 막는다.
//! 검증 규칙은 서버와 같은 [`ProblemSource::validate`].

use dioxus::prelude::*;
use promptlab_shared::account::Permission;
use promptlab_shared::api::OrgView;
use promptlab_shared::authoring::{OrgProblem, ProblemSource};
use promptlab_shared::coding::{
    DEFAULT_CASE_TIME_MS, Language, Limits, Param, Signature, TestCase, ValueType,
};

use crate::problem_editor::ProblemEditor;
use crate::shell::PageHead;
use crate::signals::kst;
use crate::{api, notice};

/// 새 문제의 뼈대(채워 고치라고 보이는 예).
pub(crate) fn blank_source() -> ProblemSource {
    let case = |a: i64, b: i64| TestCase {
        args: vec![serde_json::json!(a), serde_json::json!(b)],
        expected: serde_json::json!(a + b),
    };
    ProblemSource {
        title: "두 수 더하기".into(),
        description: "정수 `a`와 `b`의 합을 돌려준다.".into(),
        signature: Signature {
            name: "add".into(),
            params: vec![
                Param {
                    name: "a".into(),
                    ty: ValueType::Int,
                },
                Param {
                    name: "b".into(),
                    ty: ValueType::Int,
                },
            ],
            returns: ValueType::Int,
        },
        examples: vec![case(1, 2)],
        hidden: (0..5).map(|i| case(i * 7, -i)).collect(),
        reference_solution: "def add(a, b):\n    return a + b\n".into(),
        reference_language: Language::Python,
        limits: Limits {
            case_time_ms: DEFAULT_CASE_TIME_MS,
        },
    }
}

/// 편집 대상: 새 문제(`None`) 또는 불러온 문제.
#[derive(Debug, Clone, PartialEq)]
struct Editing(Option<OrgProblem>);

/// 조직의 문제 페이지(시험 관리 역할에게만 보인다): 목록 표 ↔ 편집 화면.
#[component]
pub fn ProblemsPanel(org: OrgView) -> Element {
    let can_manage = org.my_role.allows(Permission::ManageExams);
    let org_id = org.id.clone();
    let mut list = use_resource(move || {
        let org_id = org_id.clone();
        async move { api::org_problems(&org_id).await }
    });
    let mut editing = use_signal(|| None::<Editing>);
    let mut error = use_signal(|| None::<String>);
    if !can_manage {
        return rsx! { {notice("문제를 만들고 고치는 권한이 없습니다.".into())} };
    }

    if let Some(Editing(initial)) = editing.read().clone() {
        return rsx! {
            ProblemEditor {
                key: "{initial.as_ref().map(|p| p.id.as_str()).unwrap_or(\"new\")}",
                target: EditTarget::Org(org.id.clone()),
                initial,
                on_saved: move |_| list.restart(),
                on_close: move |_| {
                    list.restart();
                    editing.set(None);
                },
            }
        };
    }

    let rows = match &*list.read() {
        None => rsx! { p { class: "sc-panel-section muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(items)) if items.is_empty() => rsx! {
            p { class: "sc-panel-section muted", "아직 우리 조직 문제가 없습니다. 만든 문제는 점검을 통과해 발행하면 이 조직의 시험에서 쓸 수 있습니다." }
        },
        Some(Ok(items)) => {
            let items = items.clone();
            rsx! {
                div { class: "sc-table-scroll",
                    table { class: "sc-table",
                        thead {
                            tr {
                                th { scope: "col", "문제" }
                                th { scope: "col", class: "sc-num", "예제" }
                                th { scope: "col", class: "sc-num", "히든 테스트" }
                                th { scope: "col", "상태" }
                                th { scope: "col", "고친 때" }
                                th { scope: "col", class: "sc-num", span { class: "sc-visually-hidden", "열기" } }
                            }
                        }
                        tbody {
                            for p in items {
                                tr { key: "{p.id}",
                                    td {
                                        "{short(&p.title, 70)}"
                                        span { class: "sc-table-sub", "{p.id}" }
                                    }
                                    td { class: "sc-num", "{p.example_count}" }
                                    td { class: "sc-num", "{p.hidden_count}" }
                                    td {
                                        if p.published {
                                            span { class: "sc-badge sc-badge--success", "발행됨" }
                                        } else {
                                            span { class: "sc-badge sc-badge--neutral", "초안" }
                                        }
                                    }
                                    td { class: "muted small", "{kst(p.updated_at * 1000)}" }
                                    td { class: "sc-num",
                                        button {
                                            class: "sc-btn sc-btn--secondary sc-btn--sm",
                                            onclick: {
                                                let org_id = org.id.clone();
                                                let id = p.id.clone();
                                                move |_| {
                                                    let org_id = org_id.clone();
                                                    let id = id.clone();
                                                    spawn(async move {
                                                        match api::org_problem(&org_id, &id).await {
                                                            Ok(problem) => {
                                                                error.set(None);
                                                                editing.set(Some(Editing(Some(problem))));
                                                            }
                                                            Err(e) => error.set(Some(e.to_string())),
                                                        }
                                                    });
                                                }
                                            },
                                            "열기"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    rsx! {
        PageHead {
            title: "문제",
            sub: "우리 조직 문제를 만들고 발행 전 점검을 돌립니다. 발행한 문제만 시험에 씁니다.",
            actions: rsx! {
                button { class: "sc-btn sc-btn--accent sc-btn--sm", onclick: move |_| editing.set(Some(Editing(None))), "새 문제 만들기" }
            },
        }
        if let Some(text) = error.read().clone() {
            {notice(text)}
        }
        section { class: "sc-panel", "aria-label": "우리 조직 문제", {rows} }
    }
}

pub(crate) fn short(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

/// 편집기가 저장하는 곳.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditTarget {
    /// 조직 문제(#16) — 발행 전 점검·발행(#73).
    Org(String),
    /// 운영자 문제은행 문항(#19). 연습/시험 구분은 만들 때 정한다. 점검·발행은 `/operator/*`.
    Operator { practice: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 뼈대는_검증을_통과한다() {
        assert_eq!(blank_source().validate(), Ok(()));
    }
}
