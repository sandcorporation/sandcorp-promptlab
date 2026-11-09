//! 응시 세션 상태 기계. UI·웹캠·네트워크와 떨어진 순수 전이다 — 화면(bin)과 CLI가 같은 것을 쓴다.
//!
//! 흐름(spec #1): 시작 전 → 점검 → 동의 → 응시 중 → 제출 중 → 채점 대기 → 채점 끝 / 채점 실패 / 응시 완료.
//! 응시 중에는 편집기(Monaco, [`crate::editor`])에서 쓴 코드(언어 + 본문)를 내고 서버가 판정기로 히든 테스트를
//! 돌려 통과 수를 매긴다(spec #93). 배정된 시험 문항([`ExamItemRef`])을 응시하면 점검 때 서버가 문항 시작 시각을
//! 남기고 마감·허용 언어를 준다([`SessionEvent::ExamReady`]). 마감이 지나면 화면은 바로 제출하고, 결과를 공개하지
//! 않는 시험(채용 등)은 채점 대신 [`Phase::Completed`](「응시 완료」)로 끝난다.
//! 부수 효과(문제 받기·모델 올리기·추론·HTTP)는 부르는 쪽이 하고, 그 결과를 사건으로 넣는다.
//! 세션은 [`PublicCodingProblem`]만 쥔다 — 숨은 입력은 앱에 오지 않는다.
//! 사건 기록·오프라인 재전송·웹캠 분석기 트레잇은 #9·#12·#13에서 더한다.

use std::time::Instant;

use promptlab_shared::api::{SubmissionState, SubmissionView};
use promptlab_shared::coding::{Language, PublicCodingProblem};
use promptlab_shared::evaluation::ItemScore;
use promptlab_shared::grading::GradeReport;

/// 서버로 보낼 제출: 언어와 코드. 시험 응시는 그 전에 과정 로그에 [`Submitted`](promptlab_shared::log::LogEvent)
/// 사건(언어·코드 sha256)을 남긴다 — 서버가 본문과 맞춰 본다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub language: Language,
    pub code: String,
    pub elapsed_secs: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// 앱을 열고 응시를 시작하기 전.
    NotStarted,
    /// 문제 받기·로컬 모델 올리기(이후 PC 사양·웹캠 점검).
    Checking,
    /// 안내를 보고 동의를 기다린다.
    AwaitingConsent,
    /// 응시 중: 편집기에서 코드를 쓰고 로컬 Docker로 실행·예제 테스트한다.
    InProgress,
    /// 이 제출을 서버로 보내는 중.
    Submitting { submission: Submission },
    /// 서버가 받았고 채점을 기다린다.
    Grading { submission_id: String },
    /// 채점이 끝났다.
    /// `score`는 최종 점수·계산식·대화 평가 항목·기준선 대비(#72·#73) — 채점이 끝났고 점수를 볼 수 있을 때만 있다.
    Graded {
        submission_id: String,
        report: GradeReport,
        score: Option<ItemScore>,
    },
    /// 서버가 채점하지 못했다.
    GradingFailed { reason: String },
    /// 응시 완료. 결과는 고객사만 본다(채용 시험 등 — 서버가 점수를 보내지 않는다).
    Completed,
}

/// 배정된 시험의 문항 하나(`POST /assignments/{assignment_id}/items/{position}/…`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExamItemRef {
    pub assignment_id: String,
    pub position: u32,
}

