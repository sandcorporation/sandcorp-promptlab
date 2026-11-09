//! 채점 워커 라이브러리(spec #93): 채점 큐에서 제출 하나를 꺼내 판정기(`promptlab-judge`, compose 내부 HTTP)로
//! 히든 케이스를 돌리고 통과 수를 저장한다. 채점 큐가 비면 발행 전 점검 하나(모범 답안을 예제 + 히든으로 판정).
//!
//! - 판정기에는 **케이스 인자만** 보낸다. 기대값은 워커가 [`promptlab_judge::passed`]로 비교한다 — 히든 기대값은 워커
//!   밖으로 나가지 않는다.
//! - 컴파일 오류·런타임 오류·시간 초과는 응시자 탓이라 채점 실패가 아니라 그 케이스가 틀린 것이다(컴파일 오류면 0).
//! - 판정기가 요청을 거절하면(422 — 규칙 밖 요청) 채점 실패(짧은 이유, 상세는 DB `failure_detail`에만). 판정기가 잠시
//!   안 되면(503·연결 실패·시간 초과) 큐로 되돌리고, [`submissions::MAX_ATTEMPTS`]번 꺼낸 뒤에는 실패로 남긴다.
//! - 판정기 실행에는 토큰이 없어 비용 기록(`usage_records`)을 남기지 않는다.
//!
//! 시험은 [`JudgeClient`]를 가짜로 바꿔 끼운다 — [`fake::FakeJudge`](서버 시험도 쓴다).

use std::future::Future;
use std::time::Duration;

use promptlab_judge::{RunOutcome, RunRequest};
use promptlab_shared::coding::{CodingProblem, Language, TestCase, solution_template};
use promptlab_shared::grading::GradeReport;
use promptlab_shared::publication::{CheckResult, TemplateCheck};
use promptlab_store::{PgPool, checks, problems, submissions};
use uuid::Uuid;

pub mod fake;

/// [`run_once`] 한 번이 한 일.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 할 일이 없다(채점 큐·점검 큐 모두 빔).
    Idle,
    Graded(Uuid),
    Failed(Uuid),
    /// 판정기가 잠시 안 돼 큐로 되돌렸다(잠시 쉬고 다시).
    Requeued(Uuid),
    /// 발행 전 점검 하나를 끝냈다(통과·실패 모두).
    Checked(Uuid),
}

pub const REASON_UNKNOWN_PROBLEM: &str = "알 수 없는 문제라 채점하지 못했다";
pub const REASON_JUDGE_REJECTED: &str = "채점 서버가 이 제출을 받지 않았다";
pub const REASON_JUDGE_DOWN: &str = "채점 서버 오류로 채점하지 못했다";

/// 판정기 호출 실패.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JudgeCallError {
    /// 규칙 밖 요청(HTTP 422). 다시 해도 같다.
    #[error("판정기가 요청을 거절했다: {0}")]
    Rejected(String),
    /// 판정기가 잠시 안 된다(HTTP 503·연결 실패·시간 초과 등). 다시 하면 될 수 있다.
    #[error("판정기를 쓸 수 없다: {0}")]
    Unavailable(String),
}

/// 판정기 클라이언트. 운영은 [`HttpJudge`], 시험은 [`fake::FakeJudge`].
pub trait JudgeClient: Send + Sync {
    fn run(
        &self,
        req: &RunRequest,
    ) -> impl Future<Output = Result<RunOutcome, JudgeCallError>> + Send;
}

/// 판정기 기본 주소(compose 내부).
pub const DEFAULT_JUDGE_URL: &str = "http://judge:8090";

/// 판정기 HTTP 클라이언트(`POST {url}/run`).
#[derive(Debug, Clone)]
pub struct HttpJudge {
    client: reqwest::Client,
    url: String,
}

impl HttpJudge {
    /// 요청 시간 상한 = 판정기 벽시계 최대 + 여유 30초(동시 실행 대기 포함).
    pub fn new(base_url: &str) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .timeout(promptlab_judge::limits::MAX_WALL + Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self {
            client,
            url: format!("{}/run", base_url.trim_end_matches('/')),
        })
    }
}

impl JudgeClient for HttpJudge {
    async fn run(&self, req: &RunRequest) -> Result<RunOutcome, JudgeCallError> {
        let response = self
            .client
            .post(&self.url)
            .json(req)
            .send()
            .await
            .map_err(|e| JudgeCallError::Unavailable(e.without_url().to_string()))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<RunOutcome>()
                .await
                .map_err(|e| JudgeCallError::Unavailable(e.without_url().to_string()));
        }
        let body = response.text().await.unwrap_or_default();
        let body: String = body.chars().take(500).collect();
        if status == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            Err(JudgeCallError::Rejected(body))
        } else {
            Err(JudgeCallError::Unavailable(format!(
                "HTTP {status}: {body}"
            )))
        }
    }
}

/// 케이스들을 판정기 요청으로(인자만 — 기대값은 싣지 않는다).
fn request(
    problem: &CodingProblem,
    language: Language,
    code: &str,
    cases: &[&TestCase],
) -> RunRequest {
    RunRequest {
        language,
        code: code.to_owned(),
        signature: problem.signature.clone(),
        cases: cases.iter().map(|c| c.args.clone()).collect(),
        case_time_ms: Some(problem.limits.case_time_ms),
    }
}

/// 케이스별 통과(케이스 수만큼 — 컴파일 오류·벽시계 kill로 결과가 모자라면 그 케이스는 실패).
fn case_passed(problem: &CodingProblem, cases: &[&TestCase], outcome: &RunOutcome) -> Vec<bool> {
    let expected: Vec<serde_json::Value> = cases.iter().map(|c| c.expected.clone()).collect();
    let mut passed = promptlab_judge::passed(problem.signature.returns, &expected, outcome);
    passed.resize(cases.len(), false);
    passed
}

