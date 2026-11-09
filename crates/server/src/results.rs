//! 시험 결과(#10): 응시 현황·점수·과정 지표, CSV 내려받기, 같은 구성원의 회차 비교, 응시자 점수 공개 설정.
//! 결과 보기 역할만 본다(출제자 403, 구성원이 아닌 조직 404). 히든 테스트·제출 코드·케이스별 결과·`failure_detail`은 싣지 않는다.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use promptlab_shared::account::Permission;
use promptlab_shared::api::{
    ExamResults, ExamView, MemberRounds, ResultItem, ResultRow, UpdateExamRequest,
};
use promptlab_shared::exam::Purpose;
use promptlab_shared::retention::{days_left, effective_days};
use promptlab_store::results;

use crate::auth::Account;
use crate::exams::{exam_view, find_exam};
use crate::orgs::require;
use crate::{ApiError, AppState};

/// `GET /orgs/{id}/exams/{exam}/results`
pub async fn exam_results(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id)): Path<(String, String)>,
) -> Result<Json<ExamResults>, ApiError> {
    let org = require(&state, &org_id, &account, Permission::ViewResults).await?;
    let exam = find_exam(&state, org.org_id, &exam_id).await?;
    let mut rows = results::exam_results(&state.pool, exam.id, state.eval_ctx()).await?;
    // 「N일 뒤 삭제」(#54).
    let now = state.clock.now();
    let times: std::collections::HashMap<String, i64> =
        promptlab_store::retention::assignment_delete_times(&state.pool, exam.id)
            .await?
            .into_iter()
            .map(|(id, at)| (id.to_string(), at))
            .collect();
    for row in &mut rows {
        row.delete_at = times.get(&row.invite_id).copied();
        row.delete_in_days = row.delete_at.map(|at| days_left(at, now));
    }
    let retention_days = effective_days(
        promptlab_store::retention::exam_days(&state.pool, exam.id).await?,
        promptlab_store::retention::org_days(&state.pool, org.org_id).await?,
    );
    Ok(Json(ExamResults {
        exam_id: exam.id.to_string(),
        title: exam.title,
        purpose: exam.purpose,
        reveal_scores: exam.reveal_scores,
        rows,
        retention_days,
    }))
}

/// `GET /orgs/{id}/exams/{exam}/results.csv`
pub async fn exam_results_csv(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let org = require(&state, &org_id, &account, Permission::ViewResults).await?;
    let exam = find_exam(&state, org.org_id, &exam_id).await?;
    let rows = results::exam_results(&state.pool, exam.id, state.eval_ctx()).await?;
    let body = results_csv(&rows);
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"results-{}.csv\"", exam.id),
            ),
        ],
        body,
    )
        .into_response())
}

/// `GET /orgs/{id}/rounds`
pub async fn rounds(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
) -> Result<Json<Vec<MemberRounds>>, ApiError> {
    let org = require(&state, &org_id, &account, Permission::ViewResults).await?;
    Ok(Json(results::member_rounds(&state.pool, org.org_id).await?))
}

/// `PATCH /orgs/{id}/exams/{exam}` — 사내 진단의 점수 공개 설정과 채점 설정(w·추가 기준표). 준 칸만 바꾼다.
pub async fn update_exam(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, exam_id)): Path<(String, String)>,
    Json(request): Json<UpdateExamRequest>,
) -> Result<Json<ExamView>, ApiError> {
    let org = require(&state, &org_id, &account, Permission::ManageExams).await?;
    let exam = find_exam(&state, org.org_id, &exam_id).await?;
    if exam.purpose == Purpose::Recruiting && request.reveal_scores.is_some() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "채용 시험은 응시자에게 점수를 공개하지 않는다",
        ));
    }
    if let Some(settings) = &request.settings {
        crate::exams::check_settings(settings)?;
        promptlab_store::exams::set_settings(&state.pool, exam.id, settings).await?;
    }
    let Some(reveal_scores) = request.reveal_scores else {
        return Ok(Json(exam_view(
            find_exam(&state, org.org_id, &exam_id).await?,
        )));
    };
    results::set_reveal_scores(&state.pool, exam.id, reveal_scores).await?;
    Ok(Json(exam_view(
        find_exam(&state, org.org_id, &exam_id).await?,
    )))
}

/// 한 칸을 CSV로 쓴다. 사람이 넣은 글(이메일·이름)이 `=`·`+`·`-`·`@`(·탭·CR)로 시작하면 앞에 `'`를 붙여
/// 스프레드시트가 수식으로 읽지 못하게 하고, 쉼표·따옴표·줄바꿈이 있으면 따옴표로 감싼다.
pub fn csv_cell(text: &str) -> String {
    let guarded = if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{text}")
    } else {
        text.to_string()
    };
    if guarded.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

fn percent(rate: Option<f64>) -> String {
    rate.map(|r| format!("{:.1}", r * 100.0))
        .unwrap_or_default()
}

fn number(n: Option<u32>) -> String {
    n.map(|n| n.to_string()).unwrap_or_default()
}

/// 0~100 점수 한 칸(소수 한 자리).
fn points(score: Option<f64>) -> String {
    score.map(|s| format!("{s:.1}")).unwrap_or_default()
}

fn item_cells(item: &ResultItem) -> Vec<String> {
    let score = item.score.as_ref();
    vec![
        item.status.label().to_string(),
        item.language
            .map(|l| l.display_name().to_string())
            .unwrap_or_default(),
        item.model.clone().unwrap_or_default(),
        percent(item.pass_rate),
        match (item.passed, item.total) {
            (Some(p), Some(t)) => format!("{p}/{t}"),
            _ => String::new(),
        },
        number(item.elapsed_secs),
        points(score.and_then(|s| s.final_score)),
        points(score.and_then(|s| s.evaluation_score)),
        score
            .map(|s| s.evaluation_status.label().to_string())
            .unwrap_or_default(),
        score.map(|s| s.formula.clone()).unwrap_or_default(),
    ]
}

/// 결과 CSV(UTF-8 BOM으로 시작 — 한글 엑셀 호환, 줄 끝 CRLF).
pub fn results_csv(rows: &[ResultRow]) -> String {
    let items = rows.iter().map(|r| r.items.len()).max().unwrap_or(0);
    let mut head: Vec<String> = [
        "이메일",
        "이름",
        "상태",
        "통과율 평균(%)",
        "최종 점수",
        "AI 활용 평균",
    ]
    .map(String::from)
    .to_vec();
    for n in 1..=items {
        for col in [
            "상태",
            "언어",
            "AI 모델",
            "통과율(%)",
            "통과/전체",
            "소요 시간(초)",
            "최종 점수",
            "AI 활용 평가",
            "평가 상태",
            "계산식",
        ] {
            head.push(format!("문항{n} {col}"));
        }
    }
    let mut out = String::from('\u{feff}');
    let mut line = |cells: Vec<String>| {
        out.push_str(
            &cells
                .iter()
                .map(|c| csv_cell(c))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push_str("\r\n");
    };
    line(head);
    for row in rows {
        let mut cells = vec![
            row.email.clone(),
            row.candidate_name.clone().unwrap_or_default(),
            row.status.label().to_string(),
            percent(row.score),
            points(row.final_score),
            points(row.evaluation_score),
        ];
        for item in &row.items {
            cells.extend(item_cells(item));
        }
        line(cells);
    }
    out
}
