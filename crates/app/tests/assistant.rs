//! AI 도우미 도구 루프(#98): 가짜 추론(정해 둔 답)과 가짜 편집기로 차례·도구·제안·결정·로그 사건·요약을 본다.
//! #116: 가짜 추론은 답을 글자 하나씩 흘린다(스트리밍) — 진행·도구 호출 감춤·취소·상태 블록(문제·언어·코드)도 본다.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use promptlab_app::assistant::{
    Chat, DecideError, Entry, ProposalState, Step, ToolHost, ask, describe_problem,
};
use promptlab_app::local::{Cancel, LocalModel};
use promptlab_shared::ai::{Budget, Folded, Hidden, Message, Tool};
use promptlab_shared::coding::{
    DEFAULT_CASE_TIME_MS, Language, Limits, Param, PublicCodingProblem, Signature, TestCase,
    ValueType, templates,
};
use promptlab_shared::inference::{Generation, Inference, InferenceError, SamplingParams};
use promptlab_shared::log::{EditRange, LogEvent};
use promptlab_shared::syntax::Breakage;
use serde_json::json;

/// 정해 둔 답을 차례로 낸다. 요약 프롬프트에는 `summary`로 답한다. 받은 프롬프트를 남긴다.
#[derive(Default)]
struct Scripted {
    replies: Mutex<VecDeque<Result<String, String>>>,
    summary: Mutex<Option<Result<String, String>>>,
    prompts: Mutex<Vec<String>>,
    /// 조각마다 시험이 받았다고 알릴 때까지 기다린다(취소 시점을 결정적으로 — [`watch`]).
    lockstep: AtomicBool,
    sent: AtomicUsize,
    acked: AtomicUsize,
}

impl Scripted {
    fn new(replies: &[&str]) -> Self {
        let s = Self::default();
        *s.replies.lock().unwrap() = replies.iter().map(|r| Ok(r.to_string())).collect();
        s
    }
}

impl Scripted {
    fn next(&self, full: String) -> Result<String, InferenceError> {
        self.prompts.lock().unwrap().push(full.clone());
        let next = if full.contains("You summarize conversations") {
            self.summary
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(Ok("요약".into()))
        } else {
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok("끝".into()))
        };
        next.map_err(InferenceError::Failed)
    }
}

impl Inference for Scripted {
    fn complete(
        &self,
        prefix: &str,
        suffixes: &[&str],
        _: &SamplingParams,
    ) -> Result<Vec<String>, InferenceError> {
        self.next(format!("{prefix}{}", suffixes[0]))
            .map(|r| vec![r])
    }

