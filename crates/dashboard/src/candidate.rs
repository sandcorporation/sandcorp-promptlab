//! 응시자 상세(#75 C): 응시 결과 표의 줄을 누르면 오는 페이지.
//!
//! 탭 다섯: 점수(제출 코드·편집 타임라인·AI 대화·로컬 실행·평가 근거, #100), 과정 다시보기, 의심 신호,
//! 웹캠 사진, AI 리포트. 데이터 삭제(관리자)는 머리의 단추로 여는 `sc-dialog` 확인 상자.
//! 결과 보기 권한만 연다(서버가 강제하고 화면은 같은 역할 표로 가린다).

use dioxus::prelude::*;
use promptlab_shared::account::Permission;
use promptlab_shared::api::{OrgView, ResultItem, ResultRow};
use promptlab_shared::evaluation::{CriterionScore, ItemScore, ScoreBasis};

use crate::activity_view::ItemActivityPanel;
use crate::erasure::ErasePanel;
use crate::replay::ReplayPanel;
use crate::report::ReportPanel;
use crate::results::{clock, points};
use crate::route::{CandTab, ExamTab, OrgPage, Route};
use crate::shell::{PageHead, Tabs, go, use_crumb_tail};
use crate::signals::SignalsPanel;
use crate::timeline::anchor;
use crate::webcam::WebcamPanel;
use crate::{api, notice};

const ERASE_DIALOG: &str = "erase-dialog";

#[component]
pub fn CandidatePage(org: OrgView, exam_id: String, invite_id: String, tab: CandTab) -> Element {
    let role = org.my_role;
    if !role.allows(Permission::ViewResults) {
        return notice("응시자 결과를 볼 권한이 없습니다.".into());
    }
    let can_proctor = role.allows(Permission::Proctor);
    let can_erase = role.allows(Permission::ManageMembers);

    let mut crumb = use_crumb_tail();
    let ids = (org.id.clone(), exam_id.clone());
    let mut results = use_resource(move || {
        let (org_id, exam_id) = ids.clone();
        async move { api::exam_results(&org_id, &exam_id).await }
    });
    // 문항 번호(다시보기·대화 칸이 같이 쓴다).
    let position = use_signal(|| 0u32);
    let mut erased = use_signal(|| false);

    let (row, title): (ResultRow, String) = match &*results.read() {
        None => return rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => return notice(e.to_string()),
        Some(Ok(r)) => match r.rows.iter().find(|r| r.invite_id == invite_id) {
            Some(row) => (row.clone(), r.title.clone()),
            None => {
                return rsx! {
                    {notice("그런 응시자가 없습니다(삭제됐을 수 있습니다).".into())}
                    p { a { href: Route::org(&org.id, OrgPage::Exam { exam: exam_id.clone(), tab: ExamTab::Results }).href(), "응시 결과로" } }
                };
            }
        },
    };
    {
        let title = title.clone();
        use_effect(move || crumb.set(Some(title.clone())));
    }

    let org_id = org.id.clone();
    let back = Route::org(
        &org_id,
        OrgPage::Exam {
            exam: exam_id.clone(),
            tab: ExamTab::Results,
        },
    );
    let tabs: Vec<(String, Route)> = CandTab::ALL
        .into_iter()
        .filter(|t| *t != CandTab::Webcam || can_proctor)
        .map(|t| {
            (
                t.label().to_owned(),
                Route::org(
                    &org_id,
                    OrgPage::Candidate {
                        exam: exam_id.clone(),
                        invite: invite_id.clone(),
                        tab: t,
                    },
                ),
            )
        })
        .collect();
    let here = Route::org(
        &org_id,
        OrgPage::Candidate {
            exam: exam_id.clone(),
            invite: invite_id.clone(),
            tab,
        },
    );
    let who = match &row.candidate_name {
        Some(name) => format!("{name} <{}>", row.email),
        None => row.email.clone(),
    };
    let sub = format!(
        "{} · 최종 점수 {}",
        row.status.label(),
        points(row.final_score)
    );

    let content = match tab {
        CandTab::Conversations => rsx! {
            ConversationsView {
                org_id: org_id.clone(),
                exam_id: exam_id.clone(),
                invite_id: invite_id.clone(),
                items: row.items.clone(),
                position,
            }
        },
        CandTab::Replay => rsx! {
            ReplayChooser { items: row.items.clone(), position }
            ReplayPanel {
                org_id: org_id.clone(),
                exam_id: exam_id.clone(),
                invite_id: invite_id.clone(),
                position: position(),
                on_close: {
                    let to = Route::org(&org_id, OrgPage::Candidate { exam: exam_id.clone(), invite: invite_id.clone(), tab: CandTab::Conversations });
                    move |_| go(to.clone())
                },
            }
        },
        CandTab::Signals => rsx! {
            SignalsPanel {
                org_id: org_id.clone(),
                exam_id: exam_id.clone(),
                invite_id: invite_id.clone(),
                on_close: {
                    let to = here.clone();
                    move |_| go(to.clone())
                },
            }
        },
        CandTab::Webcam if can_proctor => rsx! {
            WebcamPanel {
                org_id: org_id.clone(),
                exam_id: exam_id.clone(),
                invite_id: invite_id.clone(),
                on_close: {
                    let to = here.clone();
                    move |_| go(to.clone())
                },
            }
        },
        CandTab::Webcam => notice("웹캠 사진은 감독 권한이 필요합니다.".into()),
        CandTab::Report => rsx! {
            ReportPanel {
                org_id: org_id.clone(),
                exam_id: exam_id.clone(),
                invite_id: invite_id.clone(),
                candidate: who.clone(),
                on_close: {
                    let to = here.clone();
                    move |_| go(to.clone())
                },
            }
        },
    };

    let actions = rsx! {
        a { class: "sc-btn sc-btn--secondary", href: "{back.href()}", "← 응시 결과" }
        if can_erase && !erased() {
            button {
                class: "sc-btn sc-btn--danger",
                r#type: "button",
                onclick: move |_| api::show_modal(ERASE_DIALOG),
                "데이터 삭제"
            }
        }
    };

    rsx! {
        PageHead { title: who.clone(), sub: sub, actions: actions }
        Tabs { label: "응시자 상세".to_owned(), tabs: tabs, current: here.clone() }
        if erased() {
            div { class: "sc-alert", role: "status",
                "이 응시자의 데이터를 삭제했습니다. "
                a { href: "{back.href()}", "응시 결과로 돌아가기" }
            }
        } else {
            {content}
        }
        if can_erase {
            dialog { class: "sc-dialog", id: ERASE_DIALOG, "aria-labelledby": "erase-title",
                h2 { id: "erase-title", class: "sc-panel-title", "응시자 데이터 삭제" }
                ErasePanel {
                    key: "erase-{invite_id}",
                    org_id: org_id.clone(),
                    exam_id: exam_id.clone(),
                    invite_id: invite_id.clone(),
                    email: row.email.clone(),
                    on_erased: move |_| {
                        erased.set(true);
                        results.restart();
                    },
                    on_close: move |_| api::close_modal(ERASE_DIALOG),
                }
            }
        }
    }
}

