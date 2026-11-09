//! AI 도우미(#98, spec #93 5절): **문제마다 대화 하나**(새로 열기·닫기 없음). 화면 없이 도는 상태와 도구 실행 루프 —
//! 화면(`ui::ai_pane`)은 [`Chat`]을 그리고 [`ask`]·[`Chat::decide`]만 부른다.
//!
//! 한 차례([`ask`]): 응시자 메시지 → 모델 → 답에 도구 호출이 있으면 실행해 결과를 붙이고 다시 모델(최대
//! [`MAX_TOOL_ROUNDS`]번) → 도구 호출 없는 답에서 끝. 도구(켜진 것만 모델에 준다, [`enabled_tools`]):
//! - `read_problem` — 공개 문제(제목·설명·서명·예제·제한). 히든 테스트는 앱에 없다(`no_hidden_inputs.rs`).
//! - `read_code` — 편집기의 지금 코드(줄 번호)·언어. 화면은 읽기 직전에 스냅숏을 강제로 남긴다(AI가 본 코드가 로그에 있다).
//! - `propose_edit` — 줄 범위 바꾸기 제안 → [`Proposal`](대기). 편집기는 diff를 띄우고 응시자가 수락·거절한다.
//!   대기 중인 제안이 있으면 새 제안을 받지 않는다(모델에게 결정을 기다리라고 답한다). 결정은 다음 응시자 메시지 앞에
//!   알림으로 붙어 모델에게 간다. #125: 적용 뒤 코드가 적용 전보다 깨지면([`promptlab_shared::syntax::regression`] —
//!   괄호·따옴표 짝, Python 빈 블록) 차례마다 한 번 띄우지 않고 모델에게 「범위를 함수 전체로 넓혀 다시」라고 되돌린다
//!   (다시 제안할 왕복이 남았을 때만). 그래도 깨지면 띄우되 [`Proposal::warning`]으로 카드에 경고를 단다.
//! - `run_examples` — 구현돼 있지만 [`promptlab_shared::ai::AI_TOOL_RUN_EXAMPLES`]가 꺼져 있어 모델에 주지 않는다.
//!
//! 과정 로그(시험만 — 화면의 `note`가 연습이면 버린다): 응시자 메시지 `AiMessage`(응시자가 쓴 글 그대로), 모델 답마다
//! `AiReply{text, tool_calls}`(원문에서 도구 호출을 뺀 글 + 호출), 제안 `EditProposed{proposal, range, text}`, 결정
//! `EditAccepted`·`EditRejected`. 요약은 모델이 보는 대화만 바꾸고 로그(원문)는 그대로다.
//!
//! 스트리밍·취소(#116): 모델 호출마다 [`LocalModel::stream`]으로 토큰이 나오는 대로 [`Chat::live`]를 고치고 알린다 —
//! 화면은 [`Live::view`](도구 호출 JSON은 감추고 「코드 읽는 중」 같은 상태로)를 그린다. [`Cancel`]이 켜지면 생성을
//! 멈추고 그때까지 **보인** 글을 `AiReply{stopped: true}`(도구 호출 없음 — 감춘 호출 조각은 버리고 실행하지 않는다)로
//! 남기고 대화에 「멈춤」 줄을 더한다. 멈춘 글이 비면 모델 대화에 assistant 차례를 넣지 않는다. 끝까지 간 답만 평소처럼
//! `AiReply`가 된다(생성 도중의 조각은 로그에 없다).
//!
//! 상태 블록(#116): 차례를 시작할 때 공개 문제·고른 언어·지금 코드(줄 번호, 길면 가운데 생략 —
//! [`promptlab_shared::ai::context_block`])를 한 번 만들어 **이번 차례 user 메시지(응시자 글 뒤)에만** 붙인다
//! ([`render_with_context`]). 대화(`messages`)에 저장하지 않으므로 다음 차례에는 옛 블록 없이 그려진다 — 옛 코드·줄 번호가
//! 쌓여 `propose_edit` 줄 번호를 헷갈리게 하지 않고 32k 예산도 먹지 않는다. 값: 접두 KV는 직전 user 메시지 앞까지만 다시
//! 쓰고 그 차례(user·답·도구 결과)를 한 번 다시 처리한다. system에 넣으면 매 차례 맨 앞이 바뀌어 전부 다시 처리한다.
//! 블록을 만들 때 화면이 스냅숏을 강제하므로 AI가 본 코드가 과정 로그에 있다. `AiMessage`는 응시자 글 그대로다.
//!
//! 문맥(32k): 매 모델 호출 전에 토큰을 세어 [`Budget::compact_at`]을 넘으면 앞 차례들을 로컬 모델로 요약해
//! system 안 한 덩이로 바꾼다([`compaction_cut`]), 요약이 실패하거나 비면 「앞 대화 생략」. 요약은 접두를 바꾸므로
//! 그다음 호출이 접두를 한 번 다시 처리한다(그 뒤로는 다시 접두 KV 재사용).