    /// 글자 하나를 토큰 하나로 흘린다. `on_text`가 `false`면 그때까지로 멈춘다.
    fn stream(
        &self,
        prefix: &str,
        suffix: &str,
        _: &SamplingParams,
        on_text: &mut dyn FnMut(&str, usize) -> bool,
    ) -> Result<Generation, InferenceError> {
        let reply = self.next(format!("{prefix}{suffix}"))?;
        for (n, (at, c)) in reply.char_indices().enumerate() {
            let end = at + c.len_utf8();
            let go = on_text(&reply[..end], n + 1);
            if self.lockstep.load(Ordering::SeqCst) {
                let sent = self.sent.fetch_add(1, Ordering::SeqCst) + 1;
                let deadline = Instant::now() + Duration::from_secs(5);
                while self.acked.load(Ordering::SeqCst) < sent && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            if !go {
                return Ok(Generation {
                    text: reply[..end].to_owned(),
                    stopped: true,
                    stats: None,
                });
            }
        }
        Ok(Generation {
            text: reply,
            stopped: false,
            stats: None,
        })
    }

    fn count_tokens(&self, text: &str) -> Result<usize, InferenceError> {
        Ok(text.chars().count())
    }
}

fn problem() -> PublicCodingProblem {
    let signature = Signature {
        name: "add_one".into(),
        params: vec![Param {
            name: "n".into(),
            ty: ValueType::Int,
        }],
        returns: ValueType::Int,
    };
    PublicCodingProblem {
        id: "p-1".into(),
        title: "하나 더하기".into(),
        description: "n에 1을 더하라".into(),
        templates: templates(&signature),
        signature,
        examples: vec![TestCase {
            args: vec![json!(1)],
            expected: json!(2),
        }],
        limits: Limits {
            case_time_ms: DEFAULT_CASE_TIME_MS,
        },
    }
}

const CODE: &str = "def add_one(n):\n    return n\n";

struct Host {
    code: RefCell<String>,
    language: Cell<Language>,
    ran: Cell<u32>,
    /// `code()`를 부른 횟수(화면은 이때 스냅숏을 강제한다).
    reads: Cell<u32>,
}

impl Host {
    fn new() -> Self {
        Self {
            code: RefCell::new(CODE.into()),
            language: Cell::new(Language::Python),
            ran: Cell::new(0),
            reads: Cell::new(0),
        }
    }
}

impl ToolHost for Host {
    fn problem(&self) -> Option<PublicCodingProblem> {
        Some(problem())
    }
    fn code(&self) -> (Language, String) {
        self.reads.set(self.reads.get() + 1);
        (self.language.get(), self.code.borrow().clone())
    }
    async fn run_examples(&self) -> String {
        self.ran.set(self.ran.get() + 1);
        "Example 1: passed".into()
    }
}

fn call(name: &str, args: serde_json::Value) -> String {
    format!(
        "<tool_call>\n{}\n</tool_call>",
        json!({"name": name, "arguments": args})
    )
}

/// 한 차례를 돌리고 로그 사건을 모은다.
async fn turn(
    model: &LocalModel<Scripted>,
    chat: &mut Chat,
    host: &Host,
    text: &str,
) -> (Result<(), InferenceError>, Vec<LogEvent>) {
    let mut events = Vec::new();
    let r = ask(model, chat, host, text, &Cancel::new(), &mut |s| {
        if let Step::Event(e) = s {
            events.push(e);
        }
    })
    .await;
    (r, events)
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
}

#[test]
fn 코드를_읽고_편집을_제안하고_수락하면_로그와_적용_범위가_나온다() {
    rt().block_on(async {
        let model = LocalModel::new(Scripted::new(&[
            &call("read_code", json!({})),
            &format!(
                "고칠게요.\n{}",
                call(
                    "propose_edit",
                    json!({"start_line": 2, "end_line": 2, "text": "    return n + 1\n"})
                )
            ),
            "diff를 확인해 주세요.",
        ]));
        let host = Host::new();
        let mut chat = Chat::new();
        let (r, events) = turn(&model, &mut chat, &host, "  고쳐 줘 ").await;
        r.unwrap();

        let range = EditRange {
            start_line: 2,
            start_column: 1,
            end_line: 2,
            end_column: 13,
        };
        assert_eq!(
            events,
            vec![
                LogEvent::AiMessage {
                    text: "고쳐 줘".into()
                },
                LogEvent::AiReply {
                    text: String::new(),
                    tool_calls: vec![promptlab_shared::log::ToolCall {
                        name: "read_code".into(),
                        arguments: json!({})
                    }],
                    stopped: false,
                },
                LogEvent::AiReply {
                    text: "고칠게요.".into(),
                    tool_calls: vec![promptlab_shared::log::ToolCall {
                        name: "propose_edit".into(),
                        arguments: json!({"start_line": 2, "end_line": 2, "text": "    return n + 1\n"})
                    }],
                    stopped: false,
                },
                LogEvent::EditProposed {
                    proposal: 1,
                    range,
                    text: "    return n + 1".into()
                },
                LogEvent::AiReply {
                    text: "diff를 확인해 주세요.".into(),
                    tool_calls: vec![],
                    stopped: false,
                },
            ]
        );
        assert_eq!(
            chat.entries(),
            &[
                Entry::User("고쳐 줘".into()),
                Entry::Tool("코드를 읽었습니다(3줄)".into()),
                Entry::Assistant("고칠게요.".into()),
                Entry::Proposal(1),
                Entry::Assistant("diff를 확인해 주세요.".into()),
            ]
        );
        // 모델이 본 도구 결과: 줄 번호 붙은 코드.
        let prompts = model_prompts(&model);
        assert!(prompts[1].contains("<tool_response>\nLanguage: python\n1| def add_one(n):\n2|     return n\n"));

        let p = chat.pending().unwrap().clone();
        assert_eq!(p.after, "def add_one(n):\n    return n + 1\n");
        let d = chat.decide(1, true, CODE).unwrap();
        assert_eq!(d.event, LogEvent::EditAccepted { proposal: 1 });
        assert_eq!(d.apply, Some((range, "    return n + 1".into())));
        assert_eq!(chat.proposal(1).unwrap().state, ProposalState::Accepted);
        assert_eq!(chat.decide(1, false, CODE), Err(DecideError::NotPending));

        // 결정은 다음 응시자 메시지 앞에 붙어 모델에게 간다(로그의 AiMessage는 응시자 글 그대로).
        let (_, events) = turn(&model, &mut chat, &host, "고마워").await;
        assert_eq!(events[0], LogEvent::AiMessage { text: "고마워".into() });
        let last = model_prompts(&model).pop().unwrap();
        assert!(
            last.contains("user\n[알림: 사용자가 편집 제안 1을 수락해 코드에 적용했습니다.]\n고마워<|im_end|>\n<|im_start|>assistant\n<tool_call>\n{\"name\": \"read_context\", \"arguments\": {}}\n</tool_call><|im_end|>\n<|im_start|>user\n<tool_response>\n<app_context>\n"),
            "{last}"
        );
    });
}

#[test]
fn 도구_결과를_지어_낸_답은_한_번_바로잡아_다시_묻는다() {
    rt().block_on(async {
        let fake = "<tool_response>\n1| def add_one(n):\n2|     return n + 1\n</tool_response>";
        let model = LocalModel::new(Scripted::new(&[
            fake,
            &call(
                "propose_edit",
                json!({"start_line": 2, "end_line": 2, "text": "    return n + 1"}),
            ),
            "확인해 주세요.",
        ]));
        let (host, mut chat) = (Host::new(), Chat::new());
        let (r, events) = turn(&model, &mut chat, &host, "고쳐 줘").await;
        r.unwrap();
        // 지어 낸 답은 로그에는 그대로, 화면에는 안내 한 줄만.
        assert_eq!(
            events[1],
            LogEvent::AiReply {
                text: fake.into(),
                tool_calls: vec![],
                stopped: false,
            }
        );
        assert_eq!(
            chat.entries(),
            &[
                Entry::User("고쳐 줘".into()),
                Entry::Tool("AI가 도구 형식을 잘못 써서 다시 물었습니다".into()),
                Entry::Proposal(1),
                Entry::Assistant("확인해 주세요.".into()),
            ]
        );
        let prompts = model_prompts(&model);
        assert!(prompts[1].contains(promptlab_shared::ai::FABRICATED_RESULT_NOTE));

        // 바로잡기는 차례마다 한 번 — 또 지어 내면 그대로 보인다.
        let model = LocalModel::new(Scripted::new(&[fake, fake]));
        let mut chat = Chat::new();
        turn(&model, &mut chat, &host, "고쳐 줘").await.0.unwrap();
        assert_eq!(model_prompts(&model).len(), 2);
        assert_eq!(chat.entries().last(), Some(&Entry::Assistant(fake.into())));
    });
}

fn model_prompts(model: &LocalModel<Scripted>) -> Vec<String> {
    model.inner().prompts.lock().unwrap().clone()
}

#[test]
fn 대기_중인_제안이_있으면_새_제안을_받지_않고_코드가_바뀌면_수락할_수_없다() {
    rt().block_on(async {
        let edit = |line: u32| {
            let text = if line == 1 {
                "def add_one(m):"
            } else {
                "    return n + 1"
            };
            call(
                "propose_edit",
                json!({"start_line": line, "end_line": line, "text": text}),
            )
        };
        let model = LocalModel::new(Scripted::new(&[&edit(1), &edit(2), "끝"]));
        let host = Host::new();
        let mut chat = Chat::new();
        let (_, events) = turn(&model, &mut chat, &host, "둘 다 고쳐").await;
        let proposed = events
            .iter()
            .filter(|e| matches!(e, LogEvent::EditProposed { .. }))
            .count();
        assert_eq!(proposed, 1);
        assert!(model_prompts(&model)[2].contains("편집 제안 1이 아직 편집기에 떠 있고"));

        // 응시자가 코드를 고친 뒤에는 수락할 수 없다 — 거절은 된다.
        assert_eq!(chat.decide(1, true, "다른 코드"), Err(DecideError::Stale));
        let d = chat.decide(1, false, "다른 코드").unwrap();
        assert_eq!(d.event, LogEvent::EditRejected { proposal: 1 });
        assert_eq!(d.apply, None);
    });
}

#[test]
fn 범위가_틀린_제안은_모델에게_오류로_돌려준다() {
    rt().block_on(async {
        let model = LocalModel::new(Scripted::new(&[
            &call(
                "propose_edit",
                json!({"start_line": 9, "end_line": 9, "text": "x"}),
            ),
            "다시 읽을게요",
        ]));
        let host = Host::new();
        let mut chat = Chat::new();
        let (_, events) = turn(&model, &mut chat, &host, "고쳐").await;
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, LogEvent::EditProposed { .. }))
        );
        assert!(chat.pending().is_none());
        assert!(model_prompts(&model)[1].contains("Error: line range 9..9 is outside the code"));
    });
}

