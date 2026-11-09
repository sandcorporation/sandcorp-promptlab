//! AI 도우미 칸(#98) — 응시 화면 오른쪽. 상태·도구 루프는 lib [`promptlab_app::assistant`], 여기는 그리기와 연결만:
//! 모델 올리기(처음 한 번, 별도 스레드), 질문 보내기([`assistant::ask`]), 제안 diff 띄우기·수락·거절(편집기 다리 `diff`·
//! `apply`·`diff_close`), 과정 로그(`note` — 시험만).
//!
//! - 문제마다 대화 하나: `Ui::chats`(키 = 시험 문항 `배정#순번` 또는 연습 문제 id). 새로 열기·닫기 없음.
//! - 입력칸은 비제어(`pl-ai-input`) — 시험이면 #97의 클립보드 가드가 문서 전체에 걸려 같은 규칙(앱 안 버퍼만)을 받는다.
//! - 모델이 답하는 동안에는 결정(수락·거절)을 받지 않는다 — 차례가 끝나면 대기 중인 제안의 diff를 편집기에 띄운다.
//! - 스트리밍·진행(#116): 답하는 동안 [`assistant::Live::view`]를 말풍선으로 그리고(도구 호출 JSON은 「코드 읽는 중」 같은
//!   상태로), 아래에 상태·생성 토큰·경과 초·「멈추기」를 둔다(경과 초는 `Ui::tick`으로 1초마다 다시 그린다).
//! - 자동 스크롤(#116): [`ScrollFollow`](lib) — 맨 아래 근처면 내용이 늘 때마다 내리고, 위로 올려 읽는 중이면 「새 메시지 ↓」.

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use promptlab_app::assistant::{self, Chat, Entry, Live, ProposalState, Step, ToolHost};
use promptlab_app::local::{Cancel, LocalModel};
use promptlab_app::localrun::LocalDocker;
use promptlab_app::workspace::{ScrollFollow, near_bottom};
use promptlab_inference::{LlamaInference, compute_info};
use promptlab_shared::ai::{Hidden, Tool};
use promptlab_shared::coding::{Language, PublicCodingProblem};
use serde_json::json;

use super::{Ui, note};

/// 앱이 쥐는 로컬 모델(앱 실행 동안 한 번 올린다).
#[derive(Clone)]
pub enum ModelSlot {
    Idle,
    Loading,
    Ready {
        model: LocalModel<LlamaInference>,
        /// GPU 이름(없으면 CPU).
        gpu: Option<String>,
        /// 그래픽 메모리가 모자라 GPU에 일부만 올렸다(다른 앱이 VRAM을 쓰는 중, #116) — 안내를 띄운다.
        gpu_short: bool,
    },
    Failed(String),
}

/// 그래픽 메모리가 모자라 GPU에 일부만 올렸을 때 AI 칸에 띄우는 안내(#116).
pub const GPU_SHORT_NOTICE: &str = "다른 앱이 그래픽 메모리를 쓰는 중이라 AI가 느릴 수 있습니다. 브라우저·게임 같은 앱을 닫고 응시 앱을 다시 켜면 빨라집니다.";

/// 입력칸 id(클립보드 가드의 붙여넣기 대상).
const INPUT_ID: &str = "pl-ai-input";

/// 대화 목록 id(자동 스크롤).
const LOG_ID: &str = "pl-ai-log";

/// 도구가 보는 응시 화면.
struct UiHost(Ui);

impl ToolHost for UiHost {
    fn problem(&self) -> Option<PublicCodingProblem> {
        self.0.session.read().problem().cloned()
    }

    fn code(&self) -> (Language, String) {
        // AI가 읽은 코드가 과정 로그에 있게 바로 전 스냅숏(바뀌었을 때만 남는다).
        self.0.snapshot(true);
        self.0
            .editor
            .read()
            .as_ref()
            .map(|e| (e.language(), e.code().to_owned()))
            .unwrap_or((Language::Python, String::new()))
    }

    async fn run_examples(&self) -> String {
        let Some((problem, (language, code))) = self.problem().map(|p| (p, self.code())) else {
            return "Error: no problem is open.".into();
        };
        let result = async {
            let docker = LocalDocker::detect()?;
            docker.run_examples(&problem, language, &code).await
        }
        .await;
        match result {
            Ok(r) => format!(
                "Result: {} ({}/{} examples passed)",
                r.summary(),
                r.passed(),
                r.total()
            ),
            Err(e) => format!("Error: could not run: {e}"),
        }
    }
}

