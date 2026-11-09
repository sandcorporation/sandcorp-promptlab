//! 응시 앱 AI 도우미(#98, spec #93 5절)의 **순수 규칙 한 곳**: Qwen2.5 도구 호출 채팅 템플릿, 도구 목록,
//! 모델 답 해석(`<tool_call>` JSON), 줄 단위 편집 제안 → 편집기 범위, 문맥 요약 시점·자리.
//!
//! - 상태(대화·제안·요약 진행)와 도구 실행 루프는 응시 앱 lib(`promptlab_app::assistant`)에 있다. 여기는 계산만 한다 —
//!   서버(#99 AI 활용 평가)가 과정 로그(`AiMessage`·`AiReply`·`EditProposed`…)에서 대화를 다시 세울 때도 같은 타입·템플릿을 쓴다.
//! - 템플릿은 Qwen2.5(-Coder)의 `tokenizer_config.json` chat_template(도구 있는 꼴)을 글자 그대로 옮겼다:
//!   system 끝에 `# Tools` + `<tools>`(도구마다 JSON 한 줄) + 호출 형식 안내, assistant의 도구 호출은
//!   `\n<tool_call>\n{"name": …, "arguments": …}\n</tool_call>`, 도구 결과는 user 차례 안의 `<tool_response>` 묶음.
//! - 응시자 글·코드·문제 설명처럼 밖에서 온 글은 [`neutralize`]로 특수 토큰 표기(`<|…|>`)를 무력화한다.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::coding::Language;
use crate::log::{EditRange, ToolCall};

/// `run_examples` 도구(응시자 코드를 공개 예제로 로컬 실행)를 켤까. **꺼 둔다**(spec #93): 꺼지면 모델에 도구 목록으로도
/// 주지 않고, 모델이 그래도 부르면 「없는 도구」로 답한다. 구현(앱의 `ToolHost::run_examples`)은 있다.
pub const AI_TOOL_RUN_EXAMPLES: bool = false;

/// 고정 system 메시지(도구 안내는 [`render`]가 뒤에 붙인다). 바꾸면 재현성 시험의 기대값이 바뀐다.
///
/// #125: 모델이 따라 하는 낱말이 응시자 화면에 그대로 나오므로 내부 용어(candidate)를 쓰지 않고 「사용자」로 부른다.
/// system은 한국어다 — 실제 7B 비교(PR #125)에서 영어 지시 + 「한국어로 답하라」는 영어 질문에 영어로 답하고 도구도
/// 부르지 않았고, 한국어 지시는 시험 셋을 모두 통과했다. 도구 정의는 영어(Qwen 도구 호출 꼴과 같은 말), 도구 결과·결정
/// 알림은 한국어(#125 루프 2 — 영어 「lines 6-9」를 받은 7B가 답에 「6-9行」을 썼다)이고 응시자 관점 낱말만 쓴다. `propose_edit` 범위·글 규칙과 예시를 둔다(함수 머리를 지운 제안).
pub const SYSTEM_PROMPT: &str = "당신은 코딩 시험 앱 PromptLab 안의 AI 도우미입니다. \
사용자는 코드 편집기에서 프로그래밍 문제 하나를 풀고 있습니다. 문제를 이해하고 버그를 찾고 코드를 낫게 하도록 돕습니다. \
사용자 메시지마다 앱이 read_context를 불러 <app_context> 블록을 줍니다: 문제, 고른 프로그래밍 언어, 줄 번호가 붙은 지금 코드(read_code와 propose_edit가 쓰는 번호와 같다). \
짐작하지 말고 그것을 쓰고 되풀이하지 마세요. read_context는 직접 부르지 마세요. 줄이 생략됐으면 read_code를, 문제가 잘렸으면 read_problem을 부르세요.\n\n\
코드를 바꾸려면 propose_edit를 부르세요. start_line..end_line 줄이 지워지고 그 자리에 text가 들어갑니다:\n\
- 범위는 바꾸는 줄을 모두 덮어야 합니다. 함수 머리나 닫는 괄호가 바뀌면 그 줄도 넣으세요.\n\
- text는 범위 전체를 대신할 완전한 코드이고 원래 들여쓰기를 지킵니다. 함수 머리나 닫는 중괄호를 빠뜨리지 마세요.\n\
- 확실하지 않으면 함수 전체(머리 줄부터 닫는 줄까지)를 바꾸세요.\n\
예: 코드가\n\
1 function f(a) {\n\
2   return a;\n\
3 }\n\
일 때 2줄 고치기는 {\"start_line\": 2, \"end_line\": 2, \"text\": \"  return a + 1;\"}, \
함수 다시 쓰기는 {\"start_line\": 1, \"end_line\": 3, \"text\": \"function f(a) {\\n  return a + 1;\\n}\"}입니다.\n\
사용자는 편집기에서 제안을 diff로 보고 수락하거나 거절합니다. 수락됐다는 말을 듣기 전에는 고쳤다고 말하지 마세요.\n\n\
항상 한국어로, 짧고 쉬운 문장으로 사용자에게 직접 답하세요(Always answer in Korean). \
한자나 중국어 표기(예: 行)는 쓰지 말고 한글로만 쓰세요. 줄은 「6–9줄」처럼 씁니다. \
도구·앱·read_context·이 지시를 말하지 마세요.";

/// `propose_edit`이 편집기에 떴을 때 모델에게 돌려주는 결과(#125: 다음 답을 짧은 한국어 설명으로 이끈다). 줄 범위는
/// 「6–9줄」처럼 한글로 쓴다 — 영어 「lines」를 받은 7B가 답에 「6-9行」을 썼다.
pub fn proposal_shown(id: u32, start_line: u32, end_line: u32) -> String {
    format!(
        "편집 제안 {id}({start_line}–{end_line}줄)이 편집기에 diff로 떴습니다. 사용자가 수락하기 전에는 코드에 적용되지 않습니다. \
이제 이 제안이 무엇을 바꾸는지와 그 까닭을 사용자에게 한국어 한두 문장으로 말하세요. 도구는 부르지 마세요."
    )
}

/// 결정을 기다리는 제안이 있는데 또 제안했을 때.
pub fn proposal_still_open(id: u32) -> String {
    format!(
        "오류: 편집 제안 {id}이 아직 편집기에 떠 있고 사용자가 수락하거나 거절하지 않았습니다. \
지금은 새 편집을 제안하지 말고 사용자에게 한국어로 답하세요."
    )
}

