//! 응시 화면 v3(#97)의 편집기 밖 로직: 언어별 코드 초안, 과정 로그 코드 스냅숏 주기, 클립보드(앱 안 버퍼) 판정.
//! 화면(`ui/exam_screen.rs`)과 Monaco 다리(`ui/editor.js`)는 이것을 부르기만 한다 — 시험은 `tests/editor.rs`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use promptlab_shared::coding::{Language, PublicCodingProblem, solution_template};
use promptlab_shared::log::{LogEvent, MAX_SNAPSHOT_BYTES};

/// 코드가 바뀌었으면 이 간격마다 스냅숏을 남긴다(spec #93 6절).
pub const SNAPSHOT_EVERY: Duration = Duration::from_secs(5);

/// 언어를 고를 수 없다.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditorError {
    #[error("이 시험에서는 {}을(를) 쓸 수 없다", .0.display_name())]
    NotAllowed(Language),
}

/// 편집기 내용: 고른 언어와 언어마다의 초안. 언어를 바꿔도 앞 언어의 초안은 남는다(돌아오면 이어 쓴다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorState {
    language: Language,
    allowed: Vec<Language>,
    drafts: BTreeMap<Language, String>,
}

impl EditorState {
    /// 문제와 허용 언어로 편집기를 연다. 처음 언어는 허용 언어 중 Python(있으면) 아니면 첫 언어, 내용은 그 언어의 틀.
    /// 허용 언어가 비면 4개 언어 전부(개인 연습).
    pub fn new(problem: &PublicCodingProblem, allowed: &[Language]) -> Self {
        let allowed: Vec<Language> = if allowed.is_empty() {
            Language::ALL.to_vec()
        } else {
            Language::ALL
                .into_iter()
                .filter(|l| allowed.contains(l))
                .collect()
        };
        let language = if allowed.contains(&Language::Python) {
            Language::Python
        } else {
            allowed[0]
        };
        let drafts = allowed.iter().map(|&l| (l, template(problem, l))).collect();
        Self {
            language,
            allowed,
            drafts,
        }
    }

    pub fn language(&self) -> Language {
        self.language
    }

    /// 고를 수 있는 언어(`Language::ALL` 순서).
    pub fn allowed(&self) -> &[Language] {
        &self.allowed
    }

    /// 지금 언어의 코드.
    pub fn code(&self) -> &str {
        self.drafts.get(&self.language).map_or("", String::as_str)
    }

    /// 편집기가 알린 지금 언어의 코드.
    pub fn edit(&mut self, code: String) {
        self.drafts.insert(self.language, code);
    }

    /// 언어를 바꾼다. 바뀌었으면 과정 로그 사건([`LogEvent::LanguageChosen`]), 같은 언어면 `None`.
    pub fn choose(&mut self, language: Language) -> Result<Option<LogEvent>, EditorError> {
        if !self.allowed.contains(&language) {
            return Err(EditorError::NotAllowed(language));
        }
        if language == self.language {
            return Ok(None);
        }
        self.language = language;
        Ok(Some(LogEvent::LanguageChosen { language }))
    }

    /// 처음 연 언어의 사건(문항 시작 바로 뒤에 남긴다).
    pub fn opened(&self) -> LogEvent {
        LogEvent::LanguageChosen {
            language: self.language,
        }
    }
}

/// 문제가 준 그 언어의 틀(없으면 서명으로 만든다 — 같은 생성기).
pub fn template(problem: &PublicCodingProblem, language: Language) -> String {
    problem
        .templates
        .iter()
        .find(|t| t.language == language)
        .map_or_else(
            || solution_template(language, &problem.signature),
            |t| t.code.clone(),
        )
}

/// 코드 스냅숏 주기: 바뀌었으면 [`SNAPSHOT_EVERY`]마다 전체 텍스트 하나([`MAX_SNAPSHOT_BYTES`]에서 자른다).
#[derive(Debug, Clone, Default)]
pub struct SnapshotClock {
    last: Option<String>,
    last_at: Option<Instant>,
}

