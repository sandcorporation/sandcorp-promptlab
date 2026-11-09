//! Java 21 틀·하네스. `int`는 `long`. 하네스는 JDK 밖 라이브러리 없이 작은 JSON 읽기·쓰기를 품는다.

use super::{Scalar, Signature, ValueType};

fn ty(t: ValueType) -> String {
    let base = match t.scalar() {
        Scalar::Int => "long",
        Scalar::Double => "double",
        Scalar::Bool => "boolean",
        Scalar::String => "String",
    };
    format!("{base}{}", "[]".repeat(t.dims() as usize))
}

fn default_value(t: ValueType) -> String {
    match t {
        ValueType::Int => "0".into(),
        ValueType::Double => "0.0".into(),
        ValueType::Bool => "false".into(),
        ValueType::String => "\"\"".into(),
        _ => format!("new {}{{}}", ty(t)),
    }
}

pub(super) fn template(sig: &Signature) -> String {
    let params = sig
        .params
        .iter()
        .map(|p| format!("{} {}", ty(p.ty), p.name))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "class Solution {{\n    public {ret} {name}({params}) {{\n        {ret} answer = {def};\n        return answer;\n    }}\n}}\n",
        ret = ty(sig.returns),
        name = sig.name,
        def = default_value(sig.returns)
    )
}

/// 값 변환 함수 이름(하네스 안 `PlJson`의 정적 메서드).
fn conv_fn(t: ValueType) -> &'static str {
    match t {
        ValueType::Int => "toLong",
        ValueType::Double => "toDouble",
        ValueType::Bool => "toBool",
        ValueType::String => "toStr",
        ValueType::IntArray => "toLongArr",
        ValueType::DoubleArray => "toDoubleArr",
        ValueType::BoolArray => "toBoolArr",
        ValueType::StringArray => "toStrArr",
        ValueType::IntMatrix => "toLongMat",
        ValueType::StringMatrix => "toStrMat",
    }
}

const HARNESS: &str = r#"import java.io.FileOutputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

public class PromptlabMain {
    static final class PlJson {
        final String s;
        int i;

        PlJson(String s) { this.s = s; }

        void ws() { while (i < s.length() && Character.isWhitespace(s.charAt(i))) i++; }

        Object value() {
            ws();
            char c = s.charAt(i);
            if (c == '[') {
                i++;
                List<Object> a = new ArrayList<>();
                ws();
                if (s.charAt(i) == ']') { i++; return a; }
                while (true) {
                    a.add(value());
                    ws();
                    char d = s.charAt(i++);
                    if (d == ']') return a;
                    if (d != ',') throw new IllegalStateException("JSON");
                }
            }
            if (c == '"') return str();
            if (s.startsWith("true", i)) { i += 4; return Boolean.TRUE; }
            if (s.startsWith("false", i)) { i += 5; return Boolean.FALSE; }
            if (s.startsWith("null", i)) { i += 4; return null; }
            int st = i;
            boolean dbl = false;
            while (i < s.length() && "+-0123456789.eE".indexOf(s.charAt(i)) >= 0) {
                char d = s.charAt(i++);
                if (d == '.' || d == 'e' || d == 'E') dbl = true;
            }
            String n = s.substring(st, i);
            if (dbl) return Double.parseDouble(n);
            return Long.parseLong(n);
        }

        String str() {
            i++;
            StringBuilder b = new StringBuilder();
            while (true) {
                char c = s.charAt(i++);
                if (c == '"') return b.toString();
                if (c != '\\') { b.append(c); continue; }
                char e = s.charAt(i++);
                switch (e) {
                    case 'n': b.append('\n'); break;
                    case 't': b.append('\t'); break;
                    case 'r': b.append('\r'); break;
                    case 'b': b.append('\b'); break;
                    case 'f': b.append('\f'); break;
                    case 'u': b.append((char) Integer.parseInt(s.substring(i, i + 4), 16)); i += 4; break;
                    default: b.append(e);
                }
            }
        }

        static List<?> arr(Object o) { return (List<?>) o; }
        static long toLong(Object o) { return ((Number) o).longValue(); }
        static double toDouble(Object o) { return ((Number) o).doubleValue(); }
        static boolean toBool(Object o) { return (Boolean) o; }
        static String toStr(Object o) { return (String) o; }
        static long[] toLongArr(Object o) { List<?> a = arr(o); long[] r = new long[a.size()]; for (int k = 0; k < r.length; k++) r[k] = toLong(a.get(k)); return r; }
        static double[] toDoubleArr(Object o) { List<?> a = arr(o); double[] r = new double[a.size()]; for (int k = 0; k < r.length; k++) r[k] = toDouble(a.get(k)); return r; }
        static boolean[] toBoolArr(Object o) { List<?> a = arr(o); boolean[] r = new boolean[a.size()]; for (int k = 0; k < r.length; k++) r[k] = toBool(a.get(k)); return r; }
        static String[] toStrArr(Object o) { List<?> a = arr(o); String[] r = new String[a.size()]; for (int k = 0; k < r.length; k++) r[k] = toStr(a.get(k)); return r; }
        static long[][] toLongMat(Object o) { List<?> a = arr(o); long[][] r = new long[a.size()][]; for (int k = 0; k < r.length; k++) r[k] = toLongArr(a.get(k)); return r; }
        static String[][] toStrMat(Object o) { List<?> a = arr(o); String[][] r = new String[a.size()][]; for (int k = 0; k < r.length; k++) r[k] = toStrArr(a.get(k)); return r; }