impl Ui {
    /// 이 응시의 대화 열쇠.
    pub fn chat_key(&self) -> String {
        let s = self.session.read();
        match (s.exam_item(), s.problem()) {
            (Some(item), _) => format!("{}#{}", item.assignment_id, item.position),
            (None, Some(p)) => p.id.clone(),
            (None, None) => String::new(),
        }
    }

    /// 모델을 아직 안 올렸으면 올린다(파일 확인 sha256 + 적재 — 수십 초).
    pub fn ensure_model(&self) {
        let mut slot = self.model;
        if !matches!(*slot.read(), ModelSlot::Idle) {
            return;
        }
        slot.set(ModelSlot::Loading);
        let ui = self.clone();
        spawn_forever(async move {
            let path = ui.model_path();
            let next = if !ui.model_ready() {
                ModelSlot::Failed(
                    "AI 모델을 아직 받지 않았습니다. 응시 전 점검의 「AI 모델」에서 받아 주세요."
                        .into(),
                )
            } else {
                match LocalModel::load(path.clone()).await {
                    Ok(model) => ModelSlot::Ready {
                        gpu_short: model.inner().gpu_short(),
                        model,
                        gpu: compute_info().ok().and_then(|i| i.gpu),
                    },
                    Err(e) => ModelSlot::Failed(format!("AI 모델을 올리지 못했습니다: {e}")),
                }
            };
            // 올리는 동안 응시자가 크기를 바꿨으면(#131) 이 결과는 버린다 — 다음 `ensure_model`이 새 모델을 올린다.
            if matches!(*slot.read(), ModelSlot::Loading) && ui.model_path() == path {
                slot.set(next);
            }
        });
    }

    /// 응시자 메시지 하나로 한 차례를 돈다.
    pub fn ask_ai(&self, text: String) {
        let ModelSlot::Ready { model, .. } = self.model.read().clone() else {
            return;
        };
        let mut busy = self.ai_busy;
        if busy() || text.trim().is_empty() {
            return;
        }
        let key = self.chat_key();
        let mut chat = self.chats.read().get(&key).cloned().unwrap_or_default();
        busy.set(true);
        let cancel = Cancel::new();
        let mut cancel_slot = self.ai_cancel;
        cancel_slot.set(Some(cancel.clone()));
        let ui = self.clone();
        spawn_forever(async move {
            let host = UiHost(ui.clone());
            let (plog, mut chats) = (ui.plog, ui.chats);
            let _ = assistant::ask(
                &model,
                &mut chat,
                &host,
                &text,
                &cancel,
                &mut |step| match step {
                    Step::Event(e) => note(plog, e),
                    Step::Changed(c) => {
                        chats.write().insert(key.clone(), c.clone());
                    }
                },
            )
            .await;
            let pending = chat.pending().cloned();
            chats.write().insert(key, chat);
            cancel_slot.set(None);
            busy.set(false);
            if let Some(p) = pending {
                ui.show_diff(p.id);
            }
        });
    }

    /// 답하는 중인 차례를 멈춘다(그때까지의 답은 남는다).
    pub fn stop_ai(&self) {
        if let Some(c) = self.ai_cancel.read().as_ref() {
            c.cancel();
        }
    }

    /// 대기 중인 제안의 diff를 편집기에 띄운다.
    pub fn show_diff(&self, id: u32) {
        let key = self.chat_key();
        let chats = self.chats.read();
        let Some(p) = chats.get(&key).and_then(|c| c.proposal(id)) else {
            return;
        };
        if p.state != ProposalState::Pending {
            return;
        }
        let language = self
            .editor
            .read()
            .as_ref()
            .map(|e| e.language())
            .unwrap_or(Language::Python);
        self.to_editor(json!({
            "t": "diff",
            "id": id,
            "original": p.base,
            "modified": p.after,
            "language": language.as_str(),
            "lines": format!("{}–{}줄", p.lines.0, p.lines.1),
        }));
    }