impl SnapshotClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// 주기마다 부른다. 앞 스냅숏에서 [`SNAPSHOT_EVERY`]가 지났고 코드가 바뀌었으면 사건.
    pub fn tick(&mut self, now: Instant, code: &str) -> Option<LogEvent> {
        if self
            .last_at
            .is_some_and(|at| now.saturating_duration_since(at) < SNAPSHOT_EVERY)
        {
            return None;
        }
        self.take(now, code)
    }

    /// 주기와 상관없이 지금 코드가 바뀌었으면 남긴다(로컬 실행·제출 바로 전 — 실행한 코드가 로그에 있게).
    pub fn force(&mut self, now: Instant, code: &str) -> Option<LogEvent> {
        self.take(now, code)
    }

    fn take(&mut self, now: Instant, code: &str) -> Option<LogEvent> {
        let cut = truncate(code, MAX_SNAPSHOT_BYTES);
        if self.last.as_deref() == Some(cut) {
            return None;
        }
        self.last = Some(cut.to_owned());
        self.last_at = Some(now);
        Some(LogEvent::CodeSnapshot {
            code: cut.to_owned(),
        })
    }
}

/// `max` 바이트 안의 가장 긴 앞부분(글자 경계).
pub fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// 붙여넣기를 어떻게 할까.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteOutcome {
    /// 앱 안에서 복사·잘라낸 글을 넣는다.
    Insert(String),
    /// 밖(OS 클립보드)의 글이라 막았다 — 알림과 [`LogEvent::PasteBlocked`].
    Blocked { chars: u32 },
    /// 넣을 것이 없다.
    Nothing,
}

impl PasteOutcome {
    /// 남길 과정 로그 사건.
    pub fn event(&self) -> Option<LogEvent> {
        match self {
            PasteOutcome::Blocked { chars } => Some(LogEvent::PasteBlocked { chars: *chars }),
            _ => None,
        }
    }
}

/// 막은 붙여넣기 알림.
pub const PASTE_BLOCKED_NOTICE: &str = "응시 중에는 앱 밖에서 복사한 글을 붙여 넣을 수 없습니다. 앱 안에서 복사한 글만 붙여 넣을 수 있고, 막은 붙여넣기는 과정 로그에 남습니다.";

/// 시험 응시 중 클립보드: 앱 안 복사·잘라내기는 OS 클립보드에 쓰지 않고 이 버퍼에만 둔다. 붙여넣기는 이 버퍼만
/// 넣고, 버퍼가 비었는데 OS 클립보드에 글이 있으면(밖에서 온 글) 막는다.
///
/// 버퍼가 있으면 OS 클립보드 내용과 상관없이 버퍼를 넣는다 — OS 클립보드에는 앱이 쓰지 않으므로 거기 든 글은 늘
/// 밖의 것이고, 밖의 글은 어느 경우에도 들어오지 않는다. 막은 것만 사건이다(응시 전부터 OS 클립보드에 남아 있던 글로
/// 거짓 신호가 나지 않게).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Clipboard {
    buffer: Option<String>,
}

impl Clipboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// 앱 안에서 복사·잘라냈다(빈 선택은 무시, [`MAX_SNAPSHOT_BYTES`]에서 자른다).
    pub fn copied(&mut self, text: &str) {
        if !text.is_empty() {
            self.buffer = Some(truncate(text, MAX_SNAPSHOT_BYTES).to_owned());
        }
    }

    /// 붙여넣기. `external` = 그 순간 OS 클립보드의 글.
    pub fn paste(&self, external: &str) -> PasteOutcome {
        match &self.buffer {
            Some(text) => PasteOutcome::Insert(text.clone()),
            None if external.is_empty() => PasteOutcome::Nothing,
            None => PasteOutcome::Blocked {
                chars: u32::try_from(external.chars().count()).unwrap_or(u32::MAX),
            },
        }
    }
}