impl Phase {
    pub fn name(&self) -> &'static str {
        match self {
            Phase::NotStarted => "시작 전",
            Phase::Checking => "점검",
            Phase::AwaitingConsent => "동의 대기",
            Phase::InProgress => "응시 중",
            Phase::Submitting { .. } => "제출 중",
            Phase::Grading { .. } => "채점 대기",
            Phase::Graded { .. } => "채점 끝",
            Phase::GradingFailed { .. } => "채점 실패",
            Phase::Completed => "응시 완료",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    Start,
    /// 점검을 마쳤다(서버에서 공개 문제를 받았다 — 개인 연습, 언어 제한 없음).
    ChecksPassed {
        problem: PublicCodingProblem,
    },
    /// 배정된 시험 문항의 점검을 마쳤다(서버가 문항을 시작해 문제·허용 언어·마감을 줬다).
    ExamReady {
        problem: PublicCodingProblem,
        item: ExamItemRef,
        /// 이 시험이 허용하는 언어(`ItemStarted.allowed_languages`).
        allowed_languages: Vec<Language>,
        /// 서버 마감을 앱 시계로 옮긴 것.
        deadline: Instant,
    },
    /// 점검에 실패했다. 시작 전으로 돌아가 다시 시작할 수 있다.
    ChecksFailed {
        reason: String,
    },
    ConsentGiven {
        at: Instant,
    },
    /// 이 코드를 제출한다. 보낼 것은 [`Phase::Submitting`]에 담긴다. 빈 코드면 `Err`.
    Submit {
        at: Instant,
        language: Language,
        code: String,
    },
    /// 서버가 제출을 받았다.
    SubmitAccepted {
        view: SubmissionView,
    },
    /// 서버로 보내지 못했다. 응시 중으로 돌아가 다시 낼 수 있다.
    SubmitRejected {
        reason: String,
    },
    /// 채점 상태를 다시 받아 왔다.
    StatusPolled {
        view: SubmissionView,
    },
}

impl SessionEvent {
    fn name(&self) -> &'static str {
        match self {
            SessionEvent::Start => "시작",
            SessionEvent::ChecksPassed { .. } => "점검 통과",
            SessionEvent::ExamReady { .. } => "시험 문항 준비",
            SessionEvent::ChecksFailed { .. } => "점검 실패",
            SessionEvent::ConsentGiven { .. } => "동의",
            SessionEvent::Submit { .. } => "제출",
            SessionEvent::SubmitAccepted { .. } => "제출 접수",
            SessionEvent::SubmitRejected { .. } => "제출 거부",
            SessionEvent::StatusPolled { .. } => "채점 상태",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    #[error("{phase} 상태에서는 {event} 를 받을 수 없다")]
    InvalidTransition {
        phase: &'static str,
        event: &'static str,
    },
    #[error("빈 코드는 제출할 수 없다")]
    EmptyCode,
    #[error("이 시험에서는 {}을(를) 낼 수 없다", .0.display_name())]
    LanguageNotAllowed(Language),
    #[error("제한 시간이 끝났다 — 지금 코드를 제출한다")]
    TimeUp,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    phase: Phase,
    problem: Option<PublicCodingProblem>,
    /// 시험의 허용 언어(개인 연습은 비어 있다 = 4개 전부).
    allowed: Vec<Language>,
    started_at: Option<Instant>,
    /// 시험 문항 응시면 그 문항과 마감.
    exam: Option<(ExamItemRef, Instant)>,
    /// 응시자에게 보일 마지막 알림(점검 실패·제출 실패 등).
    notice: Option<String>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Self {
            phase: Phase::NotStarted,
            problem: None,
            allowed: Vec::new(),
            started_at: None,
            exam: None,
            notice: None,
        }
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn problem(&self) -> Option<&PublicCodingProblem> {
        self.problem.as_ref()
    }

    /// 고를 수 있는 언어(시험은 허용 언어, 개인 연습은 4개 전부).
    pub fn allowed_languages(&self) -> Vec<Language> {
        if self.allowed.is_empty() {
            Language::ALL.to_vec()
        } else {
            self.allowed.clone()
        }
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// 시험 문항 응시면 그 문항.
    pub fn exam_item(&self) -> Option<&ExamItemRef> {
        self.exam.as_ref().map(|(item, _)| item)
    }

    /// 시험 문항의 남은 시간(초, 올림). 시험 응시가 아니면 `None`.
    pub fn remaining_secs(&self, now: Instant) -> Option<u64> {
        let (_, deadline) = self.exam.as_ref()?;
        let left = deadline.saturating_duration_since(now);
        Some(left.as_secs() + u64::from(left.subsec_nanos() > 0))
    }

    /// 시험 문항의 제한 시간이 끝났는가.
    pub fn time_up(&self, now: Instant) -> bool {
        self.exam
            .as_ref()
            .is_some_and(|(_, deadline)| now >= *deadline)
    }

    /// 응시 시작(동의)부터 `now`까지의 초.
    pub fn elapsed_secs(&self, now: Instant) -> u32 {
        self.started_at.map_or(0, |t| {
            u32::try_from(now.saturating_duration_since(t).as_secs()).unwrap_or(u32::MAX)
        })
    }

    /// 사건 하나를 넣는다. 받을 수 없는 사건이면 상태는 그대로이고 `Err`.
    pub fn apply(&mut self, event: SessionEvent) -> Result<(), SessionError> {
        let invalid = |phase: &Phase, event: &SessionEvent| SessionError::InvalidTransition {
            phase: phase.name(),
            event: event.name(),
        };
        match (&self.phase, event) {
            (Phase::NotStarted, SessionEvent::Start) => {
                self.notice = None;
                self.phase = Phase::Checking;
            }
            (Phase::Checking, SessionEvent::ChecksPassed { problem }) => {
                self.problem = Some(problem);
                self.phase = Phase::AwaitingConsent;
            }
            (
                Phase::Checking,
                SessionEvent::ExamReady {
                    problem,
                    item,
                    allowed_languages,
                    deadline,
                },
            ) => {
                self.problem = Some(problem);
                self.allowed = allowed_languages;
                self.exam = Some((item, deadline));
                self.phase = Phase::AwaitingConsent;
            }
            (Phase::Checking, SessionEvent::ChecksFailed { reason }) => {
                self.notice = Some(reason);
                self.phase = Phase::NotStarted;
            }
            (Phase::AwaitingConsent, SessionEvent::ConsentGiven { at }) => {
                self.started_at = Some(at);
                self.phase = Phase::InProgress;
            }
            (Phase::InProgress, SessionEvent::Submit { at, language, code }) => {
                if code.trim().is_empty() {
                    return Err(SessionError::EmptyCode);
                }
                if !self.allowed.is_empty() && !self.allowed.contains(&language) {
                    return Err(SessionError::LanguageNotAllowed(language));
                }
                self.notice = None;
                self.phase = Phase::Submitting {
                    submission: Submission {
                        language,
                        code,
                        elapsed_secs: self.elapsed_secs(at),
                    },
                };
            }
            (Phase::Submitting { .. }, SessionEvent::SubmitAccepted { view }) => {
                self.phase = Phase::Grading {
                    submission_id: view.id.clone(),
                };
                self.settle(view);
            }
            (Phase::Submitting { .. }, SessionEvent::SubmitRejected { reason }) => {
                self.notice = Some(reason);
                self.phase = Phase::InProgress;
            }
            (Phase::Grading { .. }, SessionEvent::StatusPolled { view }) => {
                self.settle(view);
            }
            // 채점이 끝난 뒤에도 점수가 늦게 바뀔 수 있다(대화 평가, #99) — 점수를 갱신한다.
            (Phase::Graded { .. }, SessionEvent::StatusPolled { view }) => {
                self.settle(view);
            }
            (phase, event) => return Err(invalid(phase, &event)),
        }
        Ok(())
    }

    /// 채점 대기 중에 받은 상태를 반영한다(끝났으면 결과로 넘어간다).
    fn settle(&mut self, view: SubmissionView) {
        match view.state {
            SubmissionState::Queued | SubmissionState::Grading => {}
            SubmissionState::Done { report } => {
                self.phase = Phase::Graded {
                    submission_id: view.id,
                    report,
                    score: view.score,
                }
            }
            SubmissionState::Failed { reason } => self.phase = Phase::GradingFailed { reason },
            SubmissionState::Completed => self.phase = Phase::Completed,
        }
    }
}
