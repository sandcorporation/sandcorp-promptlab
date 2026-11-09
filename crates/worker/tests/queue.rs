//! 채점 워커 큐(spec #93): 가짜 판정기([`FakeJudge`])로 제출·점검을 돌린다. 시험 DB는 `#[sqlx::test]`가 만든다.

use std::collections::HashMap;

use promptlab_shared::api::SubmissionState;
use promptlab_shared::coding::Language;
use promptlab_store::checks::{self, Scope};
use promptlab_store::smoke::PROBLEM_ID;
use promptlab_store::{PgPool, catalog, submissions};
use promptlab_worker::fake::{FakeJudge, Mode};
use promptlab_worker::{
    Outcome, REASON_JUDGE_DOWN, REASON_JUDGE_REJECTED, REASON_UNKNOWN_PROBLEM, run_once,
};
use serde_json::{Value, json};
use uuid::Uuid;

/// 내장 시험 문항의 히든 케이스를 아는 가짜 풀이: 코드 `correct`는 모두 맞히고, `wrong-first`는 첫 히든 케이스만
/// 틀린다. 모범 답안 점검용 `a + b`·`a - b`도 안다.
fn judge() -> FakeJudge {
    let problem = catalog::find(PROBLEM_ID).unwrap();
    let answers: HashMap<String, Value> = problem
        .hidden
        .iter()
        .map(|c| (Value::Array(c.args.clone()).to_string(), c.expected.clone()))
        .collect();
    let first = Value::Array(problem.hidden[0].args.clone()).to_string();
    FakeJudge::new(move |code, args| {
        let key = Value::Array(args.to_vec()).to_string();
        let int = |i: usize| args[i].as_i64().unwrap_or(0);
        match code {
            "correct" => answers.get(&key).cloned(),
            "wrong-first" if key == first => Some(json!(-12345)),
            "wrong-first" => answers.get(&key).cloned(),
            c if c.contains("a + b") => Some(json!(int(0) + int(1))),
            c if c.contains("a - b") => Some(json!(int(0) - int(1))),
            _ => None,
        }
    })
}

async fn submit(
    pool: &PgPool,
    problem_id: &str,
    language: Language,
    code: &str,
    practice: bool,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO submissions (problem_id, language, code, elapsed_secs, candidate_sub, practice)
         VALUES ($1, $2, $3, 10, 'm_test', $4) RETURNING id",
    )
    .bind(problem_id)
    .bind(language.as_str())
    .bind(code)
    .bind(practice)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn state(pool: &PgPool, id: Uuid) -> SubmissionState {
    submissions::get(pool, id).await.unwrap().unwrap().0.state
}

