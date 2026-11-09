//! Python 3.12 틀·하네스.

use super::codegen::quoted_types;
use super::{Scalar, Signature, ValueType};

fn ty(t: ValueType) -> String {
    let base = match t.scalar() {
        Scalar::Int => "int",
        Scalar::Double => "float",
        Scalar::Bool => "bool",
        Scalar::String => "str",
    };
    match t.dims() {
        0 => base.to_string(),
        1 => format!("list[{base}]"),
        _ => format!("list[list[{base}]]"),
    }
}

pub(super) fn template(sig: &Signature) -> String {
    let params = sig
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, ty(p.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    let default = match (sig.returns.dims(), sig.returns.scalar()) {
        (0, Scalar::Int) => "0",
        (0, Scalar::Double) => "0.0",
        (0, Scalar::Bool) => "False",
        (0, Scalar::String) => "\"\"",
        _ => "[]",
    };
    format!(
        "def {name}({params}) -> {ret}:\n    answer = {default}\n    return answer\n",
        name = sig.name,
        ret = ty(sig.returns)
    )
}

const HARNESS: &str = r#"import json
import math
import os
import sys
import threading

PL_NAME = "__NAME__"
PL_PARAMS = [__PARAMS__]
PL_RETURNS = "__RETURNS__"


def pl_conv(t, v):
    if t.endswith("[]"):
        if not isinstance(v, (list, tuple)):
            raise TypeError("값이 배열이 아니다")
        return [pl_conv(t[:-2], x) for x in v]
    if t == "int":
        if isinstance(v, bool) or not isinstance(v, int):
            raise TypeError("값이 int가 아니다: " + type(v).__name__)
        return v
    if t == "double":
        if isinstance(v, bool) or not isinstance(v, (int, float)):
            raise TypeError("값이 double이 아니다: " + type(v).__name__)
        f = float(v)
        if not math.isfinite(f):
            raise ValueError("double 값이 유한하지 않다")
        return f
    if t == "bool":
        if not isinstance(v, bool):
            raise TypeError("값이 bool이 아니다: " + type(v).__name__)
        return v
    if not isinstance(v, str):
        raise TypeError("값이 string이 아니다: " + type(v).__name__)
    return v


def pl_main():
    sys.setrecursionlimit(1000000)
    out = os.fdopen(3, "w", encoding="utf-8")

    def emit(line):
        out.write(line)
        out.write("\n")
        out.flush()

    def runtime(msg):
        emit(json.dumps({"error": "runtime", "message": msg[:1000]}, ensure_ascii=False))

    cases = json.loads(sys.stdin.buffer.read())
    load_error = None
    fn = None
    try:
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        import solution

        fn = getattr(solution, PL_NAME, None)
        if not callable(fn):
            load_error = "함수 " + PL_NAME + "을(를) 찾지 못했다"
    except BaseException as e:
        load_error = type(e).__name__ + ": " + str(e)
    emit('{"ready":true}')
    for args in cases:
        if load_error is not None:
            runtime(load_error)
            continue
        try:
            conv = [pl_conv(t, a) for t, a in zip(PL_PARAMS, args)]
            result = fn(*conv)
            line = '{"ok":' + json.dumps(
                pl_conv(PL_RETURNS, result), ensure_ascii=False, allow_nan=False, separators=(",", ":")
            ) + "}"
            del result, conv
        except MemoryError:
            line = '{"error":"memory"}'
        except BaseException as e:
            line = json.dumps({"error": "runtime", "message": (type(e).__name__ + ": " + str(e))[:1000]}, ensure_ascii=False)
        emit(line)


threading.stack_size(64 * 1024 * 1024)
pl_thread = threading.Thread(target=pl_main)
pl_thread.start()
pl_thread.join()
sys.stdout.flush()
os._exit(0)
"#;

pub(super) fn harness(sig: &Signature) -> String {
    HARNESS
        .replace("__NAME__", &sig.name)
        .replace(
            "__PARAMS__",
            &quoted_types(sig.params.iter().map(|p| &p.ty)),
        )
        .replace("__RETURNS__", sig.returns.as_str())
}
