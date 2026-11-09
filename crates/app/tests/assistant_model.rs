//! 실제 모델 끝에서 끝(#98): 문제 읽기 → 코드 읽기 → 편집 제안 → 수락. 모델 파일이 있어야 해서 `#[ignore]`.
//! 후보 모델(#131)마다 돌린다 — `PROMPTLAB_TEST_MODEL`(기본 `7b`)로 고른다:
//!
//! ```sh
//! bash scripts/fetch-model.sh --model 1.5b
//! PROMPTLAB_TEST_MODEL=1.5b cargo test --release -p promptlab-app --test assistant_model -- --ignored --nocapture
//! ```
//!
//! 기대는 모델마다 낮추지 않는다(제품 기준). CPU 측정(#131 루프 2, 2026-11-09): 7B 5/5 통과, **1.5B 3/5** — 고장 시험
//! (조용히 끝남·특수 토큰 노출)·JS 머리 보존·읽고 제안은 통과, 상태 블록 시험은 영어로 「읽어야 한다」만 답하고, 빈 틀 시험은
//! 제안이 코드를 깨뜨려 되돌린 뒤 `<|im_start|>`만 내 「답을 만들지 못했습니다」 줄로 끝난다(품질). 1.5B는 GPU가 모자란
//! PC의 대안이라 결과에 「쓴 모델」을 싣고 판단은 고객사에 맡긴다.
//!
//! 토큰/초(프롬프트 처리·생성)를 찍는다 — PR #98 본문의 숫자가 여기서 나왔다.

use std::time::Instant;

use promptlab_app::assistant::{Chat, Entry, Step, ToolHost, ask};
use promptlab_app::local::{Cancel, LocalModel};
use promptlab_inference::{LlamaInference, compute_info};
use promptlab_shared::ai::apply_range;
use promptlab_shared::coding::{
    DEFAULT_CASE_TIME_MS, Language, Limits, Param, PublicCodingProblem, Signature, TestCase,
    ValueType, templates,
};
use promptlab_shared::log::LogEvent;
use promptlab_shared::syntax;
use serde_json::json;

/// 시험할 후보 모델의 캐시 경로(`PROMPTLAB_TEST_MODEL`, 기본 가장 큰 후보).
fn default_model_path() -> std::path::PathBuf {
    let id = std::env::var("PROMPTLAB_TEST_MODEL")
        .unwrap_or_else(|_| promptlab_shared::inference::LARGEST_MODEL.id.to_owned());
    let model = promptlab_shared::inference::model_by_id(&id)
        .unwrap_or_else(|| panic!("PROMPTLAB_TEST_MODEL={id}은 후보가 아니다"));
    promptlab_inference::model_path(model)
}

const CODE: &str = "def add_one(n):\n    return n\n";

struct Host;

impl ToolHost for Host {
    fn problem(&self) -> Option<PublicCodingProblem> {
        let signature = Signature {
            name: "add_one".into(),
            params: vec![Param {
                name: "n".into(),
                ty: ValueType::Int,
            }],
            returns: ValueType::Int,
        };
        Some(PublicCodingProblem {
            id: "add-one".into(),
            title: "Add one".into(),
            description: "Return n + 1.".into(),
            templates: templates(&signature),
            signature,
            examples: vec![TestCase {
                args: vec![json!(1)],
                expected: json!(2),
            }],
            limits: Limits {
                case_time_ms: DEFAULT_CASE_TIME_MS,
            },
        })
    }
    fn code(&self) -> (Language, String) {
        (Language::Python, CODE.into())
    }
    async fn run_examples(&self) -> String {
        unreachable!("꺼져 있다")
    }
}

