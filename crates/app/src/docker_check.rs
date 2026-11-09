//! 응시 전 Docker 점검(#97, spec #93 5절): ① Docker 설치·데몬 확인 → ② 그 시험의 허용 언어 러너 이미지(고정
//! digest)가 없으면 받기(진행 표시) → ③ 언어마다 빈 하네스 한 번 실행. 하나라도 안 되면 시험 응시를 잠근다.
//!
//! 상태 기계는 순수하다 — [`DockerCheck::next`]가 할 일을 주고, 부르는 쪽(화면·CLI)이 [`LocalDocker`]로 하고 결과를
//! [`DockerCheck::apply`]로 넣는다([`drive`]가 그 고리). 시험 `tests/docker_check.rs`.

use promptlab_shared::coding::Language;

use crate::localrun::{DockerError, LocalDocker, PullProgress};

/// 언어 하나의 준비 상태.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LangState {
    /// 이미지가 있는지 아직 모른다.
    Unknown,
    /// 이미지를 받는 중.
    Pulling(PullProgress),
    /// 이미지가 있다 — 빈 하네스를 아직 안 돌렸다.
    ImageReady,
    /// 빈 하네스까지 돌았다.
    Ready,
}

/// 점검 전체의 단계.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// 아직 시작 전.
    Idle,
    /// docker 데몬 확인 중.
    Probing,
    /// 이미지·빈 하네스 단계.
    Languages,
    /// 모두 준비됨(엔진 판).
    Ready { version: String },
    /// 막혔다 — 까닭.
    Blocked(DockerError),
}

/// 다음에 할 일.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Probe,
    /// 이미지가 있는지 본다.
    Inspect(Language),
    Pull(Language),
    Trial(Language),
    /// 끝(준비됨 또는 막힘).
    Done,
}

