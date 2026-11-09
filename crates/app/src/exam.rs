//! 응시 흐름의 부수 효과(HTTP·모델 파일 해시)를 돌리고 그 결과를 [`SessionEvent`]로 돌려준다.
//! 화면(`main.rs`)과 CLI(`bin/promptlab-app-cli.rs`)가 같은 함수로 같은 길을 간다.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use promptlab_inference::verify_model_file;
use promptlab_shared::integrity::ClientIntegrity;
use promptlab_shared::log::{LogEvent, code_sha256};

use crate::client::{ApiClient, ClientError};
use crate::proclog::{SharedLog, flush};
use crate::session::{ExamItemRef, Phase, Session, SessionEvent};

/// 서버 마감(유닉스 초)을 앱 시계로 옮긴다. 서버가 함께 준 `now`로 재므로 PC 시계가 틀려도 맞다.
pub fn local_deadline(server_deadline: i64, server_now: i64, received_at: Instant) -> Instant {
    let left = u64::try_from(server_deadline.saturating_sub(server_now)).unwrap_or(0);
    received_at + Duration::from_secs(left)
}

/// 문항 시작 때 서버에 신고하는 값: 이 앱의 판과 올릴 때 잰 모델 파일 sha256(#12 허용 목록).
pub fn client_integrity(model_sha256: &str) -> ClientIntegrity {
    ClientIntegrity {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        model_sha256: model_sha256.to_owned(),
    }
}

/// 고정 모델 파일의 sha256을 잰다(문항 시작 때 서버 허용 목록과 대조, #12). 앱의 AI 도우미(#98)가 쓰는 파일이다.
pub async fn model_sha256(model_path: PathBuf) -> Result<String, String> {
    tokio::task::spawn_blocking(move || verify_model_file(&model_path).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// 배정된 시험 문항의 점검: 모델 파일 해시를 재고(시간이 들므로 마감 전에), 서버에 문항 시작을 알려 문제·허용
/// 언어·마감을 받는다. 성공하면 `ExamReady`, 실패하면 `ChecksFailed`.
pub async fn check_item(api: &ApiClient, model_path: PathBuf, item: ExamItemRef) -> SessionEvent {
    let sha = match model_sha256(model_path).await {
        Ok(sha) => sha,
        Err(e) => {
            return SessionEvent::ChecksFailed {
                reason: format!("로컬 모델 파일을 확인하지 못했다: {e}"),
            };
        }
    };
    let integrity = client_integrity(&sha);
    match api
        .start_item(&item.assignment_id, item.position, &integrity)
        .await
    {
        Ok(started) => SessionEvent::ExamReady {
            deadline: local_deadline(started.deadline, started.now, Instant::now()),
            problem: started.problem,
            allowed_languages: started.allowed_languages,
            item,
        },
        Err(e) => SessionEvent::ChecksFailed {
            reason: format!("문항을 시작하지 못했다: {e}"),
        },
    }
}

/// 연습 점검: 서버에서 연습 문제(공개 보기)를 받는다. 성공하면 `ChecksPassed`, 실패하면 `ChecksFailed`.
pub async fn check(api: &ApiClient, problem_id: &str) -> SessionEvent {
    match api.problem(problem_id).await {
        Ok(problem) => SessionEvent::ChecksPassed { problem },
        Err(e) => SessionEvent::ChecksFailed {
            reason: format!("문제를 받지 못했다: {e}"),
        },
    }
}

/// 시험 제출이 `409`(서버가 받은 로그가 모자람)일 때 로그를 다시 올리고 내는 횟수.
pub const SUBMIT_RETRIES: u32 = 3;

/// 제출 중인 세션의 제출을 서버로 보낸다. 제출 중이 아니면 `None`.
///
/// 시험 문항은 과정 로그에 [`LogEvent::Submitted`](언어·코드 sha256)를 남긴 뒤 쌓인 로그를 먼저 다 올리고, 언어·코드와
/// 이 문항에 쓴 사건 수를 보낸다(서버가 로그의 마지막 `Submitted`와 맞춰 본다). 서버가 `409`(로그가 덜 왔다)면 로그를
/// 다시 올리고 [`SUBMIT_RETRIES`]번까지 다시 낸다. 다시 낼 때는 사건을 또 남기지 않는다.
pub async fn submit(
    api: &ApiClient,
    session: &Session,
    log: Option<&SharedLog>,
) -> Option<SessionEvent> {
    let Phase::Submitting { submission } = session.phase() else {
        return None;
    };
    let result = match session.exam_item() {
        Some(item) => {
            if let Some(log) = log {
                let event = LogEvent::Submitted {
                    language: submission.language,
                    code_sha256: code_sha256(&submission.code),
                };
                if let Err(e) = log.lock().expect("과정 로그 잠금").record_now(event) {
                    return Some(SessionEvent::SubmitRejected {
                        reason: format!("과정 로그에 제출을 남기지 못했다: {e}"),
                    });
                }
            }
            let mut attempt = 0;
            loop {
                let log_seq = match log {
                    Some(log) => {
                        // 올리지 못해도 낸다 — 서버가 409로 알려 주면 다시 올린다.
                        let _ = flush(log, api).await;
                        log.lock().expect("과정 로그 잠금").next_seq()
                    }
                    None => 0,
                };
                let result = api
                    .submit_item(
                        &item.assignment_id,
                        item.position,
                        submission.language,
                        &submission.code,
                        log_seq,
                    )
                    .await;
                match result {
                    Err(ClientError::Rejected { status: 409, .. })
                        if log.is_some() && attempt < SUBMIT_RETRIES =>
                    {
                        attempt += 1;
                        tokio::time::sleep(Duration::from_millis(500 * u64::from(attempt))).await;
                    }
                    other => break other,
                }
            }
        }
        None => {
            let problem_id = session.problem()?.id.clone();
            api.submit(&problem_id, submission).await
        }
    };
    Some(match result {
        Ok(view) => SessionEvent::SubmitAccepted { view },
        Err(e) => SessionEvent::SubmitRejected {
            reason: format!("제출하지 못했다: {e}"),
        },
    })
}

/// 채점 대기 중이면 상태를 한 번 받아 온다. 대기 중이 아니거나 받아 오지 못했으면 `None`
/// (네트워크가 잠깐 끊긴 것일 수 있다 — 부르는 쪽이 다시 부른다).
pub async fn poll(api: &ApiClient, session: &Session) -> Option<SessionEvent> {
    let (Phase::Grading { submission_id } | Phase::Graded { submission_id, .. }) = session.phase()
    else {
        return None;
    };
    api.submission(submission_id)
        .await
        .ok()
        .map(|view| SessionEvent::StatusPolled { view })
}