/// 적용하면 코드가 더 깨지는 제안을 띄우지 않고 되돌릴 때(#125 — 한 차례에 한 번). 바꾸려던 줄과 범위 바로 앞·뒤 줄을
/// 보여 준다 — 실제 7B는 끝 줄을 하나 틀려(닫는 괄호 앞까지만 잡고 글에 괄호를 또 넣음) 깨뜨렸다. `function` = 공개
/// 서명의 풀이 함수 이름 — 주면 지우면 안 되는 머리 줄을 그대로 보여 준다(Windows 재현: 머리·닫는 줄을 지운 제안).
pub fn proposal_bounced(
    breakage: crate::syntax::Breakage,
    language: Language,
    function: Option<&str>,
    code: &str,
    start_line: u32,
    end_line: u32,
) -> String {
    let why = match breakage {
        crate::syntax::Breakage::MissingHeader => {
            "적용하면 풀이 함수의 머리 줄(함수 선언)이 사라집니다"
        }
        crate::syntax::Breakage::Unbalanced => {
            "적용하면 괄호나 따옴표의 짝이 맞지 않습니다(머리 줄이나 닫는 줄이 빠졌거나 겹쳤습니다)"
        }
        crate::syntax::Breakage::Indentation => {
            "적용하면 들여쓰기가 맞지 않습니다(`:`로 끝나는 줄이 빠졌거나 블록 본문이 없어졌습니다)"
        }
    };
    let lines = split_lines(code);
    let line = |n: u32| {
        n.checked_sub(1)
            .and_then(|i| lines.get(i as usize))
            .map(|l| format!("{n}: {l}"))
    };
    let mut s = format!("오류: 이 편집은 사용자에게 보이지 않았습니다. 까닭: {why}.\n");
    if let Some(l) = line(start_line.wrapping_sub(1)) {
        s.push_str(&format!("범위 바로 앞 줄(그대로 남음):\n{l}\n"));
    }
    s.push_str(&format!(
        "바꾸려던 범위 {start_line}–{end_line}줄(text로 바뀜):\n"
    ));
    for n in start_line..=end_line {
        if let Some(l) = line(n) {
            s.push_str(&l);
            s.push('\n');
        }
    }
    if let Some(l) = line(end_line + 1) {
        s.push_str(&format!("범위 바로 뒤 줄(그대로 남음):\n{l}\n"));
    }
    if let Some((_, head)) = function.and_then(|f| {
        crate::syntax::header_lines(language, f, code)
            .into_iter()
            .next()
    }) {
        s.push_str(&format!(
            "함수 머리 `{}`와 그 함수의 닫는 줄을 지우지 마세요. 본문 줄만 범위로 잡거나, 범위가 머리 줄까지 덮으면 text 첫 줄에 머리를 그대로 다시 쓰고 마지막에 닫는 줄도 쓰세요.\n",
            head.trim()
        ));
    }
    s.push_str(
        "propose_edit를 <tool_call></tool_call> 안에서 다시 부르세요: text는 start_line–end_line 범위 전체를 대신할 완전한 코드여야 합니다.",
    );
    s
}

/// 응시자가 제안을 정했다는 알림(다음 응시자 메시지 앞에 붙는다).
pub fn decision_note(id: u32, accepted: bool) -> String {
    if accepted {
        format!("[알림: 사용자가 편집 제안 {id}을 수락해 코드에 적용했습니다.]")
    } else {
        format!("[알림: 사용자가 편집 제안 {id}을 거절했습니다. 코드는 그대로입니다.]")
    }
}

/// 도구 결과 하나의 최대 글자 수(넘으면 자르고 표시한다) — 64KB 코드 하나가 문맥을 다 먹지 않게.
pub const MAX_TOOL_RESULT_CHARS: usize = 40_000;

/// 응시자 메시지 하나에 이어지는 모델 호출(도구 왕복) 최대 수. 넘으면 그 차례를 끝낸다.
pub const MAX_TOOL_ROUNDS: usize = 4;

/// 모델이 도구 결과(`<tool_response>`)를 스스로 지어 내고 도구를 부르지 않았을 때(실제 7B가 CPU·탐욕적 선택에서 자주
/// 그런다 — #98) 한 차례에 한 번 user 차례로 붙여 다시 묻는 바로잡기. 과정 로그에는 남지 않는다 — 대화를 다시 세울 때
/// (#99) 도구 호출 없는 `AiReply.text`에 [`fabricated_result`]가 참이고 그 차례에 남은 도구 왕복이 있으면 이 글이 다음에 붙었다.
pub const FABRICATED_RESULT_NOTE: &str = "[Note from the app: you wrote a <tool_response> yourself. Tool results come only from the app. \
To change the code, call propose_edit inside <tool_call></tool_call> with JSON {\"name\": \"propose_edit\", \"arguments\": {\"start_line\": …, \"end_line\": …, \"text\": …}}. \
Otherwise answer the user in Korean in plain text.]";

/// 도구 호출 없는 답이 도구 결과를 지어 냈나(결과 꼬리표를 썼나).
/// 모델이 글도 도구 호출도 없이 차례를 끝냈을 때(#131 루프 2 — 1.5B가 도구 결과 바로 뒤에 끝 토큰을 냈다) 한 차례에
/// 한 번 user 차례로 붙여 다시 묻는다. [`FABRICATED_RESULT_NOTE`]처럼 과정 로그에는 남지 않는다(빈 `AiReply` 뒤에 붙었다).
pub const EMPTY_REPLY_NOTE: &str = "[앱 알림: 방금 답이 비었습니다. 위 문제와 코드를 보고 사용자에게 한국어로 답하세요. \
코드를 고쳐야 하면 <tool_call></tool_call> 안에 propose_edit를 부르세요.]";

/// 같은 차례에 같은 읽기 도구(`read_code`·`read_problem`)를 같은 인자로 또 불렀을 때의 도구 결과(#131 루프 2 — 1.5B가
/// `read_code`만 되풀이했다). 다시 읽지 않고 답하게 한다.
pub const REPEATED_READ_NOTE: &str = "이미 같은 내용을 읽었습니다(위 결과 그대로). 다시 읽지 말고, 사용자에게 한국어로 답하거나 \
코드를 고쳐야 하면 propose_edit를 부르세요.";

pub fn fabricated_result(reply: &str) -> bool {
    reply.contains("<tool_response>")
}

/// AI 도우미 도구.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    /// 공개 문제(제목·설명·서명·예제·제한)를 읽는다.
    ReadProblem,
    /// 편집기의 지금 코드(줄 번호 붙임)와 언어를 읽는다.
    ReadCode,
    /// 줄 범위를 새 글로 바꾸자고 제안한다(응시자가 diff를 보고 수락·거절).
    ProposeEdit,
    /// 공개 예제로 로컬 실행([`AI_TOOL_RUN_EXAMPLES`]가 꺼져 있으면 안 준다).
    RunExamples,
}