/// 큐에서 하나 꺼내 처리한다. 채점 큐가 비었으면 발행 전 점검 하나([`check_once`]).
pub async fn run_once<J: JudgeClient>(pool: &PgPool, judge: &J) -> Result<Outcome, sqlx::Error> {
    let Some(claimed) = submissions::claim_next(pool).await? else {
        return check_once(pool, judge).await;
    };
    let id = claimed.id;
    let Some(problem) = problems::find(pool, &claimed.problem_id).await? else {
        tracing::warn!(%id, problem_id = %claimed.problem_id, "알 수 없는 문제");
        submissions::fail(pool, id, REASON_UNKNOWN_PROBLEM, &claimed.problem_id).await?;
        return Ok(Outcome::Failed(id));
    };
    let cases: Vec<&TestCase> = problem.hidden.iter().collect();
    let req = request(&problem, claimed.language, &claimed.code, &cases);
    let started = std::time::Instant::now();
    match judge.run(&req).await {
        Ok(outcome) => {
            let passed = case_passed(&problem, &cases, &outcome);
            let report = GradeReport::from_cases(&problem.id, claimed.language, &passed);
            tracing::info!(
                %id,
                language = claimed.language.as_str(),
                passed = report.passed,
                total = report.total,
                ms = started.elapsed().as_millis() as u64,
                "채점 끝"
            );
            submissions::complete(pool, id, &report).await?;
            Ok(Outcome::Graded(id))
        }
        Err(JudgeCallError::Rejected(detail)) => {
            tracing::warn!(%id, "판정기가 제출을 거절했다(상세는 DB failure_detail)");
            submissions::fail(pool, id, REASON_JUDGE_REJECTED, &detail).await?;
            Ok(Outcome::Failed(id))
        }
        Err(JudgeCallError::Unavailable(detail)) => {
            if claimed.attempts >= submissions::MAX_ATTEMPTS {
                tracing::error!(%id, attempts = claimed.attempts, "판정기를 쓸 수 없어 채점 실패");
                submissions::fail(pool, id, REASON_JUDGE_DOWN, &detail).await?;
                Ok(Outcome::Failed(id))
            } else {
                tracing::warn!(%id, attempts = claimed.attempts, "판정기를 쓸 수 없어 큐로 되돌린다");
                submissions::requeue(pool, id).await?;
                Ok(Outcome::Requeued(id))
            }
        }
    }
}

/// 서명에서 생성한 언어별 빈 틀이 컴파일되는지 본다(기본값 인자 한 케이스로 판정기를 돌려 컴파일 결과만 읽는다).
pub async fn check_templates<J: JudgeClient>(
    judge: &J,
    problem: &CodingProblem,
) -> Result<Vec<TemplateCheck>, JudgeCallError> {
    let args: Vec<serde_json::Value> = problem
        .signature
        .params
        .iter()
        .map(|p| p.ty.default_value())
        .collect();
    let mut out = Vec::new();
    for language in Language::ALL {
        let req = RunRequest {
            language,
            code: solution_template(language, &problem.signature),
            signature: problem.signature.clone(),
            cases: vec![args.clone()],
            case_time_ms: Some(problem.limits.case_time_ms),
        };
        let outcome = judge.run(&req).await?;
        let compiled = outcome.compile.as_ref().is_none_or(|c| c.ok)
            && outcome
                .cases
                .iter()
                .all(|c| c.status != promptlab_judge::Status::CompileError);
        out.push(TemplateCheck { language, compiled });
    }
    Ok(out)
}

/// 발행 전 점검 하나: 모범 답안을 예제 + 히든으로 판정기에 돌려 통과 수를 남긴다. 점검이 없으면 `Idle`.
pub async fn check_once<J: JudgeClient>(pool: &PgPool, judge: &J) -> Result<Outcome, sqlx::Error> {
    let Some(step) = checks::claim_step(pool).await? else {
        return Ok(Outcome::Idle);
    };
    let id = step.id;
    let problem = &step.problem;
    let cases: Vec<&TestCase> = problem.examples.iter().chain(&problem.hidden).collect();
    let req = request(
        problem,
        problem.reference_language,
        &problem.reference_solution,
        &cases,
    );
    let outcome = match judge.run(&req).await {
        Ok(outcome) => check_templates(judge, problem)
            .await
            .map(|templates| (outcome, templates)),
        Err(e) => Err(e),
    };
    match outcome {
        Ok((outcome, templates)) => {
            let passed = case_passed(problem, &cases, &outcome);
            let result = CheckResult {
                templates,
                passed: u32::try_from(passed.iter().filter(|p| **p).count()).unwrap_or(u32::MAX),
                total: u32::try_from(passed.len()).unwrap_or(u32::MAX),
                compiled: outcome.compile.as_ref().is_none_or(|c| c.ok),
            };
            tracing::info!(%id, passed = result.passed, total = result.total, "발행 전 점검 끝");
            checks::record(pool, id, &result).await?;
        }
        Err(e) => {
            let reason = match e {
                JudgeCallError::Rejected(_) => {
                    "판정기가 모범 답안을 받지 않았다(서명·케이스 크기를 확인하세요)"
                }
                JudgeCallError::Unavailable(_) => {
                    "채점 서버 오류로 점검하지 못했다 — 다시 점검하세요"
                }
            };
            tracing::warn!(%id, error = %e, "발행 전 점검 실패");
            checks::fail(pool, id, reason).await?;
        }
    }
    Ok(Outcome::Checked(id))
}