use std::future::Future;
use std::time::Instant;

use promptlab_shared::ai::{
    Budget, EMPTY_REPLY_NOTE, FABRICATED_RESULT_NOTE, Folded, LineEdit, MAX_TOOL_ROUNDS, Message,
    REPEATED_READ_NOTE, StreamView, Tool, clip_result, compaction_cut, context_block,
    decision_note, enabled_tools, fabricated_result, numbered, parse_reply, plan_edit,
    proposal_bounced, proposal_shown, proposal_still_open, render_message,
    render_message_with_context, render_with_context, snap_edit, stream_view, summary_prompt,
    system_block,
};
use promptlab_shared::coding::{Language, PublicCodingProblem};
use promptlab_shared::inference::{Inference, InferenceError, SamplingParams};
use promptlab_shared::log::{EditRange, LogEvent, ToolCall};
use promptlab_shared::syntax::{Breakage, regression};

use crate::local::{Cancel, LocalModel};

/// 요약 답의 최대 토큰.
pub const SUMMARY_MAX_TOKENS: u32 = 400;

/// 도구가 읽는 응시 화면(편집기·문제). 화면이 신호로, 시험이 가짜로 채운다.
pub trait ToolHost {
    /// 공개 문제(없으면 `None` — 응시 중이 아님).
    fn problem(&self) -> Option<PublicCodingProblem>;
    /// 편집기의 지금 언어·코드.
    fn code(&self) -> (Language, String);
    /// 공개 예제로 로컬 실행한 결과 요약(`run_examples`, 꺼져 있다).
    fn run_examples(&self) -> impl Future<Output = String>;
}

/// 편집 제안의 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalState {
    Pending,
    Accepted,
    Rejected,
}

/// AI의 편집 제안 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// 문항 안에서 겹치지 않는 번호(1부터).
    pub id: u32,
    /// 바꿀 줄(모델이 준 그대로, 화면 표시용).
    pub lines: (u32, u32),
    pub range: EditRange,
    pub text: String,
    /// 제안을 계산한 코드(수락할 때 지금 코드와 같아야 한다).
    pub base: String,
    /// 적용 뒤 코드(diff의 오른쪽).
    pub after: String,
    pub state: ProposalState,
    /// 적용하면 코드가 적용 전보다 깨진다(#125 — 한 번 되돌려 다시 받았는데도 그럴 때만 띄운다, 카드에 경고).
    pub warning: Option<Breakage>,
}

/// 화면에 보이는 대화 한 줄(원문 그대로 — 요약해도 지우지 않는다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    User(String),
    Assistant(String),
    /// 도구를 썼다(짧은 설명).
    Tool(String),
    /// 편집 제안 카드([`Chat::proposal`]).
    Proposal(u32),
    /// 앞 대화를 접었다(`true` = 요약, `false` = 생략).
    Folded(bool),
    /// 모델 호출이 실패했다.
    Error(String),
    /// 응시자가 생성을 멈췄다(#116).
    Stopped,
    /// 다시 물어도 글·제안 없이 차례가 끝났다(#131 루프 2 — 조용히 끝나지 않게 화면에 남긴다).
    Empty,
}

