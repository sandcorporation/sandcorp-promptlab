//! 개인 연습 기록 요약(#18): 문항별 최고·최근·추이.

use promptlab_shared::api::SubmissionState;
use promptlab_shared::coding::Language;
use promptlab_shared::grading::GradeReport;
use promptlab_shared::practice::{PracticeAttempt, TREND_LEN, summarize};

fn attempt(n: usize, problem: &str, passed: Option<usize>) -> PracticeAttempt {
    PracticeAttempt {
        submission_id: format!("s{n:03}"),
        problem_id: problem.into(),
        submitted_at: 1_000 + n as i64,
        state: match passed {
            Some(p) => SubmissionState::Done {
                report: GradeReport {
                    problem_id: problem.into(),
                    language: Language::Python,
                    passed: p as u32,
                    total: 4,
                },
            },
            None => SubmissionState::Failed {
                reason: "채점 서버 오류".into(),
            },
        },
    }
}

/// 오래된 것부터 만든 목록을 새것부터로 뒤집어 넘긴다(서버가 넘기는 순서).
fn newest_first(mut v: Vec<PracticeAttempt>) -> Vec<PracticeAttempt> {
    v.reverse();
    v
}

#[test]
fn 문항별로_최고_최근_추이를_묶고_최근에_푼_문항이_먼저다() {
    let r = summarize(newest_first(vec![
        attempt(0, "a", Some(4)),
        attempt(1, "b", Some(1)),
        attempt(2, "a", None),
        attempt(3, "a", Some(2)),
        attempt(4, "b", Some(3)),
    ]));
    assert_eq!(r.attempts.first().unwrap().submission_id, "s004");
    assert_eq!(r.attempts.len(), 5);
    let ids: Vec<&str> = r.problems.iter().map(|p| p.problem_id.as_str()).collect();
    assert_eq!(ids, ["b", "a"]);
    let a = &r.problems[1];
    assert_eq!(a.attempts, 3, "실패도 낸 횟수에 든다");
    assert_eq!(a.trend, vec![1.0, 0.5], "실패는 추이에 없다");
    assert_eq!(a.best, Some(1.0));
    assert_eq!(a.latest, Some(0.5));
    assert_eq!(a.last_submitted_at, 1_003);
    let b = &r.problems[0];
    assert_eq!((b.best, b.latest), (Some(0.75), Some(0.75)));
}

#[test]
fn 추이는_최근_것만_남기고_채점_전이면_점수가_없다() {
    let mut v: Vec<_> = (0..TREND_LEN + 5)
        .map(|n| attempt(n, "a", Some(n % 5)))
        .collect();
    let mut pending = attempt(99, "c", None);
    pending.state = SubmissionState::Queued;
    v.push(pending);
    let r = summarize(newest_first(v));
    let a = r.problems.iter().find(|p| p.problem_id == "a").unwrap();
    assert_eq!(a.trend.len(), TREND_LEN);
    assert_eq!(a.attempts as usize, TREND_LEN + 5);
    assert_eq!(*a.trend.last().unwrap(), ((TREND_LEN + 4) % 5) as f64 / 4.0);
    let c = r.problems.iter().find(|p| p.problem_id == "c").unwrap();
    assert_eq!((c.best, c.latest, c.attempts), (None, None, 1));
    assert!(summarize(Vec::new()).problems.is_empty());
}