#[test]
fn 꺼진_run_examples는_모델이_불러도_돌지_않는다() {
    rt().block_on(async {
        let model = LocalModel::new(Scripted::new(&[
            &call("run_examples", json!({})),
            "알겠어요",
        ]));
        let host = Host::new();
        let mut chat = Chat::new();
        turn(&model, &mut chat, &host, "예제 돌려 줘")
            .await
            .0
            .unwrap();
        assert_eq!(host.ran.get(), 0);
        let prompts = model_prompts(&model);
        assert!(
            !prompts[0].contains("run_examples"),
            "도구 목록에 없어야 한다"
        );
        assert!(prompts[1].contains("Error: unknown tool \"run_examples\""));
    });
}

#[test]
fn read_problem은_공개_문제만_준다() {
    let text = describe_problem(&problem());
    assert!(text.contains("Title: 하나 더하기"));
    assert!(text.contains("Function: add_one(n: int) -> int"));
    assert!(text.contains("Example 1: arguments [1] -> 2"));
}

#[test]
fn 도구_왕복은_차례마다_상한이_있다() {
    rt().block_on(async {
        let loop_call = call("read_problem", json!({}));
        let replies: Vec<&str> = vec![&loop_call; 10];
        let model = LocalModel::new(Scripted::new(&replies));
        let host = Host::new();
        let mut chat = Chat::new();
        turn(&model, &mut chat, &host, "문제").await.0.unwrap();
        assert_eq!(
            model_prompts(&model).len(),
            promptlab_shared::ai::MAX_TOOL_ROUNDS
        );
        // 마지막 도구 결과가 붙은 채로 끝난다 — 다음 메시지로 이어 간다.
        assert!(matches!(
            chat.messages().last(),
            Some(Message::ToolResults { .. })
        ));
    });
}