/// 답하는 중인 차례의 진행(#116 — 화면의 스트리밍·진행 표시).
#[derive(Debug, Clone)]
pub struct Live {
    /// 차례를 시작한 때(경과 초).
    pub started: Instant,
    /// 이 차례에서 생성한 토큰(도구 왕복 합).
    pub tokens: usize,
    /// 지금 만드는 답의 원문(도구 호출 조각 포함 — 그리기는 [`Live::view`]).
    pub raw: String,
    /// 앞 대화를 요약하는 중이다.
    pub summarizing: bool,
}

impl Live {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            tokens: 0,
            raw: String::new(),
            summarizing: false,
        }
    }

    /// 화면에 보일 글(도구 호출 JSON은 감춘다).
    pub fn view(&self) -> StreamView {
        stream_view(&self.raw)
    }
}

/// 결정을 받을 수 없는 까닭.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecideError {
    #[error("이미 결정한 제안이다")]
    NotPending,
    #[error("제안 뒤에 코드가 바뀌어 적용할 수 없습니다. 거절하고 AI에게 다시 요청해 주세요.")]
    Stale,
}

/// 결정의 결과: 과정 로그 사건과 (수락이면) 편집기에 적용할 범위·글.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub event: LogEvent,
    pub apply: Option<(EditRange, String)>,
}

/// [`ask`]가 화면에 알리는 것.
pub enum Step<'a> {
    /// 과정 로그에 남길 사건.
    Event(LogEvent),
    /// 대화가 바뀌었다(다시 그린다).
    Changed(&'a Chat),
}

/// 문제 하나의 대화.
#[derive(Debug, Clone)]
pub struct Chat {
    folded: Option<Folded>,
    /// 모델이 보는 메시지(접은 앞부분은 빠진다).
    messages: Vec<Message>,
    entries: Vec<Entry>,
    proposals: Vec<Proposal>,
    /// 다음 응시자 메시지 앞에 붙일 알림(제안 결정).
    notes: Vec<String>,
    budget: Budget,
    params: SamplingParams,
    /// 답하는 중이면 진행(#116).
    live: Option<Live>,
    /// 이번 도구 왕복에서 깨진 제안을 되돌렸다(#125).
    bounced: bool,
}

impl Default for Chat {
    fn default() -> Self {
        Self::new()
    }
}

impl Chat {
    pub fn new() -> Self {
        Self::with_budget(Budget::app(), SamplingParams::default())
    }

    /// 예산·샘플링을 바꾼 대화(시험이 작은 창으로 요약을 부른다).
    pub fn with_budget(budget: Budget, params: SamplingParams) -> Self {
        Self {
            folded: None,
            messages: Vec::new(),
            entries: Vec::new(),
            proposals: Vec::new(),
            notes: Vec::new(),
            budget,
            params,
            live: None,
            bounced: false,
        }
    }