impl Tool {
    pub const ALL: [Tool; 4] = [
        Tool::ReadProblem,
        Tool::ReadCode,
        Tool::ProposeEdit,
        Tool::RunExamples,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::ReadProblem => "read_problem",
            Tool::ReadCode => "read_code",
            Tool::ProposeEdit => "propose_edit",
            Tool::RunExamples => "run_examples",
        }
    }

    /// 모델이 부른 이름 → 켜진 도구(꺼진 도구·모르는 이름은 `None`).
    pub fn enabled_by_name(name: &str) -> Option<Tool> {
        enabled_tools().into_iter().find(|t| t.name() == name)
    }

    /// 템플릿 `<tools>`에 싣는 정의(OpenAI 함수 꼴 — Qwen 템플릿이 그대로 받는다).
    pub fn definition(self) -> Value {
        let (description, parameters) = match self {
            Tool::ReadProblem => (
                "Read the problem: title, description, function signature, public examples and limits.",
                json!({"type": "object", "properties": {}, "required": []}),
            ),
            Tool::ReadCode => (
                "Read the user's current code in the editor, with line numbers, and its language.",
                json!({"type": "object", "properties": {}, "required": []}),
            ),
            Tool::ProposeEdit => (
                "Propose replacing lines start_line..end_line (1-based, inclusive) of the current code with text. \
The range must include every line you change (with the function header and closing brace if they are affected), \
and text must be the complete code that replaces the whole range. \
The user sees a diff and accepts or rejects it. To insert without replacing, use end_line = start_line - 1.",
                json!({
                    "type": "object",
                    "properties": {
                        "start_line": {"type": "integer", "description": "First line to replace (1-based)."},
                        "end_line": {"type": "integer", "description": "Last line to replace (inclusive)."},
                        "text": {"type": "string", "description": "Complete new code for the whole range, with indentation."}
                    },
                    "required": ["start_line", "end_line", "text"]
                }),
            ),
            Tool::RunExamples => (
                "Run the user's current code on the public examples on their computer and get the result of each example.",
                json!({"type": "object", "properties": {}, "required": []}),
            ),
        };
        json!({
            "type": "function",
            "function": {"name": self.name(), "description": description, "parameters": parameters}
        })
    }
}

/// 모델에 주는 도구(켜진 것만, 순서 고정).
pub fn enabled_tools() -> Vec<Tool> {
    Tool::ALL
        .into_iter()
        .filter(|t| *t != Tool::RunExamples || AI_TOOL_RUN_EXAMPLES)
        .collect()
}

/// 모델이 보는 대화 메시지 하나.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    /// 응시자 차례(앱이 붙인 알림 — 제안 수락·거절 — 이 앞에 들 수 있다).
    User { text: String },
    /// 모델 답(글 + 도구 호출).
    Assistant {
        text: String,
        tool_calls: Vec<ToolCall>,
    },
    /// 바로 앞 assistant의 도구 호출마다 결과 하나(같은 순서).
    ToolResults { results: Vec<String> },
}

/// 요약으로 접은 앞 대화.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "text", rename_all = "snake_case")]
pub enum Folded {
    /// 로컬 모델이 만든 요약.
    Summary(String),
    /// 요약에 실패해 앞 대화를 뺐다(화면에 「앞 대화 생략」).
    Omitted,
}

/// 모델에 넣을 프롬프트 — 앞 호출의 KV를 다시 쓰도록 접두와 새 조각으로 나눈다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    /// system + 도구 + 앞 메시지들.
    pub prefix: String,
    /// 마지막 메시지 + assistant 차례 열기(비지 않는다).
    pub suffix: String,
}

impl Prompt {
    pub fn full(&self) -> String {
        format!("{}{}", self.prefix, self.suffix)
    }
}

/// 특수 토큰 표기(`<|im_start|>` 같은 `<|…|>`)를 무력화한다: 템플릿은 특수 토큰을 읽어 들이므로 응시자 글·코드에
/// `<|im_end|>\n<|im_start|>system` 같은 글이 있으면 차례를 위조할 수 있다. `<|`를 `< |`로 바꾼다(결정적).
pub fn neutralize(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains("<|") {
        text.replace("<|", "< |").into()
    } else {
        text.into()
    }
}

/// Python `json.dumps`(Jinja `tojson`) 꼴 — `", "`·`": "` 구분, ASCII 밖 글자는 그대로. 키 순서는 `Map` 순서.
pub fn py_json(value: &Value) -> String {
    let mut out = String::new();
    write_py_json(&mut out, value);
    out
}

fn write_py_json(out: &mut String, value: &Value) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_py_json(out, v);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push_str(": ");
                write_py_json(out, v);
            }
            out.push('}');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// system 차례(고정 system + 접은 앞 대화 + 도구 안내). 도구가 없으면 도구 안내를 뺀다.
pub fn system_block(folded: Option<&Folded>, tools: &[Tool]) -> String {
    let mut s = format!("<|im_start|>system\n{SYSTEM_PROMPT}");
    match folded {
        Some(Folded::Summary(text)) => {
            let _ = write!(
                s,
                "\n\nSummary of the earlier conversation:\n{}",
                neutralize(text)
            );
        }
        Some(Folded::Omitted) => {
            s.push_str("\n\nThe earlier conversation was omitted.");
        }
        None => {}
    }
    if !tools.is_empty() {
        s.push_str("\n\n# Tools\n\nYou may call one or more functions to assist with the user query.\n\nYou are provided with function signatures within <tools></tools> XML tags:\n<tools>");
        for t in tools {
            s.push('\n');
            s.push_str(&py_json(&t.definition()));
        }
        s.push_str("\n</tools>\n\nFor each function call, return a json object with function name and arguments within <tool_call></tool_call> XML tags:\n<tool_call>\n{\"name\": <function-name>, \"arguments\": <args-json-object>}\n</tool_call>");
    }
    s.push_str("<|im_end|>\n");
    s
}

/// 메시지 하나의 템플릿 조각.
pub fn render_message(message: &Message) -> String {
    match message {
        Message::User { text } => format!("<|im_start|>user\n{}<|im_end|>\n", neutralize(text)),
        Message::Assistant { text, tool_calls } => {
            let mut s = String::from("<|im_start|>assistant");
            if !text.is_empty() {
                s.push('\n');
                s.push_str(&neutralize(text));
            }
            for call in tool_calls {
                let _ = write!(
                    s,
                    "\n<tool_call>\n{{\"name\": \"{}\", \"arguments\": {}}}\n</tool_call>",
                    neutralize(&call.name),
                    neutralize(&py_json(&call.arguments))
                );
            }
            s.push_str("<|im_end|>\n");
            s
        }
        Message::ToolResults { results } => {
            let mut s = String::from("<|im_start|>user");
            for r in results {
                let _ = write!(s, "\n<tool_response>\n{}\n</tool_response>", neutralize(r));
            }
            s.push_str("<|im_end|>\n");
            s
        }
    }
}

/// assistant 차례를 여는 조각.
pub const ASSISTANT_OPEN: &str = "<|im_start|>assistant\n";

/// 대화 → 프롬프트. 마지막 메시지(보통 새 user 차례나 도구 결과)를 `suffix`로 떼어 앞의 KV를 다시 쓰게 한다.
/// 메시지가 없으면 `suffix`는 assistant 차례 열기뿐이다.
pub fn render(folded: Option<&Folded>, messages: &[Message], tools: &[Tool]) -> Prompt {
    render_with_context(folded, messages, tools, None)
}