fn small_chat() -> Chat {
    // system 블록이 2,000자 남짓이라 그만큼 + 대화 몇 차례에서 접히게 잡는다(가짜 토큰 = 글자 수).
    let fixed = promptlab_shared::ai::system_block(None, &promptlab_shared::ai::enabled_tools())
        .chars()
        .count();
    Chat::with_budget(
        Budget {
            context: fixed + 400,
            reply: 50,
            margin: 50,
        },
        SamplingParams::default(),
    )
}

#[test]
fn 문맥이_차면_앞_대화를_요약하고_실패하면_생략한다() {
    rt().block_on(async {
        let long = "가".repeat(60);
        let replies: Vec<String> = (0..8).map(|i| format!("답{i} {long}")).collect();
        let refs: Vec<&str> = replies.iter().map(String::as_str).collect();
        let model = LocalModel::new(Scripted::new(&refs));
        let host = Host::new();
        let mut chat = small_chat();
        for i in 0..4 {
            turn(&model, &mut chat, &host, &format!("질문{i} {long}"))
                .await
                .0
                .unwrap();
        }
        assert!(chat.entries().contains(&Entry::Folded(true)));
        assert_eq!(chat.folded(), Some(&Folded::Summary("요약".into())));
        // 화면의 원문은 그대로다.
        assert_eq!(
            chat.entries()
                .iter()
                .filter(|e| matches!(e, Entry::User(_)))
                .count(),
            4
        );
        // 접은 뒤 모델 프롬프트에는 요약이 system 안에, 첫 질문은 없다.
        let last = model_prompts(&model).pop().unwrap();
        assert!(last.contains("Summary of the earlier conversation:\n요약"));
        assert!(!last.contains("질문0"));

        // 요약 실패 → 생략.
        let model = LocalModel::new(Scripted::new(&refs));
        *model.inner().summary.lock().unwrap() = Some(Err("고장".into()));
        let mut chat = small_chat();
        for i in 0..4 {
            turn(&model, &mut chat, &host, &format!("질문{i} {long}"))
                .await
                .0
                .unwrap();
        }
        assert!(chat.entries().contains(&Entry::Folded(false)));
        assert_eq!(chat.folded(), Some(&Folded::Omitted));
    });
}