    /// 답하는 중인 차례의 진행.
    pub fn live(&self) -> Option<&Live> {
        self.live.as_ref()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn folded(&self) -> Option<&Folded> {
        self.folded.as_ref()
    }

    pub fn proposal(&self, id: u32) -> Option<&Proposal> {
        self.proposals.iter().find(|p| p.id == id)
    }

    /// 결정을 기다리는 제안(많아야 하나).
    pub fn pending(&self) -> Option<&Proposal> {
        self.proposals
            .iter()
            .find(|p| p.state == ProposalState::Pending)
    }

    /// 제안을 받아들이거나 버린다. 수락은 `current`(편집기의 지금 코드)가 제안을 계산한 코드와 같아야 한다.
    pub fn decide(
        &mut self,
        id: u32,
        accept: bool,
        current: &str,
    ) -> Result<Decision, DecideError> {
        let p = self
            .proposals
            .iter_mut()
            .find(|p| p.id == id && p.state == ProposalState::Pending)
            .ok_or(DecideError::NotPending)?;
        if accept && p.base != current {
            return Err(DecideError::Stale);
        }
        let (state, event) = if accept {
            (
                ProposalState::Accepted,
                LogEvent::EditAccepted { proposal: id },
            )
        } else {
            (
                ProposalState::Rejected,
                LogEvent::EditRejected { proposal: id },
            )
        };
        p.state = state;
        let apply = accept.then(|| (p.range, p.text.clone()));
        self.notes.push(decision_note(id, accept));
        Ok(Decision { event, apply })
    }

    fn next_id(&self) -> u32 {
        self.proposals.iter().map(|p| p.id).max().unwrap_or(0) + 1
    }

    /// 도구 호출 하나를 실행한다: 모델에게 돌려줄 결과 글, 로그 사건(제안이면). `may_bounce` = 깨진 제안을 띄우지 않고
    /// 모델에게 되돌려도 된다(다시 제안할 왕복이 남았고 이번 차례에 아직 되돌린 적이 없다).
    async fn run_tool(
        &mut self,
        call: &ToolCall,
        host: &impl ToolHost,
        may_bounce: bool,
    ) -> (String, Option<LogEvent>) {
        let Some(tool) = Tool::enabled_by_name(&call.name) else {
            let names: Vec<&str> = enabled_tools().iter().map(|t| t.name()).collect();
            self.entries
                .push(Entry::Tool(format!("없는 도구 `{}`", call.name)));
            return (
                format!(
                    "Error: unknown tool {:?}. Available tools: {}.",
                    call.name,
                    names.join(", ")
                ),
                None,
            );
        };
        match tool {
            Tool::ReadProblem => {
                self.entries.push(Entry::Tool("문제를 읽었습니다".into()));
                let text = host
                    .problem()
                    .map(|p| describe_problem(&p))
                    .unwrap_or_else(|| "Error: no problem is open.".into());
                (text, None)
            }
            Tool::ReadCode => {
                let (language, code) = host.code();
                let lines = code.split('\n').count();
                self.entries
                    .push(Entry::Tool(format!("코드를 읽었습니다({lines}줄)")));
                (
                    format!("Language: {}\n{}", language.as_str(), numbered(&code)),
                    None,
                )
            }
            Tool::ProposeEdit => self.propose(call, host, may_bounce),
            Tool::RunExamples => {
                self.entries
                    .push(Entry::Tool("공개 예제를 실행했습니다".into()));
                (host.run_examples().await, None)
            }
        }
    }

    fn propose(
        &mut self,
        call: &ToolCall,
        host: &impl ToolHost,
        may_bounce: bool,
    ) -> (String, Option<LogEvent>) {
        if let Some(p) = self.pending() {
            return (proposal_still_open(p.id), None);
        }
        let (language, code) = host.code();
        // 공개 서명의 풀이 함수 이름 — 머리 줄이 사라지는 제안을 잡는다(#125).
        let function = host.problem().map(|p| p.signature.name);
        let function = function.as_deref();
        let planned = LineEdit::from_arguments(&call.arguments)
            .and_then(|edit| plan_edit(&code, &edit).map(|p| (edit, p)));
        let (edit, planned) = match planned {
            Ok(v) => v,
            Err(e) => return (format!("Error: {e}"), None),
        };
        // #125: 범위 끝·머리가 한 줄 어긋나 깨지는 제안은 그 줄까지 넓혀 맞춘다(넓혀서 깨짐이 없어질 때만).
        let (edit, planned) = match snap_edit(language, function, &code, &edit) {
            Some(snapped) => match plan_edit(&code, &snapped) {
                Ok(p) => (snapped, p),
                Err(_) => (edit, planned),
            },
            None => (edit, planned),
        };
        // #125: 적용하면 적용 전보다 깨지는 제안(함수 머리·닫는 괄호를 빠뜨림 등)은 한 번 띄우지 않고 되돌린다.
        // 응시자는 보지 않았으므로 `EditProposed`도 없다(호출은 `AiReply.tool_calls`에 남는다).
        let warning = regression(language, function, &code, &planned.after);
        if let (Some(b), true) = (warning, may_bounce) {
            self.bounced = true;
            self.entries.push(Entry::Tool(
                "AI 제안이 코드를 깨뜨려 다시 물었습니다".into(),
            ));
            return (
                proposal_bounced(b, language, function, &code, edit.start_line, edit.end_line),
                None,
            );
        }
        let id = self.next_id();
        self.proposals.push(Proposal {
            id,
            lines: (edit.start_line, edit.end_line),
            range: planned.range,
            text: planned.text.clone(),
            base: code,
            after: planned.after,
            state: ProposalState::Pending,
            warning,
        });
        self.entries.push(Entry::Proposal(id));
        (
            proposal_shown(id, edit.start_line, edit.end_line),
            Some(LogEvent::EditProposed {
                proposal: id,
                range: planned.range,
                text: planned.text,
            }),
        )
    }
}

/// `read_problem` 결과(공개 정보만).
pub fn describe_problem(p: &PublicCodingProblem) -> String {
    let params: Vec<String> = p
        .signature
        .params
        .iter()
        .map(|a| format!("{}: {}", a.name, a.ty.as_str()))
        .collect();
    let mut s = format!(
        "Title: {}\nFunction: {}({}) -> {}\nTime limit per case: {} ms\n\n{}\n",
        p.title,
        p.signature.name,
        params.join(", "),
        p.signature.returns.as_str(),
        p.limits.case_time_ms,
        p.description.trim()
    );
    for (i, e) in p.examples.iter().enumerate() {
        let args = serde_json::to_string(&e.args).unwrap_or_default();
        let expected = serde_json::to_string(&e.expected).unwrap_or_default();
        s.push_str(&format!(
            "\nExample {}: arguments {args} -> {expected}",
            i + 1
        ));
    }
    clip_result(s)
}

/// 응시자 메시지 하나로 한 차례를 돈다(모델 → 도구 → 모델 …). 바뀔 때마다(스트리밍 조각마다) `on_step`으로 알린다.
/// `cancel`이 켜지면 그 자리에서 멈춘다(모듈 문서). 모델 호출이 실패하면 대화에 오류 줄을 남기고 그 오류를 돌려준다
/// (응시자 메시지는 남는다).
pub async fn ask<M: Inference + Send + Sync + 'static>(
    model: &LocalModel<M>,
    chat: &mut Chat,
    host: &impl ToolHost,
    text: &str,
    cancel: &Cancel,
    on_step: &mut impl FnMut(Step<'_>),
) -> Result<(), InferenceError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let mut full = std::mem::take(&mut chat.notes).join("\n");
    if !full.is_empty() {
        full.push('\n');
    }
    full.push_str(text);
    chat.messages.push(Message::User { text: full });
    chat.entries.push(Entry::User(text.to_owned()));
    on_step(Step::Event(LogEvent::AiMessage {
        text: text.to_owned(),
    }));
    // 상태 블록 — 화면의 `code()`가 스냅숏을 강제한다(AI가 본 코드가 로그에 남는다).
    let (language, code) = host.code();
    let problem = host.problem().map(|p| describe_problem(&p));
    let block = context_block(problem.as_deref(), language, &code);
    let at = chat.messages.len() - 1;
    chat.live = Some(Live::new());
    on_step(Step::Changed(chat));
    let from = chat.entries.len();
    let result = rounds(model, chat, host, (at, &block), cancel, on_step).await;
    let answered = chat.entries[from..].iter().any(|e| {
        matches!(
            e,
            Entry::Assistant(_) | Entry::Proposal(_) | Entry::Stopped | Entry::Error(_)
        )
    });
    if result.is_ok() && !answered {
        chat.entries.push(Entry::Empty);
    }
    chat.live = None;
    on_step(Step::Changed(chat));
    result
}