#[test]
#[ignore = "모델 파일이 필요하다 — scripts/fetch-model.sh 뒤 --ignored 로 켠다"]
fn 실제_모델이_문제와_코드를_읽고_편집을_제안하면_수락된다() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        let started = Instant::now();
        let mut model = LocalModel::<LlamaInference>::load(default_model_path())
            .await
            .unwrap();
        let perf = std::env::temp_dir().join("promptlab-assistant-model-perf.log");
        let _ = std::fs::remove_file(&perf);
        model.set_perf_log(Some(perf.clone()));
        let loaded = started.elapsed();
        let mut chat = Chat::new();
        let mut first_piece = None;
        let mut events = Vec::new();
        let mut calls = Vec::new();
        let t0 = Instant::now();
        ask(
            &model,
            &mut chat,
            &Host,
            "My solution fails the examples. Read the problem and my code, then propose a fix with propose_edit.",
            &Cancel::new(),
            &mut |s| {
                if let Step::Changed(c) = &s
                    && first_piece.is_none()
                    && c.live().is_some_and(|l| l.tokens > 0)
                {
                    first_piece = Some(t0.elapsed());
                }
                if let Step::Event(e) = s {
                    let now = t0.elapsed();
                    if let LogEvent::AiReply { text, tool_calls, .. } = &e {
                        eprintln!("[{now:.1?}] 답 {text:?} 호출 {tool_calls:?}");
                        calls.extend(tool_calls.iter().map(|c| c.name.clone()));
                    }
                    events.push(e);
                }
            },
        )
        .await
        .unwrap();
        let took = t0.elapsed();
        let (input, output) = model.token_totals().unwrap();
        eprintln!(
            "장치 {:?}, 로드(sha256 포함) {loaded:.1?}, 차례 {took:.1?}, 입력 {input}토큰·출력 {output}토큰, 전체 {:.1}토큰/초",
            compute_info().unwrap(),
            (input + output) as f64 / took.as_secs_f64()
        );
        eprintln!("첫 조각 {first_piece:.1?}");
        eprintln!("{}", std::fs::read_to_string(&perf).unwrap_or_default());
        for e in chat.entries() {
            eprintln!("  {e:?}");
        }
        assert!(calls.iter().any(|c| c == "propose_edit"), "{calls:?}");
        let p = chat.pending().expect("편집 제안").clone();
        eprintln!("제안 #{} {:?} {:?}\n→\n{}", p.id, p.range, p.text, p.after);
        assert!(
            p.after.contains("n + 1") || p.after.contains("n+1"),
            "{}",
            p.after
        );
        let d = chat.decide(p.id, true, CODE).unwrap();
        let (range, text) = d.apply.unwrap();
        assert_eq!(apply_range(CODE, &range, &text).unwrap(), p.after);
        assert!(chat.entries().contains(&Entry::Proposal(p.id)));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, LogEvent::EditProposed { .. }))
        );
    });
}

