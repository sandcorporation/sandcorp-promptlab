//! JavaScript(Node 22) 틀·하네스. 풀이는 워커 스레드에서 돈다 — 메모리 초과(`ERR_WORKER_OUT_OF_MEMORY`)를
//! 본 스레드가 잡아 그 케이스만 「메모리 초과」로 하고 남은 케이스로 워커를 다시 띄운다.

use super::codegen::quoted_types;
use super::{Scalar, Signature, ValueType};

fn ty(t: ValueType) -> String {
    let base = match t.scalar() {
        Scalar::Int | Scalar::Double => "number",
        Scalar::Bool => "boolean",
        Scalar::String => "string",
    };
    format!("{base}{}", "[]".repeat(t.dims() as usize))
}

pub(super) fn template(sig: &Signature) -> String {
    let mut s = String::from("/**\n");
    for p in &sig.params {
        s.push_str(&format!(" * @param {{{}}} {}\n", ty(p.ty), p.name));
    }
    s.push_str(&format!(" * @return {{{}}}\n */\n", ty(sig.returns)));
    let params = sig
        .params
        .iter()
        .map(|p| p.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let default = match (sig.returns.dims(), sig.returns.scalar()) {
        (0, Scalar::Int | Scalar::Double) => "0",
        (0, Scalar::Bool) => "false",
        (0, Scalar::String) => "\"\"",
        _ => "[]",
    };
    s.push_str(&format!(
        "function {}({params}) {{\n  let answer = {default};\n  return answer;\n}}\n",
        sig.name
    ));
    s
}

const HARNESS: &str = r#""use strict";
const fs = require("fs");
const path = require("path");
const { Worker, isMainThread, parentPort, workerData } = require("worker_threads");

const PL_NAME = "__NAME__";
const PL_PARAMS = [__PARAMS__];
const PL_RETURNS = "__RETURNS__";
const PL_MEMORY_MB = __MEMORY__;

function plEmit(line) {
  const buf = Buffer.from(line + "\n", "utf8");
  let off = 0;
  while (off < buf.length) off += fs.writeSync(3, buf, off, buf.length - off);
}

function plConv(t, v) {
  if (t.endsWith("[]")) {
    if (!Array.isArray(v)) throw new TypeError("값이 배열이 아니다");
    return v.map((x) => plConv(t.slice(0, -2), x));
  }
  if (t === "int") {
    if (typeof v === "bigint" && v >= -9007199254740991n && v <= 9007199254740991n) return Number(v);
    if (!Number.isSafeInteger(v)) throw new TypeError("값이 int(안전 정수)가 아니다: " + String(v));
    return v;
  }
  if (t === "double") {
    if (typeof v !== "number" || !Number.isFinite(v)) throw new TypeError("값이 유한한 double이 아니다: " + String(v));
    return v;
  }
  if (t === "bool") {
    if (typeof v !== "boolean") throw new TypeError("값이 bool이 아니다: " + typeof v);
    return v;
  }
  if (typeof v !== "string") throw new TypeError("값이 string이 아니다: " + typeof v);
  return v;
}

function plRuntime(msg) {
  return JSON.stringify({ error: "runtime", message: String(msg).slice(0, 1000) });
}

if (isMainThread) {
  const cases = JSON.parse(fs.readFileSync(0, "utf8"));
  plEmit('{"ready":true}');
  let next = 0;
  const run = () => {
    if (next >= cases.length) return;
    const w = new Worker(__filename, {
      workerData: cases.slice(next),
      stdout: false,
      stderr: false,
      resourceLimits: { maxOldGenerationSizeMb: PL_MEMORY_MB, stackSizeMb: 64 },
    });
    w.on("message", (line) => {
      plEmit(line);
      next += 1;
    });
    w.on("error", (e) => {
      if (next >= cases.length) return;
      plEmit(e && e.code === "ERR_WORKER_OUT_OF_MEMORY" ? '{"error":"memory"}' : plRuntime(e && e.stack ? e.stack : e));
      next += 1;
    });
    w.on("exit", () => run());
  };
  run();
} else {
  let fn;
  let loadError = null;
  try {
    const src = fs.readFileSync(path.join(__dirname, "solution.js"), "utf8");
    fn = new Function("require", src + "\n;return typeof " + PL_NAME + " === \"function\" ? " + PL_NAME + " : undefined;")(require);
    if (typeof fn !== "function") loadError = "함수 " + PL_NAME + "을(를) 찾지 못했다";
  } catch (e) {
    loadError = String(e && e.stack ? e.stack : e);
  }
  for (const args of workerData) {
    let line;
    if (loadError !== null) {
      line = plRuntime(loadError);
    } else {
      try {
        const conv = PL_PARAMS.map((t, i) => plConv(t, args[i]));
        line = '{"ok":' + JSON.stringify(plConv(PL_RETURNS, fn(...conv))) + "}";
      } catch (e) {
        line = plRuntime(e instanceof Error ? e.name + ": " + e.message : e);
      }
    }
    parentPort.postMessage(line);
  }
}
"#;

pub(super) fn harness(sig: &Signature) -> String {
    HARNESS
        .replace("__NAME__", &sig.name)
        .replace(
            "__PARAMS__",
            &quoted_types(sig.params.iter().map(|p| &p.ty)),
        )
        .replace("__RETURNS__", sig.returns.as_str())
        .replace("__MEMORY__", &super::MEMORY_LIMIT_MB.to_string())
}