#[test]
fn 모델이_실패하면_오류_줄을_남긴다() {
    rt().block_on(async {
        let model = LocalModel::new(Scripted::default());
        model
            .inner()
            .replies
            .lock()
            .unwrap()
            .push_back(Err("메모리 부족".into()));
        let host = Host::new();
        let mut chat = Chat::new();
        let (r, _) = turn(&model, &mut chat, &host, "안녕").await;
        assert!(r.is_err());
        assert!(
            matches!(chat.entries().last(), Some(Entry::Error(e)) if e.contains("메모리 부족"))
        );
        // 빈 메시지는 아무것도 하지 않는다.
        let (r, events) = turn(&model, &mut chat, &host, "   ").await;
        assert!(r.is_ok() && events.is_empty());
    });
}

/// 진행 화면이 본 것: 스트리밍 중 보인 글·감춘 상태·토큰 수.
#[derive(Default)]
struct Seen {
    texts: Vec<String>,
    hidden: Vec<Option<Hidden>>,
    tokens: Vec<usize>,
    events: Vec<LogEvent>,
}

/// 한 차례를 돌리며 진행을 모은다. `stop_at`이면 그 토큰 수에서 「멈추기」를 누른다.
async fn watch(
    model: &LocalModel<Scripted>,
    chat: &mut Chat,
    host: &Host,
    text: &str,
    stop_at: Option<usize>,
) -> Seen {
    let cancel = Cancel::new();
    let mut seen = Seen::default();
    let fake = model.inner();
    fake.lockstep.store(true, Ordering::SeqCst);
    let mut last = 0usize;
    let r = ask(model, chat, host, text, &cancel, &mut |s| match s {
        Step::Event(e) => seen.events.push(e),
        Step::Changed(c) => {
            if let Some(live) = c.live() {
                let piece = live.tokens > last;
                last = live.tokens;
                let v = live.view();
                seen.texts.push(v.text);
                seen.hidden.push(v.hidden);
                seen.tokens.push(live.tokens);
                if stop_at.is_some_and(|n| live.tokens >= n) {
                    cancel.cancel();
                }
                if piece {
                    fake.acked.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    })
    .await;
    r.unwrap();
    fake.lockstep.store(false, Ordering::SeqCst);
    assert!(chat.live().is_none(), "차례가 끝나면 진행이 없다");
    seen
}

#[test]
fn 답은_글자가_나오는_대로_보이고_도구_호출_json은_감춘_채_도구_이름으로_알린다() {
    rt().block_on(async {
        let model = LocalModel::new(Scripted::new(&[
            &format!("읽어 볼게요.\n{}", call("read_code", json!({}))),
            "2번 줄이 n을 그대로 돌려줍니다.",
        ]));
        let mut chat = Chat::new();
        let host = Host::new();
        let seen = watch(&model, &mut chat, &host, "버그?", None).await;
        // 조각이 늘어나며 보였다.
        assert!(seen.texts.iter().any(|t| t == "읽어"), "{:?}", seen.texts);
        assert!(seen.texts.iter().any(|t| t == "2번 줄이"));
        // 호출 JSON은 한 번도 보이지 않았다.
        assert!(
            seen.texts
                .iter()
                .all(|t| !t.contains('{') && !t.contains("<tool")),
            "{:?}",
            seen.texts
        );
        assert!(
            seen.hidden
                .contains(&Some(Hidden::Call(Some(Tool::ReadCode))))
        );
        // 토큰 수는 차례 동안 도구 왕복을 합쳐 늘기만 한다.
        assert!(
            seen.tokens.windows(2).all(|w| w[0] <= w[1]),
            "{:?}",
            seen.tokens
        );
        let answer = "2번 줄이 n을 그대로 돌려줍니다.";
        assert!(*seen.tokens.last().unwrap() >= answer.chars().count());
        // 끝난 답만 로그에(멈추지 않음).
        let replies: Vec<_> = seen
            .events
            .iter()
            .filter_map(|e| match e {
                LogEvent::AiReply { text, stopped, .. } => Some((text.clone(), *stopped)),
                _ => None,
            })
            .collect();
        assert_eq!(
            replies,
            vec![
                ("읽어 볼게요.".to_owned(), false),
                (answer.to_owned(), false)
            ]
        );
        assert_eq!(
            chat.entries().last(),
            Some(&Entry::Assistant(answer.into()))
        );
    });
}

#[test]
fn 멈추면_보인_글까지_남기고_stopped로_로그하고_다음_차례가_이어진다() {
    rt().block_on(async {
        let model = LocalModel::new(Scripted::new(&["가나다라마바사", "다음 답"]));
        let mut chat = Chat::new();
        let host = Host::new();
        let seen = watch(&model, &mut chat, &host, "길게 설명해 줘", Some(3)).await;
        let reply = seen
            .events
            .iter()
            .find_map(|e| match e {
                LogEvent::AiReply {
                    text,
                    tool_calls,
                    stopped,
                } => Some((text.clone(), tool_calls.len(), *stopped)),
                _ => None,
            })
            .unwrap();
        // 3번째 글자에서 누르면 그다음 토큰에서 멈춘다.
        assert_eq!(reply, ("가나다라".to_owned(), 0, true));
        assert_eq!(
            &chat.entries()[1..],
            &[Entry::Assistant("가나다라".into()), Entry::Stopped]
        );
        assert_eq!(
            chat.messages().last(),
            Some(&Message::Assistant {
                text: "가나다라".into(),
                tool_calls: vec![]
            })
        );
        // 다음 차례는 평소대로.
        let (r, _) = turn(&model, &mut chat, &host, "계속").await;
        r.unwrap();
        assert_eq!(
            chat.entries().last(),
            Some(&Entry::Assistant("다음 답".into()))
        );
    });
}

#[test]
fn 도구_호출을_만드는_중에_멈추면_실행하지_않고_빈_답으로_남긴다() {
    rt().block_on(async {
        let edit = call(
            "propose_edit",
            json!({"start_line": 2, "end_line": 2, "text": "    return n + 1\n"}),
        );
        let model = LocalModel::new(Scripted::new(&[&edit]));
        let mut chat = Chat::new();
        let host = Host::new();
        let seen = watch(&model, &mut chat, &host, "고쳐 줘", Some(40)).await;
        assert!(
            seen.events
                .iter()
                .all(|e| !matches!(e, LogEvent::EditProposed { .. }))
        );
        assert!(seen.events.contains(&LogEvent::AiReply {
            text: String::new(),
            tool_calls: vec![],
            stopped: true,
        }));
        assert!(chat.pending().is_none());
        // 빈 답은 모델 대화에 넣지 않는다.
        assert!(matches!(chat.messages().last(), Some(Message::User { .. })));
        assert_eq!(chat.entries().last(), Some(&Entry::Stopped));
    });
}

#[test]
fn 매_차례_문제_언어_지금_코드를_상태_블록으로_넣고_로그의_응시자_글은_그대로다() {
    rt().block_on(async {
        let model = LocalModel::new(Scripted::new(&["네", "네"]));
        let mut chat = Chat::new();
        let host = Host::new();
        let seen = watch(&model, &mut chat, &host, "이 코드 봐 줘", None).await;
        assert_eq!(host.reads.get(), 1, "차례 시작에 코드를 한 번 읽는다(스냅숏 강제)");
        let prompts = model_prompts(&model);
        let first = &prompts[0];
        assert!(first.contains("<app_context>\nProblem:\nTitle: 하나 더하기"), "{first}");
        assert!(first.contains("Function: add_one(n: int) -> int"));
        assert!(first.contains("Language: python"));
        assert!(first.contains("1| def add_one(n):\n2|     return n\n3| \n</app_context>\n</tool_response><|im_end|>"), "{first}");
        assert!(first.contains("user\n이 코드 봐 줘<|im_end|>\n<|im_start|>assistant\n<tool_call>\n{\"name\": \"read_context\", \"arguments\": {}}\n</tool_call><|im_end|>\n<|im_start|>user\n<tool_response>\n<app_context>\n"), "{first}");
        assert!(seen.events.contains(&LogEvent::AiMessage {
            text: "이 코드 봐 줘".into()
        }));
        assert_eq!(
            chat.messages()[0],
            Message::User {
                text: "이 코드 봐 줘".into()
            },
            "블록은 대화에 저장하지 않는다"
        );

        // 응시자가 언어를 바꾸고 코드를 고쳤다 → 다음 차례에는 새 블록 하나만.
        host.language.set(Language::Java);
        *host.code.borrow_mut() = "class Solution {}\n".into();
        let (r, _) = turn(&model, &mut chat, &host, "자바로 바꿨어").await;
        r.unwrap();
        let second = &model_prompts(&model)[1];
        assert_eq!(second.matches("<app_context>\n").count(), 1, "{second}");
        assert!(second.contains("Language: java"));
        assert!(second.contains("1| class Solution {}"));
        assert!(!second.contains("Language: python"));
    });
}

/// #125 (c): 함수 머리를 지운 제안 — 띄우지 않고 한 번 되돌려 다시 받는다. 응시자는 되돌린 제안을 보지 않는다
/// (`EditProposed` 없음, 호출은 `AiReply.tool_calls`에 남는다). 다시 받은 제안이 멀쩡하면 경고 없이 띄운다.
#[test]
fn 코드를_깨는_제안은_한_번_되돌려_다시_받는다() {
    rt().block_on(async {
        let broken = call(
            "propose_edit",
            json!({"start_line": 1, "end_line": 2, "text": "return n + 1"}),
        );
        let fixed = call(
            "propose_edit",
            json!({"start_line": 1, "end_line": 2, "text": "def add_one(n):\n    return n + 1"}),
        );
        let model = LocalModel::new(Scripted::new(&[&broken, &fixed, "1을 더하게 고쳤어요."]));
        let host = Host::new();
        let mut chat = Chat::new();
        let (r, events) = turn(&model, &mut chat, &host, "버그 고쳐 줘").await;
        r.unwrap();
        let proposed: Vec<&LogEvent> = events
            .iter()
            .filter(|e| matches!(e, LogEvent::EditProposed { .. }))
            .collect();
        assert_eq!(proposed.len(), 1, "{events:?}");
        let p = chat.pending().unwrap();
        assert_eq!(p.id, 1);
        assert_eq!(p.warning, None);
        assert_eq!(p.after, "def add_one(n):\n    return n + 1\n");
        let prompts = model_prompts(&model);
        assert!(
            prompts[1].contains("오류: 이 편집은 사용자에게 보이지 않았습니다"),
            "{}",
            prompts[1]
        );
        assert!(prompts[1].contains("함수 머리 `def add_one(n):`"));
        // 띄운 뒤의 결과는 짧은 한국어 설명을 시킨다.
        assert!(prompts[2].contains("편집 제안 1(1–2줄)이 편집기에 diff로 떴습니다"));
        assert!(prompts[2].contains("한국어 한두 문장"));
        assert!(chat.entries().contains(&Entry::Tool(
            "AI 제안이 코드를 깨뜨려 다시 물었습니다".into()
        )));
        assert_eq!(
            chat.entries()
                .iter()
                .filter(|e| matches!(e, Entry::Proposal(_)))
                .count(),
            1
        );
    });
}

/// 되돌린 뒤에도 또 깨진 제안이면 띄우되 경고를 단다(차례마다 되돌리기는 한 번).
#[test]
fn 다시_받은_제안도_깨지면_경고를_달아_띄운다() {
    rt().block_on(async {
        let broken = call(
            "propose_edit",
            json!({"start_line": 1, "end_line": 1, "text": "def add_one(n:"}),
        );
        let model = LocalModel::new(Scripted::new(&[&broken, &broken, "고쳤어요."]));
        let host = Host::new();
        let mut chat = Chat::new();
        let (_, events) = turn(&model, &mut chat, &host, "고쳐").await;
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, LogEvent::EditProposed { .. }))
                .count(),
            1
        );
        assert_eq!(chat.pending().unwrap().warning, Some(Breakage::Unbalanced));
        assert!(Breakage::Unbalanced.warning().contains("괄호"));
    });
}