/// 상태 블록(#116): 도구를 부르지 않아도 모델이 고른 언어와 지금 코드를 안다. 멈추면 곧 멈춘다.
#[test]
#[ignore = "모델 파일이 필요하다 — scripts/fetch-model.sh 뒤 --ignored 로 켠다"]
fn 실제_모델은_상태_블록으로_언어와_코드를_알고_멈추면_곧_멈춘다() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut model = LocalModel::<LlamaInference>::load(default_model_path())
            .await
            .unwrap();
        let perf = std::env::temp_dir().join("promptlab-assistant-model-perf2.log");
        let _ = std::fs::remove_file(&perf);
        model.set_perf_log(Some(perf.clone()));
        let mut chat = Chat::new();
        let mut events = Vec::new();
        ask(
            &model,
            &mut chat,
            &Host,
            "Without calling any tool: which programming language am I using, and what does line 2 of my code return? Answer in one sentence.",
            &Cancel::new(),
            &mut |s| {
                if let Step::Event(e) = s {
                    events.push(e);
                }
            },
        )
        .await
        .unwrap();
        let replies: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                LogEvent::AiReply { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        eprintln!("답 {replies:?}");
        // #125 루프 2: 이 시험에서 7B가 「Python 2行은…」을 냈다 — 보이는 답에는 `hangul_lines`가 「2줄」로 바꾼다.
        assert!(replies.iter().all(|t| !t.contains('行')), "{replies:?}");
        let last = replies.last().cloned().unwrap_or_default().to_lowercase();
        // #125부터 답은 한국어라 「파이썬」으로 쓸 수 있다.
        assert!(last.contains("python") || last.contains("파이썬"), "{replies:?}");
        assert!(last.contains("return n") || last.contains("`n`") || last.contains(" n"), "{replies:?}");

        // 멈추기: 토큰 5개가 보이면 누른다 → stopped 답, 곧 끝난다.
        let cancel = Cancel::new();
        let t0 = Instant::now();
        let mut stopped = None;
        ask(
            &model,
            &mut chat,
            &Host,
            "Explain in detail, step by step, how to test this function. Write at least 300 words.",
            &cancel,
            &mut |s| match s {
                Step::Changed(c) => {
                    if c.live().is_some_and(|l| l.tokens >= 5) {
                        cancel.cancel();
                    }
                }
                Step::Event(LogEvent::AiReply { text, stopped: true, .. }) => {
                    stopped = Some(text);
                }
                Step::Event(_) => {}
            },
        )
        .await
        .unwrap();
        eprintln!("멈춤 {:.1?} 답 {stopped:?}", t0.elapsed());
        assert!(stopped.is_some(), "멈춘 답이 로그에 있어야 한다");
        assert_eq!(chat.entries().last(), Some(&Entry::Stopped));
        eprintln!("{}", std::fs::read_to_string(&perf).unwrap_or_default());
    });
}

/// #125: 두 수 합 JS 코드(안쪽 루프가 `j = i`부터라 같은 원소를 두 번 쓴다).
const JS_CODE: &str = "/**\n * @param {number[]} nums\n * @param {number} target\n * @return {number[]}\n */\nfunction two_sum(nums, target) {\n    for (let i = 0; i < nums.length; i++) {\n        for (let j = i; j < nums.length; j++) {\n            if (nums[i] + nums[j] === target) return [i, j];\n        }\n    }\n    return [];\n}\n";

/// 두 수 합 문제의 응시 화면(언어·코드 바꿈).
struct JsHost(std::cell::RefCell<String>, Language);

impl ToolHost for JsHost {
    fn problem(&self) -> Option<PublicCodingProblem> {
        let signature = Signature {
            name: "two_sum".into(),
            params: vec![
                Param {
                    name: "nums".into(),
                    ty: ValueType::IntArray,
                },
                Param {
                    name: "target".into(),
                    ty: ValueType::Int,
                },
            ],
            returns: ValueType::IntArray,
        };
        Some(PublicCodingProblem {
            id: "two-sum".into(),
            title: "두 수의 합".into(),
            description: "정수 배열 nums와 target이 주어진다. 합이 target인 서로 다른 두 원소의 인덱스 [i, j](i < j)를 돌려준다. 답은 하나뿐이다.".into(),
            templates: templates(&signature),
            signature,
            examples: vec![
                TestCase {
                    args: vec![json!([2, 7, 11, 15]), json!(9)],
                    expected: json!([0, 1]),
                },
                TestCase {
                    args: vec![json!([3, 3]), json!(6)],
                    expected: json!([0, 1]),
                },
                TestCase {
                    args: vec![json!([3, 2, 4]), json!(6)],
                    expected: json!([1, 2]),
                },
            ],
            limits: Limits {
                case_time_ms: DEFAULT_CASE_TIME_MS,
            },
        })
    }
    fn code(&self) -> (Language, String) {
        (self.1, self.0.borrow().clone())
    }
    async fn run_examples(&self) -> String {
        unreachable!("꺼져 있다")
    }
}