async fn failure_detail(pool: &PgPool, id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT failure_detail FROM submissions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn hidden_total() -> u32 {
    catalog::find(PROBLEM_ID).unwrap().hidden.len() as u32
}

fn done(s: SubmissionState) -> (u32, u32, Language) {
    match s {
        SubmissionState::Done { report } => (report.passed, report.total, report.language),
        other => panic!("채점이 끝나지 않았다: {other:?}"),
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 정답이면_모든_히든_케이스를_통과한다(pool: PgPool) {
    let judge = judge();
    let id = submit(&pool, PROBLEM_ID, Language::Python, "correct", true).await;
    assert_eq!(run_once(&pool, &judge).await.unwrap(), Outcome::Graded(id));
    assert_eq!(
        done(state(&pool, id).await),
        (hidden_total(), hidden_total(), Language::Python)
    );
    assert_eq!(run_once(&pool, &judge).await.unwrap(), Outcome::Idle);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 일부_틀리면_통과_수만_준다(pool: PgPool) {
    let judge = judge();
    let id = submit(&pool, PROBLEM_ID, Language::Javascript, "wrong-first", true).await;
    run_once(&pool, &judge).await.unwrap();
    assert_eq!(
        done(state(&pool, id).await),
        (hidden_total() - 1, hidden_total(), Language::Javascript)
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 컴파일_오류는_채점_실패가_아니라_0점이다(pool: PgPool) {
    let judge = judge();
    judge.set_mode(Mode::CompileError);
    let id = submit(&pool, PROBLEM_ID, Language::Java, "correct", true).await;
    assert_eq!(run_once(&pool, &judge).await.unwrap(), Outcome::Graded(id));
    assert_eq!(
        done(state(&pool, id).await),
        (0, hidden_total(), Language::Java)
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 벽시계로_죽이면_돌지_못한_케이스는_틀린다(pool: PgPool) {
    let judge = judge();
    judge.set_mode(Mode::WallKill(2));
    let id = submit(&pool, PROBLEM_ID, Language::Cpp, "correct", true).await;
    run_once(&pool, &judge).await.unwrap();
    assert_eq!(
        done(state(&pool, id).await),
        (2, hidden_total(), Language::Cpp)
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 판정기가_거절하면_짧은_이유로_실패하고_상세는_db에만(pool: PgPool) {
    let judge = judge();
    judge.set_mode(Mode::Rejected);
    let id = submit(&pool, PROBLEM_ID, Language::Python, "correct", true).await;
    assert_eq!(run_once(&pool, &judge).await.unwrap(), Outcome::Failed(id));
    assert_eq!(
        state(&pool, id).await,
        SubmissionState::Failed {
            reason: REASON_JUDGE_REJECTED.into()
        }
    );
    assert_eq!(
        failure_detail(&pool, id).await.as_deref(),
        Some("케이스가 너무 많다")
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 판정기가_잠시_안_되면_되돌리고_세_번_뒤_실패한다(pool: PgPool) {
    let judge = judge();
    judge.set_mode(Mode::Unavailable);
    let id = submit(&pool, PROBLEM_ID, Language::Python, "correct", true).await;
    assert_eq!(
        run_once(&pool, &judge).await.unwrap(),
        Outcome::Requeued(id)
    );
    assert_eq!(state(&pool, id).await, SubmissionState::Queued);
    assert_eq!(
        run_once(&pool, &judge).await.unwrap(),
        Outcome::Requeued(id)
    );
    assert_eq!(run_once(&pool, &judge).await.unwrap(), Outcome::Failed(id));
    assert_eq!(
        state(&pool, id).await,
        SubmissionState::Failed {
            reason: REASON_JUDGE_DOWN.into()
        }
    );
    assert!(failure_detail(&pool, id).await.unwrap().contains("503"));
    assert_eq!(judge.requests().len(), 3);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 판정기_요청에는_히든_인자만_있고_기대값은_없다(pool: PgPool) {
    let judge = judge();
    submit(&pool, PROBLEM_ID, Language::Python, "correct", true).await;
    run_once(&pool, &judge).await.unwrap();
    let problem = catalog::find(PROBLEM_ID).unwrap();
    let requests = judge.requests();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    let args: Vec<Vec<Value>> = problem.hidden.iter().map(|c| c.args.clone()).collect();
    assert_eq!(req.cases, args);
    assert_eq!(req.code, "correct");
    assert_eq!(req.case_time_ms, Some(problem.limits.case_time_ms));
    let body = serde_json::to_string(req).unwrap();
    assert!(!body.contains("expected"), "{body}");
    assert!(!body.contains(problem.reference_solution.trim()));
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 없는_문제는_실패로_남긴다(pool: PgPool) {
    let judge = judge();
    let id = submit(&pool, "no-such", Language::Python, "correct", true).await;
    assert_eq!(run_once(&pool, &judge).await.unwrap(), Outcome::Failed(id));
    assert_eq!(
        state(&pool, id).await,
        SubmissionState::Failed {
            reason: REASON_UNKNOWN_PROBLEM.into()
        }
    );
    assert!(judge.requests().is_empty());
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 연습은_먼저_냈어도_시험_제출_뒤에_채점한다(pool: PgPool) {
    let judge = judge();
    let practice = submit(&pool, PROBLEM_ID, Language::Python, "correct", true).await;
    let exam = submit(&pool, PROBLEM_ID, Language::Python, "correct", false).await;
    assert_eq!(
        run_once(&pool, &judge).await.unwrap(),
        Outcome::Graded(exam)
    );
    assert_eq!(
        run_once(&pool, &judge).await.unwrap(),
        Outcome::Graded(practice)
    );
}

/// 운영자 문항 하나의 점검을 큐에 넣는다(모범 답안 `reference`).
async fn preview(pool: &PgPool, reference: &str) -> Uuid {
    sqlx::query("INSERT INTO users (sub) VALUES ('m_author') ON CONFLICT DO NOTHING")
        .execute(pool)
        .await
        .unwrap();
    let source = json!({
        "title": "두 수 더하기",
        "description": "`a + b`를 돌려준다.",
        "signature": {"name": "add", "params": [{"name": "a", "type": "int"}, {"name": "b", "type": "int"}], "returns": "int"},
        "examples": [{"args": [1, 2], "expected": 3}],
        "hidden": [
            {"args": [0, 0], "expected": 0},
            {"args": [-1, 1], "expected": 0},
            {"args": [5, 7], "expected": 12},
            {"args": [100, 1], "expected": 101},
            {"args": [-3, -4], "expected": -7}
        ],
        "reference_solution": reference
    });
    sqlx::query_scalar(
        "INSERT INTO problem_previews (org_id, requested_by, problem_id, source, total)
         VALUES (NULL, 'm_author', 'bank-add', $1, 6) RETURNING id",
    )
    .bind(source)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 점검은_모범_답안이_예제와_히든을_모두_통과해야_발행_가능이다(
    pool: PgPool,
) {
    let judge = judge();
    let good = preview(&pool, "def add(a, b):\n    return a + b\n").await;
    let bad = preview(&pool, "def add(a, b):\n    return a - b\n").await;
    assert_eq!(
        run_once(&pool, &judge).await.unwrap(),
        Outcome::Checked(good)
    );
    assert_eq!(
        run_once(&pool, &judge).await.unwrap(),
        Outcome::Checked(bad)
    );
    assert_eq!(run_once(&pool, &judge).await.unwrap(), Outcome::Idle);

    let good = checks::get(&pool, Scope::Operator, good)
        .await
        .unwrap()
        .unwrap();
    let result = good.result.clone().unwrap();
    assert_eq!((result.passed, result.total), (6, 6));
    assert!(good.publishable);

    let bad = checks::get(&pool, Scope::Operator, bad)
        .await
        .unwrap()
        .unwrap();
    assert!(!bad.publishable);
    assert!(bad.reason.unwrap().contains("/ 6"));

    // 점검은 모범 답안을 예제 + 히든 인자로 보낸다(기대값 없이).
    let req = &judge.requests()[0];
    assert_eq!(req.cases.len(), 6);
    assert!(req.code.contains("a + b"));
    assert!(!serde_json::to_string(req).unwrap().contains("expected"));
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 점검은_채점_큐가_비었을_때만_돈다(pool: PgPool) {
    let judge = judge();
    let check = preview(&pool, "def add(a, b):\n    return a + b\n").await;
    let sub = submit(&pool, PROBLEM_ID, Language::Python, "correct", true).await;
    assert_eq!(run_once(&pool, &judge).await.unwrap(), Outcome::Graded(sub));
    assert_eq!(
        run_once(&pool, &judge).await.unwrap(),
        Outcome::Checked(check)
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 판정기가_안_되면_점검은_짧은_이유로_실패한다(pool: PgPool) {
    let judge = judge();
    judge.set_mode(Mode::Unavailable);
    let check = preview(&pool, "def add(a, b):\n    return a + b\n").await;
    assert_eq!(
        run_once(&pool, &judge).await.unwrap(),
        Outcome::Checked(check)
    );
    let view = checks::get(&pool, Scope::Operator, check)
        .await
        .unwrap()
        .unwrap();
    assert!(!view.publishable);
    assert!(view.failure_reason.unwrap().contains("채점 서버 오류"));
}
