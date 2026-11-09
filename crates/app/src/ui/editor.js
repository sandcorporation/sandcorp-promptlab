// PromptLab 편집기 다리(#97). Rust가 dioxus eval로 한 번 넣는다 — 주고받기는 dioxus.send(…) / await dioxus.recv().
// Monaco는 앱에 구운 자산(/monaco/…)에서 읽는다(외부 요청 없음). 받는 말: init·set·insert·readonly·focus.
// 보내는 말: ready·change(코드)·key(단축키 + 코드)·copy(앱 안 복사 글)·paste(OS 클립보드 글·대상)·error.
// 시험(guard)이면 복사·잘라내기·붙여넣기·끌어 놓기를 문서 전체에서 가로챈다: OS 클립보드에 쓰지 않고 Rust의 앱 안
// 버퍼(promptlab_app::editor::Clipboard)가 무엇을 넣을지 정한다.
const send = (m) => dioxus.send(m);
const init = await dioxus.recv();
window.__plSend = send;
window.__plGuard = !!init.guard;

let host = null;
for (let i = 0; i < 200 && !host; i++) {
  host = document.getElementById("pl-editor");
  if (!host) await new Promise((r) => setTimeout(r, 25));
}
if (!host) {
  send({ t: "error", message: "편집기 자리를 찾지 못했다" });
  return;
}

const base = new URL("/monaco/", location.href).href;
function loadScript(src) {
  return new Promise((ok, fail) => {
    const s = document.createElement("script");
    s.src = src;
    s.onload = ok;
    s.onerror = () => fail(new Error("불러오지 못함: " + src));
    document.head.appendChild(s);
  });
}
try {
  if (!window.monaco) {
    window.__plLoader = window.__plLoader || loadScript(base + "vs/loader.js");
    await window.__plLoader;
    require.config({ paths: { vs: base + "vs" }, "vs/nls": { availableLanguages: { "*": "ko" } } });
    await new Promise((ok, fail) => require(["vs/editor/editor.main"], ok, fail));
  }
} catch (e) {
  send({ t: "error", message: "편집기를 불러오지 못했다: " + e });
  return;
}

const guard = !!init.guard;
const dark = matchMedia("(prefers-color-scheme: dark)");
const theme = () => (dark.matches ? "vs-dark" : "vs");
host.textContent = "";
const editor = monaco.editor.create(host, {
  value: init.code,
  language: init.language,
  theme: theme(),
  automaticLayout: true,
  minimap: { enabled: false },
  fontSize: 14,
  fontFamily: "Consolas, 'Cascadia Mono', 'D2Coding', 'Malgun Gothic', monospace",
  tabSize: 4,
  insertSpaces: true,
  scrollBeyondLastLine: false,
  wordBasedSuggestions: "off",
  emptySelectionClipboard: false,
  // 시험에서는 Monaco 메뉴의 붙여넣기(navigator.clipboard)와 끌어 놓기를 끈다 — 아래 가로채기만 길이다.
  contextmenu: !guard,
  dragAndDrop: !guard,
  readOnly: !!init.readOnly,
});
dark.addEventListener("change", () => monaco.editor.setTheme(theme()));
window.__plEditor = editor;

let timer = null;
const flush = () => {
  clearTimeout(timer);
  timer = null;
  send({ t: "change", code: editor.getValue() });
};
editor.onDidChangeModelContent(() => {
  clearTimeout(timer);
  timer = setTimeout(flush, 300);
});
editor.onDidBlurEditorText(() => {
  if (timer) flush();
});
const key = (name) => () => send({ t: "key", name, code: editor.getValue() });
editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, key("examples"));
editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.Enter, key("run"));

// 클립보드 가로채기는 문서에 한 번만 건다(편집기를 다시 만들어도 window.__plSend·__plEditor가 지금 것을 가리킨다).
if (!window.__plClipInstalled) {
  window.__plClipInstalled = true;
  const inEditor = (t) => {
    const h = document.getElementById("pl-editor");
    return !!(h && t && h.contains(t));
  };
  const isField = (t) => t && (t.tagName === "TEXTAREA" || t.tagName === "INPUT");
  const onCopy = (e, cut) => {
    if (!window.__plGuard) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    const t = e.target;
    let text = "";
    const ed = window.__plEditor;
    if (ed && inEditor(t)) {
      const sel = ed.getSelection();
      text = sel ? ed.getModel().getValueInRange(sel) : "";
      if (cut && text) {
        ed.executeEdits("pl-cut", [{ range: sel, text: "" }]);
        ed.pushUndoStop();
      }
    } else if (isField(t)) {
      text = t.value.substring(t.selectionStart || 0, t.selectionEnd || 0);
      if (cut && text && !t.readOnly) document.execCommand("delete");
    } else {
      text = String(window.getSelection() || "");
    }
    if (text) window.__plSend({ t: "copy", text });
  };
  document.addEventListener("copy", (e) => onCopy(e, false), true);
  document.addEventListener("cut", (e) => onCopy(e, true), true);
  document.addEventListener(
    "paste",
    (e) => {
      if (!window.__plGuard) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      const t = e.target;
      const target = inEditor(t) ? "editor" : isField(t) && t.id ? t.id : "";
      const ext = (e.clipboardData && e.clipboardData.getData("text/plain")) || "";
      window.__plSend({ t: "paste", ext, target });
    },
    true,
  );
  document.addEventListener(
    "drop",
    (e) => {
      if (!window.__plGuard) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      const ext = (e.dataTransfer && e.dataTransfer.getData("text/plain")) || "";
      window.__plSend({ t: "paste", ext, target: "" });
    },
    true,
  );
}