/// 할 일의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckEvent {
    Probed(Result<String, DockerError>),
    Inspected(Language, Result<bool, DockerError>),
    PullProgress(Language, PullProgress),
    Pulled(Language, Result<(), DockerError>),
    Tried(Language, Result<(), DockerError>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerCheck {
    stage: Stage,
    version: String,
    languages: Vec<(Language, LangState)>,
}

impl DockerCheck {
    /// 이 언어들(시험의 허용 언어, `Language::ALL` 순서로 맞춘다)을 점검한다.
    pub fn new(languages: &[Language]) -> Self {
        let languages = Language::ALL
            .into_iter()
            .filter(|l| languages.contains(l))
            .map(|l| (l, LangState::Unknown))
            .collect();
        Self {
            stage: Stage::Idle,
            version: String::new(),
            languages,
        }
    }

    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    pub fn languages(&self) -> &[(Language, LangState)] {
        &self.languages
    }

    /// 모두 준비됐다(응시를 시작해도 된다).
    pub fn ready(&self) -> bool {
        matches!(self.stage, Stage::Ready { .. })
    }

    /// 막힌 까닭.
    pub fn blocker(&self) -> Option<&DockerError> {
        match &self.stage {
            Stage::Blocked(e) => Some(e),
            _ => None,
        }
    }

    /// Docker Desktop 설치 안내(링크 + 라이선스 한 줄)를 보여야 하나.
    pub fn needs_install_guide(&self) -> bool {
        matches!(
            self.blocker(),
            Some(DockerError::NotInstalled | DockerError::DaemonDown)
        )
    }

    /// 다시 확인(막힌 뒤 「다시 확인」). 이미 준비된 언어는 그대로 둔다.
    pub fn restart(&mut self) {
        self.stage = Stage::Idle;
    }

    /// 다음 할 일.
    pub fn next_action(&mut self) -> Action {
        match &self.stage {
            Stage::Idle => {
                self.stage = Stage::Probing;
                Action::Probe
            }
            Stage::Probing => Action::Probe,
            Stage::Ready { .. } | Stage::Blocked(_) => Action::Done,
            Stage::Languages => {
                for (l, s) in &self.languages {
                    match s {
                        LangState::Unknown => return Action::Inspect(*l),
                        LangState::Pulling(_) => return Action::Pull(*l),
                        LangState::ImageReady => return Action::Trial(*l),
                        LangState::Ready => {}
                    }
                }
                self.stage = Stage::Ready {
                    version: self.version.clone(),
                };
                Action::Done
            }
        }
    }

    fn set(&mut self, language: Language, state: LangState) {
        if let Some((_, s)) = self.languages.iter_mut().find(|(l, _)| *l == language) {
            *s = state;
        }
    }

    /// 할 일의 결과를 넣는다.
    pub fn apply(&mut self, event: CheckEvent) {
        match event {
            CheckEvent::Probed(Ok(version)) => {
                self.version = version;
                self.stage = Stage::Languages;
            }
            CheckEvent::Inspected(l, Ok(true)) => self.set(l, LangState::ImageReady),
            CheckEvent::Inspected(l, Ok(false)) => {
                self.set(l, LangState::Pulling(PullProgress::new()))
            }
            CheckEvent::PullProgress(l, p) => self.set(l, LangState::Pulling(p)),
            CheckEvent::Pulled(l, Ok(())) => self.set(l, LangState::ImageReady),
            CheckEvent::Tried(l, Ok(())) => self.set(l, LangState::Ready),
            CheckEvent::Probed(Err(e))
            | CheckEvent::Inspected(_, Err(e))
            | CheckEvent::Pulled(_, Err(e))
            | CheckEvent::Tried(_, Err(e)) => {
                // 이미지 받기·실행이 실패한 언어는 처음부터 다시 본다.
                for (_, s) in self.languages.iter_mut() {
                    if !matches!(s, LangState::Ready | LangState::ImageReady) {
                        *s = LangState::Unknown;
                    }
                }
                self.stage = Stage::Blocked(e);
            }
        }
    }

    /// 한 줄 상태(점검 행의 설명).
    pub fn status_text(&self) -> String {
        match &self.stage {
            Stage::Idle => "확인 전".to_owned(),
            Stage::Probing => "Docker 확인 중…".to_owned(),
            Stage::Languages => {
                let ready = self
                    .languages
                    .iter()
                    .filter(|(_, s)| *s == LangState::Ready)
                    .count();
                let doing = self.languages.iter().find_map(|(l, s)| match s {
                    LangState::Pulling(p) => {
                        Some(format!("{} 이미지 받는 중({})", l.display_name(), p.text()))
                    }
                    LangState::ImageReady => Some(format!("{} 빈 틀 실행 중", l.display_name())),
                    LangState::Unknown => Some(format!("{} 이미지 확인 중", l.display_name())),
                    LangState::Ready => None,
                });
                format!(
                    "{} · 언어 {ready}/{} 준비",
                    doing.unwrap_or_default(),
                    self.languages.len()
                )
            }
            Stage::Ready { version } => {
                format!("Docker {version} · 언어 {}개 준비됨", self.languages.len())
            }
            Stage::Blocked(e) => e.to_string(),
        }
    }
}

/// 점검을 끝(준비·막힘)까지 돈다. 단계마다 `on_change`로 지금 상태를 알린다(화면 갱신·CLI 출력).
pub async fn drive(
    docker: Option<&LocalDocker>,
    check: &mut DockerCheck,
    mut on_change: impl FnMut(&DockerCheck),
) {
    loop {
        let action = check.next_action();
        on_change(check);
        let event = match (action, docker) {
            (Action::Done, _) => return,
            (_, None) => CheckEvent::Probed(Err(DockerError::NotInstalled)),
            (Action::Probe, Some(d)) => CheckEvent::Probed(d.probe().await),
            (Action::Inspect(l), Some(d)) => CheckEvent::Inspected(l, d.has_image(l).await),
            (Action::Pull(l), Some(d)) => {
                let result = d
                    .pull(l, |p| {
                        check.apply(CheckEvent::PullProgress(l, p.clone()));
                        on_change(check);
                    })
                    .await;
                CheckEvent::Pulled(l, result)
            }
            (Action::Trial(l), Some(d)) => CheckEvent::Tried(l, d.trial(l).await),
        };
        check.apply(event);
    }
}