    /// 제안을 받아들이거나 버린다(편집기 diff의 단추·AI 칸의 단추).
    pub fn decide(&self, id: u32, accept: bool) {
        let (mut toast, mut error) = (self.toast, self.error);
        if (self.ai_busy)() {
            toast.set(Some(
                "AI가 답하는 중입니다. 답이 끝난 뒤 수락·거절해 주세요.".into(),
            ));
            return;
        }
        let current = self
            .editor
            .read()
            .as_ref()
            .map(|e| e.code().to_owned())
            .unwrap_or_default();
        let key = self.chat_key();
        let mut chats = self.chats;
        let decided = chats
            .write()
            .get_mut(&key)
            .map(|c| c.decide(id, accept, &current));
        match decided {
            Some(Ok(d)) => {
                note(self.plog, d.event);
                match d.apply {
                    Some((r, text)) => self.to_editor(json!({
                        "t": "apply",
                        "range": [r.start_line, r.start_column, r.end_line, r.end_column],
                        "text": text,
                    })),
                    None => self.to_editor(json!({"t": "diff_close"})),
                }
            }
            Some(Err(e)) => error.set(Some(e.to_string())),
            None => {}
        }
    }
}

/// 입력칸의 글을 AI에게 보낸다(모델이 준비됐고 답하는 중이 아닐 때만).
fn send(ui: &Ui, mut draft: CopyValue<String>) {
    let text = draft.read().clone();
    if text.trim().is_empty()
        || !matches!(*ui.model.read(), ModelSlot::Ready { .. })
        || (ui.ai_busy)()
    {
        return;
    }
    draft.set(String::new());
    clear_input();
    ui.ask_ai(text);
}

/// 대화 목록을 맨 아래로 내린다(그린 다음 프레임에).
fn scroll_to_bottom() {
    document::eval(&format!(
        "requestAnimationFrame(() => {{ const el = document.getElementById('{LOG_ID}'); if (el) {{ el.scrollTop = el.scrollHeight; }} }});"
    ));
}

/// 진행 줄의 상태 글.
fn live_status(live: &Live) -> &'static str {
    if live.summarizing {
        return "앞 대화를 요약하는 중";
    }
    match live.view().hidden {
        Some(Hidden::Call(Some(Tool::ReadCode))) => "코드를 읽으려는 중",
        Some(Hidden::Call(Some(Tool::ReadProblem))) => "문제를 읽으려는 중",
        Some(Hidden::Call(Some(Tool::ProposeEdit))) => "편집 제안을 쓰는 중",
        Some(Hidden::Call(Some(Tool::RunExamples))) => "예제를 실행하려는 중",
        Some(Hidden::Call(None)) => "도구를 부르는 중",
        Some(Hidden::Undecided) => "답을 정리하는 중",
        None if live.raw.is_empty() => "질문과 코드를 읽는 중",
        None => "답을 쓰는 중",
    }
}

/// 입력칸을 비운다(비제어라 DOM에서).
fn clear_input() {
    document::eval(&format!(
        "const el = document.getElementById('{INPUT_ID}'); if (el) {{ el.value = ''; el.focus(); }}"
    ));
}