/// [`render`] + 상태 블록(#116): `context = (i, 블록)`이면 메시지 `i`(이번 차례의 user 메시지)의 글 뒤에
/// `<app_context>…</app_context>`를 가짜 도구 왕복으로 잇는다([`render_message_with_context`]). 블록은 대화에 저장하지 않는다 — 다음
/// 차례에는 새 블록이 새 user 메시지에 붙고 옛 메시지는 블록 없이 그려진다(옛 코드·줄 번호가 대화에 쌓이지 않는다).
pub fn render_with_context(
    folded: Option<&Folded>,
    messages: &[Message],
    tools: &[Tool],
    context: Option<(usize, &str)>,
) -> Prompt {
    let mut prefix = system_block(folded, tools);
    let one = |i: usize, m: &Message| match context {
        Some((at, block)) if at == i => render_message_with_context(m, block),
        _ => render_message(m),
    };
    let last = messages.len().checked_sub(1);
    for (i, m) in messages.iter().enumerate() {
        if Some(i) != last {
            prefix.push_str(&one(i, m));
        }
    }
    let mut suffix = last.map(|i| one(i, &messages[i])).unwrap_or_default();
    suffix.push_str(ASSISTANT_OPEN);
    Prompt { prefix, suffix }
}

/// 상태 블록을 싣는 가짜 도구 호출 이름(#116). 도구 목록에는 없고 앱만 부른다(모델이 부르면 「모르는 도구」 결과).
pub const CONTEXT_CALL: &str = "read_context";

/// user 메시지 하나를 상태 블록과 함께 그린다(user가 아니면 [`render_message`] 그대로): 응시자 글 차례 뒤에 앱이 부른
/// 것처럼 [`CONTEXT_CALL`] 호출(assistant 차례)과 그 결과(`<tool_response>` 안 `<app_context>`)를 잇는다 — 블록을
/// user 글에 붙이면(앞이든 뒤든) 실제 7B가 블록을 답에 그대로 되풀이하고 도구를 부르지 않았다. 도구 결과 꼴이면 Qwen2.5가
/// 결과로 읽고 이어서 답하거나 `propose_edit`을 부른다.
pub fn render_message_with_context(message: &Message, block: &str) -> String {
    match message {
        Message::User { text } => format!(
            "<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n<tool_call>\n{{\"name\": \"{CONTEXT_CALL}\", \"arguments\": {{}}}}\n</tool_call><|im_end|>\n<|im_start|>user\n<tool_response>\n<app_context>\n{}\n</app_context>\n</tool_response><|im_end|>\n",
            neutralize(text),
            neutralize(block.trim_end_matches('\n'))
        ),
        other => render_message(other),
    }
}

/// 상태 블록의 문제 글 최대 글자 수(넘으면 자르고 `read_problem`으로 안내한다).
pub const MAX_CONTEXT_PROBLEM_CHARS: usize = 4_000;

/// 상태 블록의 코드 최대 글자 수(줄 번호 포함, 약 3~4k 토큰). 넘으면 [`context_code`]가 가운데를 뺀다.
pub const MAX_CONTEXT_CODE_CHARS: usize = 12_000;

/// 상태 블록에서 한 줄의 최대 글자 수(넘는 줄은 끝을 자른다 — 한 줄짜리 거대한 코드가 예산을 다 먹지 않게).
pub const MAX_CONTEXT_LINE_CHARS: usize = 400;

/// 매 응시자 메시지 앞에 붙는 상태 블록(#116): 공개 문제(`read_problem`과 같은 글 — 숨은 입력은 앱에 없다), 고른 언어,
/// 편집기의 지금 코드([`context_code`] — `read_code`와 같은 줄 번호). 응시자 글이 아니라 앱이 쓴 것이고 과정 로그에는
/// 싣지 않는다(코드는 그 순간 강제한 `CodeSnapshot`이 남는다).
pub fn context_block(problem: Option<&str>, language: Language, code: &str) -> String {
    let mut s = String::new();
    match problem {
        Some(p) => {
            let p = p.trim();
            if p.chars().count() <= MAX_CONTEXT_PROBLEM_CHARS {
                let _ = writeln!(s, "Problem:\n{p}\n");
            } else {
                let head: String = p.chars().take(MAX_CONTEXT_PROBLEM_CHARS).collect();
                let _ = writeln!(
                    s,
                    "Problem:\n{head}\n…(cut — call read_problem for the whole statement)\n"
                );
            }
        }
        None => s.push_str("Problem: (none open)\n\n"),
    }
    let _ = writeln!(s, "Language: {}\n", language.as_str());
    s.push_str("Code in the editor (line numbers as in read_code and propose_edit):\n");
    s.push_str(&context_code(code));
    s
}

/// 상태 블록의 코드: [`numbered`]와 같은 꼴·같은 줄 번호(번호 폭도 전체 줄 수 기준). 전체가 [`MAX_CONTEXT_CODE_CHARS`]
/// 이하면 그대로, 넘으면 **앞과 뒤를 반씩** 남기고 가운데 줄들을 한 줄 안내(`… lines a-b omitted — call read_code …`)로
/// 바꾼다. [`MAX_CONTEXT_LINE_CHARS`]를 넘는 줄은 끝을 자르고 표시한다. 같은 코드면 같은 글(결정적).
pub fn context_code(code: &str) -> String {
    let lines = split_lines(code);
    let width = lines.len().max(1).to_string().len();
    let rendered: Vec<String> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let body = if line.chars().count() > MAX_CONTEXT_LINE_CHARS {
                let head: String = line.chars().take(MAX_CONTEXT_LINE_CHARS).collect();
                format!("{head} …(line cut)")
            } else {
                (*line).to_owned()
            };
            format!("{:>width$}| {body}\n", i + 1)
        })
        .collect();
    let total: usize = rendered.iter().map(|l| l.chars().count()).sum();
    if total <= MAX_CONTEXT_CODE_CHARS {
        return rendered.concat();
    }
    let half = MAX_CONTEXT_CODE_CHARS / 2;
    let take = |iter: &mut dyn Iterator<Item = &String>| {
        let mut used = 0usize;
        let mut n = 0usize;
        for l in iter {
            used += l.chars().count();
            if used > half {
                break;
            }
            n += 1;
        }
        n
    };
    let head = take(&mut rendered.iter());
    let tail = take(&mut rendered.iter().rev());
    let mut s = rendered[..head].concat();
    let (from, to) = (head + 1, rendered.len() - tail);
    let _ = writeln!(
        s,
        "… lines {from}-{to} omitted ({} lines) — call read_code to read the whole code …",
        to + 1 - from
    );
    s.push_str(&rendered[rendered.len() - tail..].concat());
    s
}

/// 스트리밍 중 답에서 감춘 부분(#116 — 도구 호출 JSON은 화면에 날것으로 흘리지 않는다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hidden {
    /// 도구 호출일 수도 있는 시작(`{`·```·`<` 조각) — 더 나와야 안다.
    Undecided,
    /// 도구 호출(꼬리표나 호출 꼴 JSON)이 나오는 중. 이름을 읽었으면 그 도구(모르는 이름이면 `None`).
    Call(Option<Tool>),
}

/// 스트리밍 중인 답의 보일 글과 감춘 부분.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamView {
    pub text: String,
    pub hidden: Option<Hidden>,
}