/// 문항 고르기 칩(다시보기 탭용).
#[component]
fn ReplayChooser(items: Vec<ResultItem>, position: Signal<u32>) -> Element {
    rsx! { {item_chips(&items, position)} }
}

fn item_chips(items: &[ResultItem], mut position: Signal<u32>) -> Element {
    let started: Vec<u32> = items
        .iter()
        .filter(|i| i.status != promptlab_shared::api::ResultItemStatus::NotStarted)
        .map(|i| i.position)
        .collect();
    rsx! {
        div { class: "sc-toolbar-chips", role: "group", "aria-label": "문항",
            for n in started {
                button {
                    class: "sc-chip",
                    r#type: "button",
                    "aria-pressed": if position() == n { "true" } else { "false" },
                    onclick: move |_| position.set(n),
                    "문항 {n + 1}"
                }
            }
        }
    }
}

/// 점수 탭: 가운데 = 제출 코드·편집 타임라인·AI 대화·로컬 실행(`activity_view`), 오른쪽 칸 = 점수 계산식·평가 항목·근거 칩.
#[component]
fn ConversationsView(
    org_id: String,
    exam_id: String,
    invite_id: String,
    items: Vec<ResultItem>,
    position: Signal<u32>,
) -> Element {
    let picked = use_signal(|| None::<String>);
    let item = items.iter().find(|i| i.position == position()).cloned();
    rsx! {
        {item_chips(&items, position)}
        div { class: "sc-split",
            div { class: "split-main",
                ItemActivityPanel {
                    key: "{position()}",
                    org_id: org_id.clone(),
                    exam_id: exam_id.clone(),
                    invite_id: invite_id.clone(),
                    position: position(),
                    picked,
                }
            }
            aside { class: "sc-split-rail", "aria-label": "점수",
                if let Some(item) = item {
                    {score_panels(&item, picked)}
                }
            }
        }
    }
}