#[component]
pub fn AiPane() -> Element {
    let ui = use_context::<Ui>();
    {
        let ui = ui.clone();
        use_hook(move || ui.ensure_model());
    }
    let mut toast = ui.toast;
    let mut error = ui.error;
    let guarded = ui.guarded();
    let busy = (ui.ai_busy)();
    let slot = ui.model.read().clone();
    let key = ui.chat_key();
    let chat = ui.chats.read().get(&key).cloned().unwrap_or_else(Chat::new);
    // 비제어 입력칸의 지금 글(그리기에 쓰지 않는다).
    let mut draft = use_hook(|| CopyValue::new(String::new()));
    let ready = matches!(slot, ModelSlot::Ready { .. });
    // 진행 중이면 1초마다 다시 그린다(경과 초).
    if busy {
        let _ = (ui.tick)();
    }
    let live = chat.live().cloned();
    let live_view = live.as_ref().map(Live::view);

    // 자동 스크롤: 내용(줄 수·스트리밍 글 길이·토큰)이 바뀔 때마다 따라갈지 정한다.
    let mut follow = use_signal(ScrollFollow::default);
    let content = (
        chat.entries().len(),
        live_view.as_ref().map_or(0, |v| v.text.len()),
        live.as_ref().map_or(0, |l| l.tokens),
        busy,
    );
    use_effect(use_reactive!(|content| {
        let _ = content;
        if follow.write().on_content() {
            scroll_to_bottom();
        }
    }));
    let unseen = follow.read().unseen();

    let send_click = {
        let ui = ui.clone();
        move |_| {
            follow.write().jump();
            send(&ui, draft)
        }
    };
    let on_key = {
        let ui = ui.clone();
        move |e: KeyboardEvent| {
            if e.key() == Key::Enter && !e.modifiers().shift() && !e.is_composing() {
                e.prevent_default();
                follow.write().jump();
                send(&ui, draft);
            }
        }
    };
    let stop_click = {
        let ui = ui.clone();
        move |_| ui.stop_ai()
    };
    let stopping = busy
        && ui
            .ai_cancel
            .read()
            .as_ref()
            .is_some_and(Cancel::is_cancelled);

    let (badge_class, badge_text) = match &slot {
        ModelSlot::Ready { gpu: Some(_), .. } => ("badge ok", "GPU".to_owned()),
        ModelSlot::Ready { gpu: None, .. } => ("badge", "CPU".to_owned()),
        ModelSlot::Loading | ModelSlot::Idle => ("badge warn", "준비 중".to_owned()),
        ModelSlot::Failed(_) => ("badge err", "꺼짐".to_owned()),
    };

    rsx! {
        aside { class: "pane ai", "aria-label": "AI 도우미",
            if let Some(t) = toast() {
                div { class: "infobar warn", role: "status",
                    span { class: "grow", "{t}" }
                    button { class: "link", onclick: move |_| toast.set(None), "닫기" }
                }
            }
            if let Some(e) = error() {
                div { class: "infobar error", role: "alert",
                    span { class: "grow", "{e}" }
                    button { class: "link", onclick: move |_| error.set(None), "닫기" }
                }
            }
            div { class: "ai-head",
                h3 { "AI 도우미" }
                span { class: "{badge_class}", title: "AI 속도", "{badge_text}" }
            }
            match &slot {
                ModelSlot::Loading | ModelSlot::Idle => rsx! {
                    p { class: "sub", "AI 모델을 확인하고 올리는 중입니다(수십 초). 그동안 코드를 써도 됩니다." }
                },
                ModelSlot::Failed(m) => rsx! { div { class: "infobar warn", role: "status", "{m}" } },
                ModelSlot::Ready { gpu_short: true, model, .. } => {
                    // 더 작은 후보가 있으면 바꾸라고 덧붙인다(#131 — 바꾸기는 응시 전 점검에서만).
                    let hint = promptlab_shared::inference::model_by_sha256(model.sha256())
                        .and_then(promptlab_app::model_pick::smaller_model_hint)
                        .map(|h| format!(" {h}"))
                        .unwrap_or_default();
                    rsx! { div { class: "infobar warn", role: "status", "{GPU_SHORT_NOTICE}{hint}" } }
                }
                ModelSlot::Ready { .. } => rsx! {},
            }
            div { class: "ai-log-wrap",
                div {
                    id: LOG_ID,
                    class: "ai-log",
                    role: "log",
                    "aria-live": "polite",
                    onscroll: move |e| {
                        let near = near_bottom(
                            e.scroll_top(),
                            f64::from(e.scroll_height()),
                            f64::from(e.client_height()),
                        );
                        if follow.peek().following() != near || (near && follow.peek().unseen()) {
                            follow.write().on_scroll(near);
                        }
                    },
                    if chat.entries().is_empty() && ready {
                        p { class: "sub", "문제나 코드에 대해 물어보세요. AI는 문제·고른 언어·지금 코드를 보고 답하고, 코드를 바꿀 때는 편집 제안(diff)을 보냅니다 — 수락해야 편집기에 들어갑니다." }
                    }
                    for (i, entry) in chat.entries().iter().enumerate() {
                        {render_entry(&ui, &chat, i, entry, busy)}
                    }
                    if let Some(v) = live_view.as_ref().filter(|v| !v.text.is_empty()) {
                        div { class: "bubble ai live", "{v.text}" }
                    }
                }
                if unseen {
                    button {
                        class: "ai-new",
                        onclick: move |_| {
                            follow.write().jump();
                            scroll_to_bottom();
                        },
                        "새 메시지 ↓"
                    }
                }
            }
            if let Some(l) = live.as_ref() {
                div { class: "ai-progress", role: "status",
                    span { class: "ai-spinner", "aria-hidden": "true" }
                    span { class: "grow",
                        if stopping { "멈추는 중…" } else { "{live_status(l)}…" }
                    }
                    span { class: "ai-count", "토큰 {l.tokens} · {l.started.elapsed().as_secs()}초" }
                    button { disabled: stopping, onclick: stop_click, "멈추기" }
                }
            }
            div { class: "ai-input",
                textarea {
                    id: INPUT_ID,
                    rows: 3,
                    placeholder: if ready { "메시지 (Enter 보내기 · Shift+Enter 줄바꿈)" } else { "AI 모델을 기다리는 중" },
                    disabled: !ready,
                    oninput: move |e| draft.set(e.value()),
                    onkeydown: on_key,
                }
                button { class: "accent", disabled: !ready || busy, onclick: send_click, "보내기" }
            }
            if guarded {
                p { class: "sub", "시험 중에는 앱 밖에서 복사한 글을 붙여 넣을 수 없습니다. 앱 안에서 복사한 글은 붙여 넣을 수 있습니다. AI와의 대화는 과정 기록에 남습니다." }
            }
        }
    }
}