// AI 편집 제안 diff 덮개(#98): 편집기 자리 위에 diff 편집기(읽기 전용)와 머리 막대(수락·거절)를 띄운다.
let diffBox = null;
const closeDiff = () => {
  if (!diffBox) return;
  diffBox.editor.dispose();
  diffBox.original.dispose();
  diffBox.modified.dispose();
  diffBox.el.remove();
  diffBox = null;
};
const showDiff = (m) => {
  closeDiff();
  const el = document.createElement("div");
  el.className = "diff-overlay";
  el.setAttribute("role", "dialog");
  el.setAttribute("aria-label", "AI 편집 제안 " + m.id);
  const head = document.createElement("div");
  head.className = "diff-head";
  const title = document.createElement("strong");
  title.textContent = "AI 제안 #" + m.id;
  const lines = document.createElement("span");
  lines.className = "sub grow";
  lines.textContent = m.lines || "";
  const button = (label, accent, accept) => {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = label;
    if (accent) b.className = "accent";
    b.addEventListener("click", () => send({ t: "decide", id: m.id, accept }));
    return b;
  };
  const hide = document.createElement("button");
  hide.type = "button";
  hide.className = "link";
  hide.textContent = "닫기";
  hide.title = "제안은 AI 칸에 남습니다";
  hide.addEventListener("click", closeDiff);
  head.append(title, lines, button("수락", true, true), button("거절", false, false), hide);
  const body = document.createElement("div");
  body.className = "diff-body";
  el.append(head, body);
  host.appendChild(el);
  const original = monaco.editor.createModel(m.original, m.language);
  const modified = monaco.editor.createModel(m.modified, m.language);
  const diff = monaco.editor.createDiffEditor(body, {
    theme: theme(),
    automaticLayout: true,
    readOnly: true,
    originalEditable: false,
    renderSideBySide: false,
    minimap: { enabled: false },
    fontSize: 14,
    scrollBeyondLastLine: false,
  });
  diff.setModel({ original, modified });
  diffBox = { el, editor: diff, original, modified };
};

send({ t: "ready" });

while (true) {
  const m = await dioxus.recv();
  if (m.t === "set") {
    monaco.editor.setModelLanguage(editor.getModel(), m.language);
    editor.setValue(m.code);
    editor.focus();
  } else if (m.t === "insert") {
    if (m.target === "editor") {
      editor.focus();
      const edits = editor.getSelections().map((r) => ({ range: r, text: m.text, forceMoveMarkers: true }));
      editor.executeEdits("pl-paste", edits);
      editor.pushUndoStop();
    } else {
      const el = document.getElementById(m.target);
      if (el) {
        el.focus();
        document.execCommand("insertText", false, m.text);
      }
    }
  } else if (m.t === "readonly") {
    editor.updateOptions({ readOnly: !!m.on });
  } else if (m.t === "focus") {
    editor.focus();
  } else if (m.t === "diff") {
    showDiff(m);
  } else if (m.t === "diff_close") {
    closeDiff();
  } else if (m.t === "apply") {
    // 수락한 제안: 덮개를 닫고 범위를 바꾼 뒤 바로 change를 보낸다(스냅숏이 따른다). 되돌리기(Ctrl+Z) 한 번으로 돌아간다.
    closeDiff();
    const [sl, sc, el, ec] = m.range;
    editor.pushUndoStop();
    editor.executeEdits("pl-ai", [
      { range: new monaco.Range(sl, sc, el, ec), text: m.text, forceMoveMarkers: true },
    ]);
    editor.pushUndoStop();
    editor.revealLineInCenter(sl);
    editor.focus();
    flush();
  } else if (m.t === "guard") {
    window.__plGuard = !!m.on;
  }
}