fn basis_text(b: ScoreBasis) -> &'static str {
    match b {
        ScoreBasis::Weighted => "통과율 + AI 활용",
        ScoreBasis::HiddenOnly => "통과율만(평가 없음)",
        ScoreBasis::GradingFailed => "채점 실패 — 0점",
        ScoreBasis::Pending => "기다리는 중",
    }
}

fn score_panels(item: &ResultItem, mut picked: Signal<Option<String>>) -> Element {
    let metrics = {
        let mut parts: Vec<String> = Vec::new();
        if let Some(language) = item.language {
            parts.push(language.display_name().to_owned());
        }
        if let Some(s) = item.elapsed_secs {
            parts.push(format!("소요 {}", clock(s)));
        }
        if let (Some(p), Some(t)) = (item.passed, item.total) {
            parts.push(format!("히든 테스트 {p}/{t} 통과"));
        }
        parts.join(" · ")
    };
    let score: Option<ItemScore> = item.score.clone();
    rsx! {
        section { class: "sc-panel sc-panel-section", "aria-label": "문항 점수",
            h2 { class: "sc-panel-title", "문항 {item.position + 1} 점수" }
            if let Some(s) = score {
                p { class: "sc-stat-value sc-num", "{points(s.final_score)}" }
                p { class: "muted small", "{basis_text(s.basis)}" }
                p { class: "small", "{s.formula}" }
                dl { class: "kv",
                    dt { "통과율" }
                    dd { class: "sc-num", "{points(s.hidden_score)} · 비율 {s.weight}%" }
                    dt { "AI 활용 평가" }
                    dd { class: "sc-num", "{points(s.evaluation_score)} · 비율 {100 - u32::from(s.weight)}%" }
                }
            } else {
                p { class: "muted", "아직 점수가 없습니다({item.status.label()})." }
            }
            if !metrics.is_empty() {
                p { class: "muted small", "{metrics}" }
            }
        }
        if let Some(ev) = item.score.as_ref().and_then(|s| s.evaluation.clone()) {
            section { class: "sc-panel sc-panel-section", "aria-label": "AI 활용 평가",
                h2 { class: "sc-panel-title", "AI 활용 평가 {points(Some(ev.score))}" }
                for c in ev.criteria.iter() {
                    {criterion(c, &mut picked)}
                }
                if ev.dropped_evidence > 0 {
                    p { class: "muted small", "실제 없는 턴을 가리켜 버린 근거 {ev.dropped_evidence}개" }
                }
                p { class: "muted small", "평가 모델 {ev.model} — 점수는 통과율와 비율대로 합쳐집니다." }
            }
        }
    }
}

fn criterion(c: &CriterionScore, picked: &mut Signal<Option<String>>) -> Element {
    let mut picked = *picked;
    let pct = if c.max == 0 {
        0.0
    } else {
        f64::from(c.score) / f64::from(c.max) * 100.0
    };
    rsx! {
        div { class: "criterion",
            div { class: "spread",
                strong { "{c.name}" }
                span { class: "sc-num", "{c.score}/{c.max}" }
            }
            div { class: "sc-meter", span { class: "sc-meter-fill", style: "--sc-value: {pct}%" } }
            if !c.reason.is_empty() {
                p { class: "small", "{c.reason}" }
            }
            div { class: "sc-toolbar-chips",
                for e in c.evidence.iter() {
                    {
                        let label = e.label();
                        let anchor = anchor(e);
                        rsx! {
                            button {
                                class: "sc-chip",
                                r#type: "button",
                                title: "그 기록으로",
                                onclick: move |_| {
                                    picked.set(Some(anchor.clone()));
                                    scroll_to(&anchor);
                                },
                                "{label}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 대화 칸이 다시 그려진 뒤 그 턴으로 스크롤한다(브라우저만).
fn scroll_to(anchor: &str) {
    let anchor = anchor.to_owned();
    spawn(async move {
        api::sleep_ms(50).await;
        api::scroll_into_view(&anchor);
    });
}