        static void w(StringBuilder b, long v) { b.append(v); }
        static void w(StringBuilder b, double v) { if (Double.isFinite(v)) b.append(v); else b.append("null"); }
        static void w(StringBuilder b, boolean v) { b.append(v); }
        static void w(StringBuilder b, String v) {
            if (v == null) { b.append("null"); return; }
            b.append('"');
            for (int k = 0; k < v.length(); k++) {
                char c = v.charAt(k);
                if (c == '"') b.append("\\\"");
                else if (c == '\\') b.append("\\\\");
                else if (c < 0x20) b.append(String.format("\\u%04x", (int) c));
                else b.append(c);
            }
            b.append('"');
        }
        static void w(StringBuilder b, long[] v) { if (v == null) { b.append("null"); return; } b.append('['); for (int k = 0; k < v.length; k++) { if (k > 0) b.append(','); w(b, v[k]); } b.append(']'); }
        static void w(StringBuilder b, double[] v) { if (v == null) { b.append("null"); return; } b.append('['); for (int k = 0; k < v.length; k++) { if (k > 0) b.append(','); w(b, v[k]); } b.append(']'); }
        static void w(StringBuilder b, boolean[] v) { if (v == null) { b.append("null"); return; } b.append('['); for (int k = 0; k < v.length; k++) { if (k > 0) b.append(','); w(b, v[k]); } b.append(']'); }
        static void w(StringBuilder b, String[] v) { if (v == null) { b.append("null"); return; } b.append('['); for (int k = 0; k < v.length; k++) { if (k > 0) b.append(','); w(b, v[k]); } b.append(']'); }
        static void w(StringBuilder b, long[][] v) { if (v == null) { b.append("null"); return; } b.append('['); for (int k = 0; k < v.length; k++) { if (k > 0) b.append(','); w(b, v[k]); } b.append(']'); }
        static void w(StringBuilder b, String[][] v) { if (v == null) { b.append("null"); return; } b.append('['); for (int k = 0; k < v.length; k++) { if (k > 0) b.append(','); w(b, v[k]); } b.append(']'); }
    }

    static OutputStream plOut;

    static void plEmit(String line) {
        try {
            plOut.write((line + "\n").getBytes(StandardCharsets.UTF_8));
            plOut.flush();
        } catch (java.io.IOException e) {
            Runtime.getRuntime().halt(3);
        }
    }

    static void plRuntime(Throwable e) {
        String m = e.toString();
        if (m.length() > 1000) m = m.substring(0, 1000);
        StringBuilder b = new StringBuilder("{\"error\":\"runtime\",\"message\":");
        PlJson.w(b, m);
        plEmit(b.append('}').toString());
    }

    static void plRun(List<?> cases) {
        for (Object c : cases) {
            List<?> args = (List<?>) c;
            try {
__CONVERT__
                __RET__ r = new Solution().__NAME__(__ARGS__);
                StringBuilder b = new StringBuilder("{\"ok\":");
                PlJson.w(b, r);
                plEmit(b.append('}').toString());
            } catch (OutOfMemoryError e) {
                plEmit("{\"error\":\"memory\"}");
            } catch (Throwable e) {
                plRuntime(e);
            }
        }
    }

    public static void main(String[] argv) throws Exception {
        plOut = new FileOutputStream("/proc/self/fd/3");
        String input = new String(System.in.readAllBytes(), StandardCharsets.UTF_8);
        List<?> cases = (List<?>) new PlJson(input).value();
        plEmit("{\"ready\":true}");
        Thread t = new Thread(null, () -> plRun(cases), "promptlab", 256L << 20);
        t.start();
        t.join();
        System.out.flush();
        Runtime.getRuntime().halt(0);
    }
}
"#;

pub(super) fn harness(sig: &Signature) -> String {
    let convert = sig
        .params
        .iter()
        .enumerate()
        .map(|(k, p)| {
            format!(
                "                {} pl_a{k} = PlJson.{}(args.get({k}));",
                ty(p.ty),
                conv_fn(p.ty)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let args = (0..sig.params.len())
        .map(|k| format!("pl_a{k}"))
        .collect::<Vec<_>>()
        .join(", ");
    HARNESS
        .replace("__CONVERT__", &convert)
        .replace("__RET__", &ty(sig.returns))
        .replace("__NAME__", &sig.name)
        .replace("__ARGS__", &args)
}