/// #125 (c)·(d): JS 「버그 고쳐 줘」 → 제안은 함수 머리·닫는 괄호를 지키고(적용 뒤 구문 점검 통과), 답은 한국어에
/// 내부 용어(candidate)가 없다. 수락 알림 뒤 다음 차례 답도 한국어다.
#[test]
#[ignore = "모델 파일이 필요하다 — scripts/fetch-model.sh 뒤 --ignored 로 켠다"]
fn 실제_모델의_js_버그_수정_제안은_함수_머리를_지키고_한국어로_답한다() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        let model = LocalModel::<LlamaInference>::load(default_model_path())
            .await
            .unwrap();
        let host = JsHost(
            std::cell::RefCell::new(JS_CODE.to_owned()),
            Language::Javascript,
        );
        let mut chat = Chat::new();
        let mut replies = Vec::new();
        let t0 = Instant::now();
        ask(
            &model,
            &mut chat,
            &host,
            "예제 [3, 3], 6에서 [0, 0]이 나와. 버그 고쳐 줘",
            &Cancel::new(),
            &mut |s| {
                if let Step::Event(LogEvent::AiReply {
                    text, tool_calls, ..
                }) = s
                {
                    eprintln!("[{:.1?}] 답 {text:?} 호출 {tool_calls:?}", t0.elapsed());
                    replies.push(text);
                }
            },
        )
        .await
        .unwrap();
        for e in chat.entries() {
            eprintln!("  {e:?}");
        }
        let p = chat.pending().expect("편집 제안").clone();
        eprintln!("제안 #{} {:?}\n{}\n→\n{}", p.id, p.lines, p.text, p.after);
        report_syntax(&p.after);
        let shown: Vec<&String> = replies.iter().filter(|t| !t.trim().is_empty()).collect();
        eprintln!("보인 답 {shown:?}");
        let d = chat.decide(p.id, true, JS_CODE).unwrap();
        let (range, text) = d.apply.unwrap();
        *host.0.borrow_mut() = apply_range(JS_CODE, &range, &text).unwrap();
        let mut after = Vec::new();
        ask(
            &model,
            &mut chat,
            &host,
            "고마워. 이제 맞아?",
            &Cancel::new(),
            &mut |s| {
                if let Step::Event(LogEvent::AiReply { text, .. }) = s {
                    eprintln!("[{:.1?}] 다음 답 {text:?}", t0.elapsed());
                    after.push(text);
                }
            },
        )
        .await
        .unwrap();
        check_answers(&p.after, &shown, &after);
    });
}

fn has_hangul(s: &str) -> bool {
    s.chars().any(|c| ('가'..='힣').contains(&c))
}

fn report_syntax(after: &str) {
    eprintln!(
        "구문 {:?}, 머리 {}",
        syntax::check(Language::Javascript, after),
        after.contains("function two_sum(")
    );
}

/// 적용 뒤 코드가 함수 머리를 지키고 깨지지 않았고, 보인 답이 모두 한국어이며 내부 용어가 없다.
fn check_answers(after: &str, shown: &[&String], next: &[String]) {
    assert!(
        after.contains("function two_sum("),
        "함수 머리가 지워졌다:\n{after}"
    );
    assert_eq!(
        syntax::regression(Language::Javascript, Some("two_sum"), JS_CODE, after),
        None,
        "{after}"
    );
    let next: Vec<&String> = next.iter().filter(|t| !t.trim().is_empty()).collect();
    assert!(!next.is_empty(), "다음 차례 답이 없다");
    for t in shown.iter().chain(next.iter()) {
        assert!(has_hangul(t), "한국어가 아닌 답: {t:?}");
        assert!(!t.to_lowercase().contains("candidate"), "내부 용어: {t:?}");
        assert!(!has_hanja(t), "한자: {t:?}");
    }
}

