//! 개인 연습(#18): 조직·초대 없이 연습 문제를 풀고 내 기록·점수 추이를 본다.
//!
//! - 연습 문제는 문제은행의 일부이고 시험 문항과 겹치지 않는다(목록은 `promptlab_store::catalog::PRACTICE`).
//! - 연습은 감독이 아니다: 웹캠·동의·과정 로그·앱 판 허용 목록을 걸지 않는다. 점수는 서버가 같은 판정기로 매긴다.
//! - 연습 기록은 낸 사람(`sub`)만 본다(`GET /me/practice`). 고객사 화면·집계에는 나오지 않는다.
//! - 채점 큐에서 연습은 시험 뒤에 꺼내고, 사람마다 채점을 기다리는 연습은 [`MAX_PENDING`]개까지다.

use serde::{Deserialize, Serialize};

use crate::api::SubmissionState;

/// 사람마다 채점을 기다리는(대기·채점 중) 연습 제출 상한. 넘으면 `POST /submissions`가 `429`.
pub const MAX_PENDING: usize = 2;

/// 앱이 처음 고르는 연습 문제 — 운영 문제 디렉터리(비공개 데이터 저장소)의 연습 문항이다. 공개 픽스처로 뜬 개발
/// 스택에서는 `PROMPTLAB_PROBLEM_ID`·`--problem`으로 픽스처 연습 문항을 고른다(#138).
pub const DEFAULT_PROBLEM_ID: &str = "two-sum";

/// 문항별 추이에 싣는 최근 점수 개수.
pub const TREND_LEN: usize = 20;

/// `GET /me/practice` 응답: 내 연습 기록.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PracticeRecords {
    /// 문항별 요약(최근에 푼 문항 먼저).
    pub problems: Vec<ProblemRecord>,
    /// 제출들(새것부터).
    pub attempts: Vec<PracticeAttempt>,
}

/// 연습 제출 하나.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PracticeAttempt {
    pub submission_id: String,
    pub problem_id: String,
    /// 낸 시각(유닉스 초).
    pub submitted_at: i64,
    pub state: SubmissionState,
}

impl PracticeAttempt {
    /// 채점이 끝났으면 숨은 검사 통과율(0.0~1.0).
    pub fn score(&self) -> Option<f64> {
        match &self.state {
            SubmissionState::Done { report } => Some(report.pass_rate()),
            _ => None,
        }
    }
}

/// 한 문항의 내 연습 요약.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProblemRecord {
    pub problem_id: String,
    /// 낸 횟수(채점 전·실패 포함).
    pub attempts: u32,
    /// 채점이 끝난 것 중 최고 점수.
    pub best: Option<f64>,
    /// 가장 최근에 채점이 끝난 점수.
    pub latest: Option<f64>,
    /// 채점이 끝난 점수들, 오래된 것부터(최근 [`TREND_LEN`]개).
    pub trend: Vec<f64>,
    /// 마지막으로 낸 시각(유닉스 초).
    pub last_submitted_at: i64,
}

/// 새것부터 놓인 제출들을 문항별 요약과 (그대로) 새것부터의 목록으로 묶는다.
/// 순서는 부르는 쪽(DB의 낸 시각, 마이크로초)이 정한다 — 초 단위 `submitted_at`으로는 같은 초에 낸 것을 가르지 못한다.
pub fn summarize(mut attempts: Vec<PracticeAttempt>) -> PracticeRecords {
    // 오래된 것부터 훑어 추이를 쌓는다.
    attempts.reverse();
    let mut problems: Vec<ProblemRecord> = Vec::new();
    for attempt in &attempts {
        let record = match problems
            .iter_mut()
            .position(|p| p.problem_id == attempt.problem_id)
        {
            Some(i) => &mut problems[i],
            None => {
                problems.push(ProblemRecord {
                    problem_id: attempt.problem_id.clone(),
                    attempts: 0,
                    best: None,
                    latest: None,
                    trend: Vec::new(),
                    last_submitted_at: attempt.submitted_at,
                });
                problems.last_mut().expect("방금 넣었다")
            }
        };
        record.attempts += 1;
        record.last_submitted_at = attempt.submitted_at;
        if let Some(score) = attempt.score() {
            record.best = Some(record.best.map_or(score, |b| b.max(score)));
            record.latest = Some(score);
            record.trend.push(score);
        }
    }
    for record in &mut problems {
        let over = record.trend.len().saturating_sub(TREND_LEN);
        record.trend.drain(..over);
    }
    problems.sort_by(|a, b| {
        b.last_submitted_at
            .cmp(&a.last_submitted_at)
            .then_with(|| a.problem_id.cmp(&b.problem_id))
    });
    attempts.reverse();
    PracticeRecords { problems, attempts }
}