/// 다시 제안할 왕복이 남지 않았으면(마지막 왕복의 제안) 되돌리지 않고 경고를 달아 띄운다.
#[test]
fn 마지막_왕복의_깨진_제안은_되돌리지_않고_경고한다() {
    rt().block_on(async {
        let read = call("read_code", json!({}));
        let broken = call(
            "propose_edit",
            json!({"start_line": 2, "end_line": 2, "text": "    return (n + 1"}),
        );
        let model = LocalModel::new(Scripted::new(&[&read, &read, &read, &broken]));
        let host = Host::new();
        let mut chat = Chat::new();
        turn(&model, &mut chat, &host, "고쳐").await.0.unwrap();
        assert_eq!(chat.pending().unwrap().warning, Some(Breakage::Unbalanced));
    });
}

/// 이미 깨진 코드(쓰는 중)는 제안이 더 깨지 않으면 경고하지 않는다.
#[test]
fn 이미_깨진_코드에서_더_깨지_않는_제안은_경고하지_않는다() {
    rt().block_on(async {
        let edit = call(
            "propose_edit",
            json!({"start_line": 2, "end_line": 2, "text": "    return (n + 1"}),
        );
        let model = LocalModel::new(Scripted::new(&[&edit, "고쳤어요."]));
        let host = Host::new();
        *host.code.borrow_mut() = "def add_one(n):\n    return (n\n".into();
        let mut chat = Chat::new();
        turn(&model, &mut chat, &host, "고쳐").await.0.unwrap();
        assert_eq!(chat.pending().unwrap().warning, None);
        assert_eq!(model_prompts(&model).len(), 2);
    });
}