/// 생성 중인 답 원문 → 화면에 보일 글. [`parse_reply`]가 끝에서 호출로 꺼낼 수 있는 것은 보이지 않게 한다:
/// - `<tool_call>`·`<tool_response>` 꼬리표부터 끝까지 감춘다(앞의 글은 보인다). 끝에 꼬리표의 앞 조각(`<tool` 등)이
///   걸려 있으면 그 조각만 잠시 감춘다.
/// - 답이 `{`로 시작하면(꼬리표 없는 호출 JSON일 수 있다) 전부 감춘다. 울타리(```` ``` ````)는 언어 꼬리표가 무엇이든
///   (실제 7B는 호출을 ```` ```python ````으로도 싼다 — #116 Windows 루프 3) 본문이 `{`로 시작하면 감추고, 코드로 시작하면
///   보인다. 첫 줄(꼬리표)이 덜 나왔거나 본문이 아직 없으면 판단할 만큼 안 나왔으니 감춘다. 답 앞 울타리가 닫혔으면
///   그 뒤 글은 보인다.
///
/// 끝난 답은 [`parse_reply`]가 정한다 — 감췄던 것이 호출이 아니면 그때 글로 보인다.
pub fn stream_view(raw: &str) -> StreamView {
    let mut v = stream_view_raw(cut_special(raw));
    if let std::borrow::Cow::Owned(t) = strip_turn_start(&v.text) {
        v.text = t;
    }
    if let std::borrow::Cow::Owned(t) = hangul_lines(&v.text) {
        v.text = t;
    }
    v
}

fn stream_view_raw(raw: &str) -> StreamView {
    // `<|im_start|>`는 답에 보일 일이 없는 차례 표시다 — 실제 7B가 호출을 그것으로 싼 적이 있다(#125).
    const TAGS: [&str; 3] = ["<tool_call>", "<tool_response>", IM_START];
    let at = TAGS.iter().filter_map(|t| raw.find(t)).min();
    if let Some(at) = at {
        return StreamView {
            text: raw[..at].trim().to_owned(),
            hidden: Some(Hidden::Call(called_tool(&raw[at..]))),
        };
    }
    // 끝에 걸린 꼬리표 앞 조각.
    let mut visible = raw;
    let mut hidden = None;
    if let Some(lt) = raw.rfind('<') {
        let tail = &raw[lt..];
        if TAGS.iter().any(|t| t.starts_with(tail)) {
            visible = &raw[..lt];
            hidden = Some(Hidden::Undecided);
        }
    }
    let t = visible.trim_start();
    // 글 뒤에 열린 ```json 울타리(호출일 수 있다 — [`parse_reply`]가 끝나고 정한다)는 감춘다.
    if let Some(open) = trailing_fence_open(t).filter(|&at| at > 0) {
        return StreamView {
            text: t[..open].trim().to_owned(),
            hidden: Some(Hidden::Call(called_tool(&t[open..]))),
        };
    }
    if t.starts_with('{') {
        return StreamView {
            text: String::new(),
            hidden: Some(Hidden::Call(called_tool(t))),
        };
    }
    if "```".starts_with(t) && !t.is_empty() {
        return StreamView {
            text: String::new(),
            hidden: Some(Hidden::Undecided),
        };
    }
    if let Some((_, body)) = t.strip_prefix("```").and_then(split_fence) {
        let body = body.map(str::trim_start);
        match body {
            None | Some("") => {
                return StreamView {
                    text: String::new(),
                    hidden: Some(Hidden::Undecided),
                };
            }
            Some(b) if b.starts_with('{') => {
                // 닫힌 울타리 뒤 글은 보인다(호출인지는 끝나고 parse_reply가 정한다).
                let after = b.find("```").map_or("", |end| b[end + 3..].trim());
                return StreamView {
                    text: after.to_owned(),
                    hidden: Some(Hidden::Call(called_tool(b))),
                };
            }
            Some(_) => {}
        }
    }
    StreamView {
        text: t.trim_end().to_owned(),
        hidden,
    }
}