/// 한자(CJK 통합 한자)가 있나 — 7B가 「6-9行」처럼 섞었다(#125 루프 2).
fn has_hanja(s: &str) -> bool {
    s.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c))
}

/// #125 루프 2(Windows 재현): 빈 틀 그대로에서 「예제 테스트가 틀려. 버그 고쳐 줘」 → 띄운 제안을 적용한 코드에 풀이 함수
/// 머리가 남고 깨지지 않는다(`regression` 서명 포함), 보인 답은 한국어이고 한자·candidate가 없다.
async fn empty_template_case(model: &LocalModel<LlamaInference>, language: Language) {
    let problem = JsHost(std::cell::RefCell::new(String::new()), language)
        .problem()
        .unwrap();
    let template = problem
        .templates
        .iter()
        .find(|t| t.language == language)
        .unwrap()
        .code
        .clone();
    let host = JsHost(std::cell::RefCell::new(template.clone()), language);
    let mut chat = Chat::new();
    let mut shown = Vec::new();
    let t0 = Instant::now();
    ask(
        model,
        &mut chat,
        &host,
        "예제 테스트가 틀려. 버그 고쳐 줘",
        &Cancel::new(),
        &mut |s| {
            if let Step::Event(LogEvent::AiReply {
                text, tool_calls, ..
            }) = s
            {
                eprintln!(
                    "[{language:?} {:.1?}] 답 {text:?} 호출 {tool_calls:?}",
                    t0.elapsed()
                );
                if !text.trim().is_empty() {
                    shown.push(text);
                }
            }
        },
    )
    .await
    .unwrap();
    for e in chat.entries() {
        eprintln!("  {e:?}");
    }
    let p = chat.pending().expect("편집 제안").clone();
    eprintln!(
        "제안 #{} {:?} 경고 {:?}\n{}\n→\n{}",
        p.id, p.lines, p.warning, p.text, p.after
    );
    let head = format!(
        "{}two_sum(",
        if language == Language::Python {
            "def "
        } else {
            "function "
        }
    );
    assert!(
        p.after.contains(&head),
        "함수 머리가 사라졌다:\n{}",
        p.after
    );
    assert_eq!(
        syntax::regression(language, Some("two_sum"), &template, &p.after),
        None,
        "{}",
        p.after
    );
    assert_eq!(p.warning, None);
    assert!(!shown.is_empty());
    for t in &shown {
        assert!(has_hangul(t), "한국어가 아닌 답: {t:?}");
        assert!(!has_hanja(t), "한자: {t:?}");
        assert!(!t.to_lowercase().contains("candidate"), "내부 용어: {t:?}");
    }
}

#[test]
#[ignore = "모델 파일이 필요하다 — scripts/fetch-model.sh 뒤 --ignored 로 켠다"]
fn 실제_모델의_빈_틀_버그_수정_제안은_함수_머리를_지킨다() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        let model = LocalModel::<LlamaInference>::load(default_model_path())
            .await
            .unwrap();
        empty_template_case(&model, Language::Javascript).await;
        empty_template_case(&model, Language::Python).await;
    });
}

/// 모델 원문을 stderr에 찍는 래퍼(#131 루프 2 진단 — 1.5B가 Windows에서 조용히 끝나거나 `<|im_start|>`를 보였다).
struct Raw(LlamaInference);