/// 한 차례의 모델·도구 왕복([`ask`]의 몸통). `context` = (이번 user 메시지 자리, 상태 블록).
async fn rounds<M: Inference + Send + Sync + 'static>(
    model: &LocalModel<M>,
    chat: &mut Chat,
    host: &impl ToolHost,
    context: (usize, &str),
    cancel: &Cancel,
    on_step: &mut impl FnMut(Step<'_>),
) -> Result<(), InferenceError> {
    let tools = enabled_tools();
    let (block, mut at) = (context.1, Some(context.0));
    let mut corrected = false;
    let mut nudged = false;
    let mut bounced = false;
    // 이번 차례에 이미 부른 읽기 호출(이름·인자) — 되풀이는 실행하지 않는다.
    let mut reads: Vec<ToolCall> = Vec::new();
    for round in 0..MAX_TOOL_ROUNDS {
        if cancel.is_cancelled() {
            chat.entries.push(Entry::Stopped);
            return Ok(());
        }
        match compact(model, chat, &tools, at.map(|i| (i, block)), cancel, on_step).await {
            Compaction::Kept => {}
            Compaction::Cut(n) => at = at.and_then(|i| i.checked_sub(n)),
            Compaction::Stopped => {
                chat.entries.push(Entry::Stopped);
                return Ok(());
            }
        }
        let prompt = render_with_context(
            chat.folded.as_ref(),
            &chat.messages,
            &tools,
            at.map(|i| (i, block)),
        );
        let base = chat.live.as_ref().map_or(0, |l| l.tokens);
        let params = chat.params;
        let generation = model
            .stream(prompt, params, cancel, |raw, n| {
                if let Some(live) = chat.live.as_mut() {
                    live.raw = raw.to_owned();
                    live.tokens = base + n;
                }
                on_step(Step::Changed(chat));
            })
            .await;
        if let Some(live) = chat.live.as_mut() {
            live.raw.clear();
        }
        let generation = match generation {
            Ok(g) => g,
            Err(e) => {
                chat.entries.push(Entry::Error(e.to_string()));
                return Err(e);
            }
        };
        if generation.stopped {
            // 보인 글만 남긴다(감춘 도구 호출 조각은 실행하지 않고 버린다).
            let shown = stream_view(&generation.text).text;
            on_step(Step::Event(LogEvent::AiReply {
                text: shown.clone(),
                tool_calls: Vec::new(),
                stopped: true,
            }));
            if !shown.is_empty() {
                chat.entries.push(Entry::Assistant(shown.clone()));
                chat.messages.push(Message::Assistant {
                    text: shown,
                    tool_calls: Vec::new(),
                });
            }
            chat.entries.push(Entry::Stopped);
            return Ok(());
        }
        let (reply, calls) = parse_reply(&generation.text);
        on_step(Step::Event(LogEvent::AiReply {
            text: reply.clone(),
            tool_calls: calls.clone(),
            stopped: false,
        }));
        // 도구 결과를 지어 낸 답은 응시자에게 보이지 않고 한 번 바로잡아 다시 묻는다(남은 왕복이 있을 때).
        let retry = calls.is_empty()
            && !corrected
            && round + 1 < MAX_TOOL_ROUNDS
            && fabricated_result(&reply);
        // 글도 호출도 없는 답(1.5B가 도구 결과 뒤에 바로 끝냈다)은 한 번 다시 묻는다(남은 왕복이 있을 때).
        if calls.is_empty() && reply.trim().is_empty() && !nudged && round + 1 < MAX_TOOL_ROUNDS {
            nudged = true;
            chat.messages.push(Message::User {
                text: EMPTY_REPLY_NOTE.to_owned(),
            });
            on_step(Step::Changed(chat));
            continue;
        }
        if retry {
            chat.entries.push(Entry::Tool(
                "AI가 도구 형식을 잘못 써서 다시 물었습니다".into(),
            ));
        } else if !reply.is_empty() {
            chat.entries.push(Entry::Assistant(reply.clone()));
        }
        // 빈 답(글·호출 없음)은 모델 대화에 넣지 않는다 — 빈 assistant 차례가 남으면 1.5B가 다음 질문에도 그대로 따라
        // 빈 답·`<|im_start|>`만 냈다(#131 루프 2, 멈춘 빈 답과 같다).
        if calls.is_empty() && reply.trim().is_empty() {
            return Ok(());
        }
        chat.messages.push(Message::Assistant {
            text: reply,
            tool_calls: calls.clone(),
        });
        if retry {
            corrected = true;
            chat.messages.push(Message::User {
                text: FABRICATED_RESULT_NOTE.to_owned(),
            });
            on_step(Step::Changed(chat));
            continue;
        }
        if calls.is_empty() {
            return Ok(());
        }
        let mut results = Vec::with_capacity(calls.len());
        let may_bounce = !bounced && round + 1 < MAX_TOOL_ROUNDS;
        chat.bounced = false;
        for call in &calls {
            let read = matches!(call.name.as_str(), "read_code" | "read_problem");
            if read && reads.contains(call) {
                results.push(REPEATED_READ_NOTE.to_owned());
                continue;
            }
            if read {
                reads.push(call.clone());
            }
            let (result, event) = chat.run_tool(call, host, may_bounce && !chat.bounced).await;
            if let Some(e) = event {
                on_step(Step::Event(e));
            }
            results.push(clip_result(result));
        }
        bounced |= chat.bounced;
        chat.messages.push(Message::ToolResults { results });
        on_step(Step::Changed(chat));
    }
    Ok(())
}