/// #125 (d): 모델이 보는 글(system·도구 정의·결과·알림)에 내부 용어 candidate가 없고, 한국어 답 지시가 있다.
#[test]
fn 모델이_보는_글에_내부_용어가_없고_한국어_답을_시킨다() {
    rt().block_on(async {
        let model = LocalModel::new(Scripted::new(&[
            &call(
                "propose_edit",
                json!({"start_line": 2, "end_line": 2, "text": "    return n + 1"}),
            ),
            "고쳤어요.",
            "끝",
        ]));
        let host = Host::new();
        let mut chat = Chat::new();
        turn(&model, &mut chat, &host, "고쳐").await.0.unwrap();
        chat.decide(1, false, CODE).unwrap();
        turn(&model, &mut chat, &host, "왜?").await.0.unwrap();
        let prompts = model_prompts(&model);
        let last = prompts.last().unwrap();
        assert!(last.contains("[알림: 사용자가 편집 제안 1을 거절했습니다. 코드는 그대로입니다.]"));
        for p in &prompts {
            assert!(!p.to_lowercase().contains("candidate"), "{p}");
            assert!(p.contains("항상 한국어로"));
        }
    });
}

/// #131 루프 2(Windows 1.5B): 도구 결과 뒤 빈 답(끝 토큰만)이면 한 번 다시 묻고, 그래도 비면 조용히 끝나지 않고
/// 「답을 만들지 못했다」 줄을 남긴다. 차례 표지 글자(`<|im_start|>`)는 답으로 보이지 않는다.
#[test]
fn 빈_답은_한_번_다시_묻고_그래도_비면_안내를_남긴다() {
    rt().block_on(async {
        let host = Host::new();
        let model = LocalModel::new(Scripted::new(&[
            &call("read_code", json!({})),
            "",
            "고쳤습니다.",
        ]));
        let mut chat = Chat::new();
        turn(&model, &mut chat, &host, "고쳐 줘").await.0.unwrap();
        let prompts = model_prompts(&model);
        assert_eq!(prompts.len(), 3);
        assert!(prompts[2].contains(promptlab_shared::ai::EMPTY_REPLY_NOTE));
        assert_eq!(
            chat.entries().last(),
            Some(&Entry::Assistant("고쳤습니다.".into()))
        );
        assert!(!chat.entries().contains(&Entry::Empty));

        // 다시 물어도 비면(차례 표지만 내도 빈 답이다) 안내 한 줄.
        let model = LocalModel::new(Scripted::new(&[
            &call("read_code", json!({})),
            "",
            "<|im_start|>",
        ]));
        let mut chat = Chat::new();
        turn(&model, &mut chat, &host, "고쳐 줘").await.0.unwrap();
        assert_eq!(model_prompts(&model).len(), 3);
        assert_eq!(chat.entries().last(), Some(&Entry::Empty));
        assert!(
            chat.entries()
                .iter()
                .all(|e| !matches!(e, Entry::Assistant(t) if t.contains("<|")))
        );
    });
}

/// #131 루프 2: 같은 차례에 같은 읽기를 또 부르면 실행하지 않고 「이미 읽었다 — 답하라」를 돌려준다.
#[test]
fn 같은_차례의_되풀이_읽기는_실행하지_않는다() {
    rt().block_on(async {
        let host = Host::new();
        let read = call("read_code", json!({}));
        let model = LocalModel::new(Scripted::new(&[&read, &read, "고칠 곳이 없습니다."]));
        let mut chat = Chat::new();
        turn(&model, &mut chat, &host, "봐 줘").await.0.unwrap();
        let prompts = model_prompts(&model);
        assert_eq!(prompts.len(), 3);
        assert!(prompts[2].contains(promptlab_shared::ai::REPEATED_READ_NOTE));
        let tools = chat
            .entries()
            .iter()
            .filter(|e| matches!(e, Entry::Tool(_)))
            .count();
        assert_eq!(tools, 1, "{:?}", chat.entries());
        assert_eq!(
            chat.entries().last(),
            Some(&Entry::Assistant("고칠 곳이 없습니다.".into()))
        );
    });
}