impl promptlab_shared::inference::Inference for Raw {
    fn complete(
        &self,
        prefix: &str,
        suffixes: &[&str],
        params: &promptlab_shared::inference::SamplingParams,
    ) -> Result<Vec<String>, promptlab_shared::inference::InferenceError> {
        let out = self.0.complete(prefix, suffixes, params)?;
        eprintln!("    [원문 complete] {out:?}");
        Ok(out)
    }
    fn count_tokens(
        &self,
        text: &str,
    ) -> Result<usize, promptlab_shared::inference::InferenceError> {
        self.0.count_tokens(text)
    }
    fn context_tokens(&self) -> u32 {
        self.0.context_tokens()
    }
    fn token_totals(&self) -> Option<(u64, u64)> {
        self.0.token_totals()
    }
    fn stream(
        &self,
        prefix: &str,
        suffix: &str,
        params: &promptlab_shared::inference::SamplingParams,
        on_text: &mut dyn FnMut(&str, usize) -> bool,
    ) -> Result<promptlab_shared::inference::Generation, promptlab_shared::inference::InferenceError>
    {
        let g = self.0.stream(prefix, suffix, params, on_text)?;
        let tail: String = suffix
            .chars()
            .rev()
            .take(160)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if std::env::var("RAW_FULL").is_ok() {
            let p: String = prefix
                .chars()
                .rev()
                .take(2500)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            eprintln!("    [prefix 끝] {p}\n    [suffix] {suffix}");
        }
        eprintln!("    [suffix 끝] {tail:?}\n    [원문] {:?}", g.text);
        Ok(g)
    }
}

/// #131 루프 2(Windows 1.5B 고장): 빈 JS 틀에서 「예제 테스트가 틀려. 버그 고쳐 줘」 → 「two_sum 함수를 구현해 줘」.
/// 품질이 아니라 **고장**만 본다: 차례마다 보이는 답·제안·「답을 만들지 못했다」 안내 중 하나가 남고(조용히 끝나지
/// 않는다), 특수 토큰 글자(`<|…|>`)가 답에 나오지 않는다.
#[test]
#[ignore = "모델 파일이 필요하다 — scripts/fetch-model.sh 뒤 --ignored 로 켠다"]
fn 실제_모델은_조용히_끝나거나_특수_토큰을_보이지_않는다() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        let path = default_model_path();
        let inner = LlamaInference::load_with(&path, true, None).unwrap();
        let model = LocalModel::new(Raw(inner));
        let language = Language::Javascript;
        let problem = JsHost(std::cell::RefCell::new(String::new()), language)
            .problem()
            .unwrap();
        let template = problem
            .templates
            .iter()
            .find(|t| t.language == language)
            .unwrap()
            .code
            .clone();
        let host = JsHost(std::cell::RefCell::new(template), language);
        let mut chat = Chat::new();
        for question in [
            "예제 테스트가 틀려. 버그 고쳐 줘",
            "two_sum 함수를 구현해 줘",
        ] {
            if let Some(id) = chat.pending().map(|p| p.id) {
                let code = host.0.borrow().clone();
                chat.decide(id, false, &code).unwrap();
            }
            let before = chat.entries().len();
            let mut replies = Vec::new();
            let t0 = Instant::now();
            ask(
                &model,
                &mut chat,
                &host,
                question,
                &Cancel::new(),
                &mut |s| {
                    if let Step::Event(LogEvent::AiReply {
                        text, tool_calls, ..
                    }) = s
                    {
                        eprintln!("[{:.1?}] 답 {text:?} 호출 {tool_calls:?}", t0.elapsed());
                        replies.push(text);
                    }
                },
            )
            .await
            .unwrap();
            let entries: Vec<&Entry> = chat.entries()[before..].iter().collect();
            for e in &entries {
                eprintln!("  {e:?}");
            }
            for t in &replies {
                assert!(!t.contains("<|"), "특수 토큰이 보인다: {t:?}");
            }
            // 답·제안이 없으면 적어도 「답을 만들지 못했다」 안내(Entry::Empty)가 남아야 한다(조용히 끝나지 않는다).
            let visible = replies.iter().any(|t| !t.trim().is_empty());
            let told = entries.iter().any(|e| matches!(e, Entry::Empty));
            if told {
                eprintln!("  → 안내 줄로 끝남(품질 실패, 고장 아님)");
            }
            assert!(
                visible || chat.pending().is_some() || told,
                "{question}: 답도 제안도 없이 끝났다 {entries:?}"
            );
        }
    });
}