/// 나오는 중인 호출 글에서 `"name": "…"`을 읽어 도구로(이름이 다 안 나왔거나 모르면 `None`).
fn called_tool(s: &str) -> Option<Tool> {
    let at = s.find("\"name\"")?;
    let rest = s[at + 6..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let name = &rest[..rest.find('"')?];
    Tool::ALL.into_iter().find(|t| t.name() == name)
}

/// 채팅 템플릿의 차례 시작 표시.
const IM_START: &str = "<|im_start|>";

/// 모델 답을 글과 도구 호출로 나눈다. `<tool_call>…</tool_call>` 안이 `{"name": 문자열, "arguments": 객체}` JSON이면
/// 호출로 꺼내고(글에서는 뺀다), 꼴이 틀리면 글에 그대로 둔다. 닫는 꼬리표가 없는 마지막 블록(최대 토큰에서 잘림)도 시도한다.
pub fn parse_reply(raw: &str) -> (String, Vec<ToolCall>) {
    let (text, calls) = parse_reply_raw(cut_special(raw));
    (hangul_lines(&strip_turn_start(&text)).into_owned(), calls)
}

fn parse_reply_raw(raw: &str) -> (String, Vec<ToolCall>) {
    const OPEN: &str = "<tool_call>";
    const CLOSE: &str = "</tool_call>";
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut rest = raw;
    while let Some(at) = rest.find(OPEN) {
        text.push_str(&rest[..at]);
        let after = &rest[at + OPEN.len()..];
        let (inner, next) = match after.find(CLOSE) {
            Some(end) => (&after[..end], &after[end + CLOSE.len()..]),
            None => (after, ""),
        };
        match parse_call(inner) {
            Some(call) => calls.push(call),
            None => {
                text.push_str(OPEN);
                text.push_str(inner);
                if !next.is_empty() || after.contains(CLOSE) {
                    text.push_str(CLOSE);
                }
            }
        }
        rest = next;
    }
    text.push_str(rest);
    // 꼬리표 없이 호출 JSON만 낸 답(Qwen2.5-Coder 7B가 탐욕적 선택에서 자주 그런다 — #98 실제 모델 시험)도 호출로 받는다.
    // 답 전체가 그 꼴(줄마다 하나씩 여러 개도 — 실제 7B가 `read_problem`·`read_code`를 잇달아 낸다)이거나
    // ```json 울타리 하나일 때만.
    if calls.is_empty() {
        let bare = text.trim();
        let fenced = bare
            .strip_prefix("```")
            .and_then(split_fence)
            .and_then(|(_, body)| body?.trim_end().strip_suffix("```"))
            .or_else(|| {
                bare.strip_prefix("```json")
                    .and_then(|b| b.strip_suffix("```"))
            })
            .unwrap_or(bare);
        // 실제 7B는 호출을 결과 꼬리표(`<tool_response>`)로 잘못 싸기도 한다 — 안이 호출 꼴이면 받는다.
        let fenced = fenced
            .strip_prefix("<tool_response>")
            .and_then(|b| b.strip_suffix("</tool_response>"))
            .unwrap_or(fenced);
        if let Some(bare_calls) = parse_bare_calls(fenced) {
            return (String::new(), bare_calls);
        }
        // 글 사이의 울타리 안이 호출 꼴이면(#116 — 상태 블록을 받은 실제 7B가 「…고치자.」 뒤에 ```json으로, Windows에서는
        // 답 앞에 ```python으로 싸서 냈다) 그 울타리만 호출로 받고 나머지 글은 남긴다. 꼬리표는 따지지 않는다.
        if let Some((rest, fence_calls)) = fenced_calls(bare) {
            return (rest, fence_calls);
        }
        // #125: 실제 7B가 되돌린 제안을 다시 낼 때 호출을 꼬리표 대신 차례 표시 `<|im_start|>`로 쌌다 — 뒤가 호출 꼴이면 받는다.
        if let Some(at) = bare.find(IM_START) {
            let tail = bare[at + IM_START.len()..].trim();
            let tail = tail.strip_prefix("assistant").map_or(tail, str::trim_start);
            if let Some(c) = parse_bare_calls(tail) {
                return (bare[..at].trim().to_owned(), c);
            }
        }
    }
    (text.trim().to_owned(), calls)
}

/// 글 안의 울타리 중 본문이 호출 꼴인 것들을 꺼낸다 → (그 울타리들을 뺀 글, 호출들). 없으면 `None`.
/// 호출 꼴이 아닌 울타리(코드·예시 JSON)는 글에 그대로 둔다. 닫히지 않은 마지막 울타리(최대 토큰에서 잘림)도 시도한다.
fn fenced_calls(text: &str) -> Option<(String, Vec<ToolCall>)> {
    let mut rest_text = String::new();
    let mut calls = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("```") {
        let after = &rest[open + 3..];
        // 여는 줄(꼬리표)과 본문, 닫는 울타리.
        let Some((_, Some(body))) = split_fence(after) else {
            rest_text.push_str(&rest[..open + 3]);
            rest = after;
            continue;
        };
        let (inner, next) = match body.find("```") {
            Some(end) => (&body[..end], &body[end + 3..]),
            None => (body, ""),
        };
        match parse_bare_calls(inner.trim()) {
            Some(found) => {
                rest_text.push_str(&rest[..open]);
                calls.extend(found);
            }
            // 울타리째(닫는 ``` 포함) 글로 둔다.
            None => rest_text.push_str(&rest[..rest.len() - next.len()]),
        }
        rest = next;
    }
    rest_text.push_str(rest);
    (!calls.is_empty()).then(|| (tidy(&rest_text), calls))
}

/// 차례 끝 표지(#131 루프 2). 답 원문에 글로 나오면 거기서 답이 끝난 것이다.
pub const TURN_END_MARKERS: [&str; 2] = ["<|im_end|>", "<|endoftext|>"];

/// 차례 머리 표지. 7B는 이것으로 도구 호출을 싸기도 해서(#125) 원문을 자르지 않고, 호출을 읽은 뒤 **보이는 글에서만**
/// 지운다 — 1.5B가 Windows에서 「<|im_start|>」 한 줄을 답으로 보였다(#131 루프 2).
pub const TURN_START_MARKER: &str = "<|im_start|>";

/// 답 원문을 첫 차례 끝 표지 앞까지만 남긴다.
pub fn cut_special(raw: &str) -> &str {
    let end = TURN_END_MARKERS
        .iter()
        .filter_map(|m| raw.find(m))
        .min()
        .unwrap_or(raw.len());
    &raw[..end]
}

/// 보이는 글에서 차례 머리 표지(와 바로 뒤 역할 이름 줄)를 지운다. 표지가 없으면 그대로.
pub fn strip_turn_start(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains(TURN_START_MARKER) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(TURN_START_MARKER) {
        out.push_str(&rest[..i]);
        rest = &rest[i + TURN_START_MARKER.len()..];
        for role in ["assistant\n", "user\n", "system\n"] {
            if let Some(r) = rest.strip_prefix(role) {
                rest = r;
                break;
            }
        }
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out.trim().to_owned())
}

/// 숫자 뒤의 「行」을 「줄」로(#125 루프 2 — system에서 막아도 실제 7B가 「Python 2行은…」·「6-9行」을 썼다).
/// 숫자(와 사이 공백) 바로 뒤의 `行`만 바꾸고 다른 글은 그대로 둔다 — 보이는 글(끝난 답·스트리밍)에만 쓴다.
pub fn hangul_lines(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('行') {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '行'
            && out
                .trim_end_matches(' ')
                .ends_with(|p: char| p.is_ascii_digit())
        {
            out.push('줄');
        } else {
            out.push(c);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// 울타리를 뺀 자리의 빈 줄을 줄인다.
fn tidy(text: &str) -> String {
    let mut out = String::new();
    for part in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(part);
    }
    out
}

/// 울타리 여는 ```` ``` ```` 뒤를 (언어 꼬리표, 본문)으로. 꼬리표는 비었거나 글자·숫자·`+#-_.`만(`json`·`python`·`c++`)
/// — 아니면 울타리가 아니다. 첫 줄이 아직 안 끝났으면 본문 `None`.
fn split_fence(rest: &str) -> Option<(&str, Option<&str>)> {
    let (lang, body) = match rest.split_once('\n') {
        Some((lang, body)) => (lang, Some(body)),
        None => (rest, None),
    };
    let lang = lang.trim();
    lang.chars()
        .all(|c| c.is_alphanumeric() || "+#-_.".contains(c))
        .then_some((lang, body))
}

/// 글 안 마지막 울타리 여는 자리 — 꼬리표가 무엇이든 본문이 아직 없거나 `{`로 시작할 때만(호출일 수 있다).
fn trailing_fence_open(text: &str) -> Option<usize> {
    let at = text.rfind("```")?;
    let (_, body) = split_fence(&text[at + 3..])?;
    let body = body.map_or("", str::trim_start);
    (body.is_empty() || body.starts_with('{')).then_some(at)
}

/// 공백으로 이어진 호출 JSON 객체들(하나 이상). 하나라도 호출 꼴이 아니면 `None`.
fn parse_bare_calls(text: &str) -> Option<Vec<ToolCall>> {
    let mut calls = Vec::new();
    for value in serde_json::Deserializer::from_str(text).into_iter::<Value>() {
        calls.push(parse_call(&value.ok()?.to_string())?);
    }
    (!calls.is_empty()).then_some(calls)
}

fn parse_call(inner: &str) -> Option<ToolCall> {
    let v: Value = serde_json::from_str(inner.trim()).ok()?;
    let name = v.get("name")?.as_str()?.to_owned();
    let arguments = match v.get("arguments") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        // 어떤 모델은 인자를 JSON 문자열로 싸서 낸다.
        Some(Value::String(s)) => serde_json::from_str(s).ok()?,
        Some(a @ Value::Object(_)) => a.clone(),
        Some(_) => return None,
    };
    Some(ToolCall { name, arguments })
}

/// 도구 결과를 [`MAX_TOOL_RESULT_CHARS`]에서 자른다.
pub fn clip_result(text: String) -> String {
    if text.chars().count() <= MAX_TOOL_RESULT_CHARS {
        return text;
    }
    let mut s: String = text.chars().take(MAX_TOOL_RESULT_CHARS).collect();
    s.push_str("\n…(truncated)");
    s
}

/// 줄 번호를 붙인 코드(`read_code` 결과 — `propose_edit`의 줄 번호와 같은 1부터).
pub fn numbered(code: &str) -> String {
    let lines = split_lines(code);
    let width = lines.len().max(1).to_string().len();
    let mut s = String::new();
    for (i, line) in lines.iter().enumerate() {
        let _ = writeln!(s, "{:>width$}| {line}", i + 1);
    }
    s
}

/// 줄로 나눈다(`\n` 기준, 끝의 `\r`은 뺀다). 빈 코드는 빈 줄 하나다(편집기와 같다).
fn split_lines(code: &str) -> Vec<&str> {
    code.split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect()
}

/// `propose_edit` 인자.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineEdit {
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
}

/// 줄 편집을 받을 수 없는 까닭(모델에게 도구 결과로 돌려준다).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("arguments must be {{\"start_line\": int, \"end_line\": int, \"text\": string}}")]
    BadArguments,
    #[error("line range {start}..{end} is outside the code (1..{lines}); read_code first")]
    OutOfRange { start: u32, end: u32, lines: u32 },
    #[error("the edit does not change the code")]
    NoChange,
}