fn render_entry(ui: &Ui, chat: &Chat, i: usize, entry: &Entry, busy: bool) -> Element {
    match entry {
        Entry::User(t) => rsx! { div { key: "{i}", class: "bubble me", "{t}" } },
        Entry::Assistant(t) => rsx! { div { key: "{i}", class: "bubble ai", "{t}" } },
        Entry::Tool(t) => rsx! { p { key: "{i}", class: "ai-tool", "· {t}" } },
        Entry::Folded(true) => {
            rsx! { p { key: "{i}", class: "ai-fold", "앞 대화를 요약해 AI에게 넘겼습니다(기록에는 원문 그대로 남습니다)." } }
        }
        Entry::Folded(false) => {
            rsx! { p { key: "{i}", class: "ai-fold", "앞 대화 생략 — 대화가 길어 AI가 앞부분을 보지 못합니다." } }
        }
        Entry::Stopped => {
            rsx! { p { key: "{i}", class: "ai-fold", "응시자가 답을 멈췄습니다." } }
        }
        Entry::Empty => {
            rsx! { p { key: "{i}", class: "ai-fold", "AI가 답을 만들지 못했습니다 — 질문을 조금 바꿔 다시 물어 주세요." } }
        }
        Entry::Error(e) => {
            rsx! { div { key: "{i}", class: "infobar error", role: "alert", "AI 답을 만들지 못했습니다: {e}" } }
        }
        Entry::Proposal(id) => {
            let Some(p) = chat.proposal(*id) else {
                return rsx! {};
            };
            let id = *id;
            let (u1, u2, u3) = (ui.clone(), ui.clone(), ui.clone());
            let added = p.text.lines().count();
            rsx! {
                div { key: "{i}", class: "card proposal",
                    div { class: "row-line",
                        strong { "편집 제안 #{id}" }
                        span { class: "sub", "{p.lines.0}–{p.lines.1}줄 → {added}줄" }
                        match p.state {
                            ProposalState::Pending => rsx! { span { class: "badge warn", "결정 대기" } },
                            ProposalState::Accepted => rsx! { span { class: "badge ok", "수락함" } },
                            ProposalState::Rejected => rsx! { span { class: "badge", "거절함" } },
                        }
                    }
                    if let Some(w) = p.warning {
                        div { class: "infobar warn", role: "status", "{w.warning()}" }
                    }
                    if p.state == ProposalState::Pending {
                        div { class: "row-line",
                            button { onclick: move |_| u1.show_diff(id), "diff 보기" }
                            button { class: "accent", disabled: busy, onclick: move |_| u2.decide(id, true), "수락" }
                            button { disabled: busy, onclick: move |_| u3.decide(id, false), "거절" }
                        }
                    }
                }
            }
        }
    }
}
