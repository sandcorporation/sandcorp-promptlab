//! 사용량·추정 비용(#15): 관리자(구성원 관리)가 조직의 응시 수와 모델 호출 종류별(채점·AI 리포트·풀림 미리
//! 돌리기·공개 예시 대조) 토큰·추정 비용을 본다. 비용은 호출할 때의 단가로 서버가 계산해 저장한 값의 합이다
//! (로컬 모델 단가는 0 — `shared::report::LOCAL_PRICES`).

use dioxus::prelude::*;
use promptlab_shared::report::{KindUsage, OrgUsage, UsageTotals};

use crate::{api, notice};

/// 사용량 칸(관리자만 — 서버가 강제한다).
#[component]
pub fn UsagePanel(org_id: String) -> Element {
    let mut usage = use_resource(move || {
        let org_id = org_id.clone();
        async move { api::usage(&org_id).await }
    });
    let body = match &*usage.read() {
        None => rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(u)) => usage_body(u),
    };
    rsx! {
        section { class: "sc-panel sc-panel-section usage-card",
            div { class: "row spread",
                h2 { class: "sc-panel-title", "사용량·추정 비용" }
                button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| usage.restart(), "새로고침" }
            }
            {body}
        }
    }
}

/// 천 단위 쉼표.
pub(crate) fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub(crate) fn usd(cost: f64) -> String {
    if cost > 0.0 && cost < 0.01 {
        "< $0.01".to_owned()
    } else {
        format!("${cost:.2}")
    }
}

/// 입력 토큰(캐시 읽기·쓰기 포함).
fn tokens_in(t: &UsageTotals) -> u64 {
    t.input_tokens + t.cache_read_tokens + t.cache_write_tokens
}

pub(crate) fn kind_table(rows: &[KindUsage]) -> Element {
    let calls: u64 = rows.iter().map(|k| k.totals.calls).sum();
    let tin: u64 = rows.iter().map(|k| tokens_in(&k.totals)).sum();
    let tout: u64 = rows.iter().map(|k| k.totals.output_tokens).sum();
    let cost: f64 = rows.iter().map(|k| k.totals.cost_usd).sum();
    rsx! {
        div { class: "sc-table-scroll",
        table { class: "sc-table",
            thead {
                tr {
                    th { "종류" }
                    th { class: "sc-num", "호출" }
                    th { class: "sc-num", "입력 토큰" }
                    th { class: "sc-num", "출력 토큰" }
                    th { class: "sc-num", "추정 비용" }
                }
            }
            tbody {
                for k in rows.iter() {
                    tr {
                        td { "{k.kind.label()}" }
                        td { class: "sc-num", "{thousands(k.totals.calls)}" }
                        td { class: "sc-num", "{thousands(tokens_in(&k.totals))}" }
                        td { class: "sc-num", "{thousands(k.totals.output_tokens)}" }
                        td { class: "sc-num", "{usd(k.totals.cost_usd)}" }
                    }
                }
                tr { class: "total",
                    td { "합" }
                    td { class: "sc-num", "{thousands(calls)}" }
                    td { class: "sc-num", "{thousands(tin)}" }
                    td { class: "sc-num", "{thousands(tout)}" }
                    td { class: "sc-num", "{usd(cost)}" }
                }
            }
        }
        }
    }
}

fn usage_body(u: &OrgUsage) -> Element {
    rsx! {
        p { "응시 {thousands(u.attempts)}건 · 추정 비용 {usd(u.cost_usd)}" }
        {kind_table(&u.by_kind)}
        p { class: "muted small",
            "추정 비용은 호출 때의 단가로 계산한 값이라 실제 청구액과 다를 수 있습니다. 채점·미리 돌리기·대조는 우리 서버의 로컬 모델이라 비용 0으로 세고 토큰·처리 시간만 쌓습니다."
        }
        if !u.exams.is_empty() {
            h3 { "시험별" }
            for e in u.exams.iter() {
                details {
                    summary { "{e.title} — 응시 {thousands(e.attempts)}건 · {usd(e.by_kind.iter().map(|k| k.totals.cost_usd).sum())}" }
                    {kind_table(&e.by_kind)}
                }
            }
        }
    }
}