impl LineEdit {
    pub fn from_arguments(arguments: &Value) -> Result<Self, EditError> {
        let n = |k: &str| {
            arguments
                .get(k)
                .and_then(|v| v.as_u64().or_else(|| v.as_str()?.trim().parse().ok()))
                .and_then(|v| u32::try_from(v).ok())
        };
        let text = arguments.get("text").and_then(Value::as_str);
        match (n("start_line"), n("end_line"), text) {
            (Some(start_line), Some(end_line), Some(text)) => Ok(Self {
                start_line,
                end_line,
                text: text.to_owned(),
            }),
            _ => Err(EditError::BadArguments),
        }
    }
}

/// 계산한 편집: 편집기 범위(Monaco 1부터, 끝 열은 포함하지 않는다)와 넣을 글, 적용 뒤 코드.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedEdit {
    pub range: EditRange,
    pub text: String,
    pub after: String,
}

/// 줄 편집 → 편집기 범위. `start..=end` 줄 전체를 `text`로 바꾼다(줄 끝 줄바꿈은 그대로 둔다). `end = start - 1`이면
/// `start` 줄 앞에 끼워 넣는다. 범위 밖이거나 바뀌는 것이 없으면 오류.
pub fn plan_edit(code: &str, edit: &LineEdit) -> Result<PlannedEdit, EditError> {
    let lines = split_lines(code);
    let count = lines.len() as u32;
    let (start, end) = (edit.start_line, edit.end_line);
    let out_of_range = EditError::OutOfRange {
        start,
        end,
        lines: count,
    };
    if start == 0 || end + 1 < start || end > count || start > count + 1 {
        return Err(out_of_range);
    }
    let mut text = edit.text.replace("\r\n", "\n");
    let range = if end + 1 == start {
        // 끼워 넣기: start 줄 맨 앞(마지막 줄 뒤면 끝에 줄을 하나 더한다).
        if start > count {
            let last = lines[lines.len() - 1];
            let col = last.chars().count() as u32 + 1;
            text = format!("\n{}", text.strip_suffix('\n').unwrap_or(&text));
            EditRange {
                start_line: count,
                start_column: col,
                end_line: count,
                end_column: col,
            }
        } else {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            EditRange {
                start_line: start,
                start_column: 1,
                end_line: start,
                end_column: 1,
            }
        }
    } else {
        if start > count {
            return Err(out_of_range);
        }
        // 바꾸기: start 줄 첫 열부터 end 줄 끝까지(줄바꿈 앞). 새 글 끝의 줄바꿈 하나는 뺀다(원래 줄바꿈이 남는다).
        if let Some(t) = text.strip_suffix('\n') {
            text = t.to_owned();
        }
        let last = lines[end as usize - 1];
        EditRange {
            start_line: start,
            start_column: 1,
            end_line: end,
            end_column: last.chars().count() as u32 + 1,
        }
    };
    let after = apply_range(code, &range, &text).ok_or(out_of_range)?;
    if after == code {
        return Err(EditError::NoChange);
    }
    Ok(PlannedEdit { range, text, after })
}

/// 제안 범위의 한 줄 어긋남을 맞춘다(#125). 적용하면 코드가 깨질 때만([`crate::syntax::regression`], `function` = 공개
/// 서명의 풀이 함수 이름) 아래 후보를 차례로 보고 **깨짐이 없어지는 첫 후보**를 준다(없으면 `None` — 되돌림·경고 몫):
/// - 넓히기: 글의 마지막 줄이 범위 바로 뒤 줄과 같다(닫는 `}`를 글에 넣고 범위는 그 앞까지 — 실제 7B가 되돌려도
///   되풀이했다) → 끝을 한 줄 늘림. 글의 첫 줄이 범위 바로 앞 줄과 같다 → 시작을 한 줄 당김. 둘 다면 둘 다.
/// - 줄이기: 범위 첫 줄이 블록 머리(풀이 함수 머리 줄, 또는 `{`·`:`로 끝나는 줄)인데 글에 그 줄이 없다 → 시작을 한 줄
///   미룸. 범위 끝 줄이 닫는 줄(`}`·`};`·`)` 따위만)인데 글의 마지막 줄이 그것이 아니다 → 끝을 한 줄 줄임. 둘 다면
///   둘 다 먼저(#125 Windows 재현: 빈 틀 6–9줄 = 머리·본문 2줄·`}`를 본문만으로 바꿈 → 7–8줄).
///
/// 빈 줄·앞뒤 공백만 다른 것은 같은 줄로 본다. 줄이기는 머리·닫는 줄을 **남기는** 쪽이라 응시자 코드를 덜 지운다.
pub fn snap_edit(
    language: Language,
    function: Option<&str>,
    code: &str,
    edit: &LineEdit,
) -> Option<LineEdit> {
    use crate::syntax::{header_lines, regression};
    let planned = plan_edit(code, edit).ok()?;
    regression(language, function, code, &planned.after)?;
    let (start, end) = (edit.start_line, edit.end_line);
    if end < start {
        return None;
    }
    let lines = split_lines(code);
    let line = |n: u32| {
        n.checked_sub(1)
            .and_then(|i| lines.get(i as usize))
            .map(|l| l.trim())
    };
    let text_lines: Vec<&str> = edit
        .text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let (first, last) = (text_lines.first().copied()?, text_lines.last().copied()?);
    let grow_end = line(end + 1).is_some_and(|l| !l.is_empty() && l == last);
    let grow_start = line(start.wrapping_sub(1)).is_some_and(|l| !l.is_empty() && l == first);
    let headers = function
        .map(|f| header_lines(language, f, code))
        .unwrap_or_default();
    let head = line(start).unwrap_or("");
    let is_head = headers.iter().any(|(n, _)| *n == start)
        || (head.ends_with('{') || (language == Language::Python && head.ends_with(':')));
    let shrink_start = start < end && is_head && !text_lines.contains(&head);
    let tail = line(end).unwrap_or("");
    let closing = !tail.is_empty() && tail.chars().all(|c| matches!(c, '}' | ']' | ')' | ';'));
    let shrink_end = start < end && closing && last != tail;
    let mut tries = Vec::new();
    if grow_end {
        tries.push((start, end + 1));
    }
    if grow_start {
        tries.push((start - 1, end));
    }
    if grow_end && grow_start {
        tries.push((start - 1, end + 1));
    }
    if shrink_start && shrink_end && start + 1 < end {
        tries.push((start + 1, end - 1));
    }
    if shrink_start {
        tries.push((start + 1, end));
    }
    if shrink_end {
        tries.push((start, end - 1));
    }
    tries.into_iter().find_map(|(start_line, end_line)| {
        let e = LineEdit {
            start_line,
            end_line,
            text: edit.text.clone(),
        };
        let p = plan_edit(code, &e).ok()?;
        regression(language, function, code, &p.after)
            .is_none()
            .then_some(e)
    })
}