/// [`compact`]의 결과.
enum Compaction {
    /// 접지 않았다.
    Kept,
    /// 앞 메시지 n개를 접었다.
    Cut(usize),
    /// 요약하는 동안 응시자가 멈췄다(접지 않았다).
    Stopped,
}

/// 문맥이 차면 앞을 접는다(모듈 문서). 토큰을 셀 수 없으면 건너뛴다(모델 호출이 `TooLong`으로 알린다).
/// `context` = 이번 user 메시지에 붙는 상태 블록(세기에 넣는다).
async fn compact<M: Inference + Send + Sync + 'static>(
    model: &LocalModel<M>,
    chat: &mut Chat,
    tools: &[Tool],
    context: Option<(usize, &str)>,
    cancel: &Cancel,
    on_step: &mut impl FnMut(Step<'_>),
) -> Compaction {
    let mut texts = vec![system_block(chat.folded.as_ref(), tools)];
    texts.extend(
        chat.messages
            .iter()
            .enumerate()
            .map(|(i, m)| match context {
                Some((at, block)) if at == i => render_message_with_context(m, block),
                _ => render_message(m),
            }),
    );
    let Ok(counts) = model.count(texts).await else {
        return Compaction::Kept;
    };
    let Some(cut) = compaction_cut(
        &chat.messages,
        &counts[1..],
        counts[0],
        &chat.budget.capped(model.context_tokens()),
    ) else {
        return Compaction::Kept;
    };
    let prompt = summary_prompt(chat.folded.as_ref(), &chat.messages[..cut]);
    let params = SamplingParams {
        max_tokens: SUMMARY_MAX_TOKENS,
        ..chat.params
    };
    if let Some(live) = chat.live.as_mut() {
        live.summarizing = true;
    }
    on_step(Step::Changed(chat));
    let summary = model.stream(prompt, params, cancel, |_, _| {}).await;
    if let Some(live) = chat.live.as_mut() {
        live.summarizing = false;
    }
    let folded = match summary {
        Ok(g) if g.stopped => return Compaction::Stopped,
        Ok(g) if !g.text.trim().is_empty() => Folded::Summary(g.text.trim().to_owned()),
        _ => Folded::Omitted,
    };
    let summarized = matches!(folded, Folded::Summary(_));
    chat.folded = Some(folded);
    chat.messages.drain(..cut);
    chat.entries.push(Entry::Folded(summarized));
    on_step(Step::Changed(chat));
    Compaction::Cut(cut)
}