/// 편집기 범위(1부터, 열은 문자 단위)를 `text`로 바꾼 코드. 범위가 코드 밖이면 `None`.
pub fn apply_range(code: &str, range: &EditRange, text: &str) -> Option<String> {
    let start = offset(code, range.start_line, range.start_column)?;
    let end = offset(code, range.end_line, range.end_column)?;
    if end < start {
        return None;
    }
    Some(format!("{}{}{}", &code[..start], text, &code[end..]))
}

/// (줄, 열) → 바이트 위치. 줄 끝(줄바꿈 앞, `\r` 앞)까지 갈 수 있다.
fn offset(code: &str, line: u32, column: u32) -> Option<usize> {
    if line == 0 || column == 0 {
        return None;
    }
    let mut at = 0usize;
    for (i, l) in code.split('\n').enumerate() {
        if i + 1 == line as usize {
            let body = l.strip_suffix('\r').unwrap_or(l);
            let mut chars = body.char_indices().map(|(b, _)| b).chain([body.len()]);
            return chars.nth(column as usize - 1).map(|b| at + b);
        }
        at += l.len() + 1;
    }
    None
}

/// 문맥 예산(토큰).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// 문맥 창([`crate::inference::CONTEXT_TOKENS`]).
    pub context: usize,
    /// 답 자리(`SamplingParams::max_tokens`).
    pub reply: usize,
    /// 템플릿·다음 user 차례 여유.
    pub margin: usize,
}

impl Budget {
    /// 앱 기본: 32k 창, 답 1024, 여유 2048 — 프롬프트가 약 29.7k를 넘으면 요약한다.
    pub fn app() -> Self {
        Self {
            context: crate::inference::CONTEXT_TOKENS as usize,
            reply: crate::inference::SamplingParams::default().max_tokens as usize,
            margin: 2048,
        }
    }

    /// 문맥 창을 `n_ctx`(실제 추론기가 연 창 — [`crate::inference::Inference::context_tokens`]) 이하로 줄인다.
    pub fn capped(self, n_ctx: u32) -> Self {
        Self {
            context: self.context.min(n_ctx as usize),
            ..self
        }
    }

    /// 요약을 시작하는 프롬프트 길이(이보다 길면 접는다).
    pub fn compact_at(&self) -> usize {
        self.context.saturating_sub(self.reply + self.margin)
    }

    /// 접은 뒤 남길 목표(창의 절반 — 요약 한 번에 여러 차례를 더 쓸 수 있게).
    pub fn target(&self) -> usize {
        self.context / 2
    }
}

/// 문맥이 차면 앞에서 몇 개의 메시지를 접을지 고른다. `fixed` = system·도구·접은 앞 대화 토큰,
/// `tokens[i]` = 메시지 i의 토큰. 접는 자리는 **user 메시지 앞**만(도구 호출과 그 결과를 가르지 않는다)이고 마지막 user
/// 메시지는 남긴다. 목표([`Budget::target`])에 닿는 가장 앞 자리, 없으면 남길 수 있는 가장 뒤 자리. 접을 필요가 없거나
/// 접을 자리가 없으면 `None`.
pub fn compaction_cut(
    messages: &[Message],
    tokens: &[usize],
    fixed: usize,
    budget: &Budget,
) -> Option<usize> {
    let total: usize = fixed + tokens.iter().sum::<usize>();
    if total <= budget.compact_at() {
        return None;
    }
    let boundaries: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(i, m)| *i > 0 && matches!(m, Message::User { .. }))
        .map(|(i, _)| i)
        .collect();
    let last = *boundaries.last()?;
    boundaries
        .iter()
        .copied()
        .find(|&k| fixed + tokens[k..].iter().sum::<usize>() <= budget.target())
        .or(Some(last))
}

/// 요약에 넣을 메시지 하나의 최대 글자 수(긴 코드·도구 결과는 앞만).
pub const SUMMARY_INPUT_CHARS: usize = 4_000;

/// 앞 대화를 요약하는 프롬프트(도구 없음). 앞에 이미 요약이 있으면 그것도 넣어 하나로 합친다.
pub fn summary_prompt(previous: Option<&Folded>, messages: &[Message]) -> Prompt {
    let clip = |t: &str| -> String {
        if t.chars().count() <= SUMMARY_INPUT_CHARS {
            t.to_owned()
        } else {
            let mut s: String = t.chars().take(SUMMARY_INPUT_CHARS).collect();
            s.push_str(" …");
            s
        }
    };
    let mut body = String::new();
    if let Some(Folded::Summary(s)) = previous {
        let _ = writeln!(body, "[Earlier summary]\n{}\n", clip(s));
    }
    for m in messages {
        match m {
            Message::User { text } => {
                let _ = writeln!(body, "[Candidate]\n{}\n", clip(text));
            }
            Message::Assistant { text, tool_calls } => {
                let _ = writeln!(body, "[Assistant]\n{}", clip(text));
                for c in tool_calls {
                    let _ = writeln!(body, "(called {} {})", c.name, clip(&py_json(&c.arguments)));
                }
                body.push('\n');
            }
            Message::ToolResults { results } => {
                for r in results {
                    let _ = writeln!(body, "[Tool result]\n{}\n", clip(r));
                }
            }
        }
    }
    let prefix = format!(
        "<|im_start|>system\nYou summarize conversations between a user taking a coding test and a coding assistant.<|im_end|>\n\
<|im_start|>user\nSummarize the conversation below in at most 200 words. Keep what the user asked, what was found about \
their code, which edits were proposed, accepted or rejected, and what is still open. Do not include code longer than one line.\n\n{}",
        neutralize(&body)
    );
    Prompt {
        prefix,
        suffix: format!("<|im_end|>\n{ASSISTANT_OPEN}"),
    }
}
