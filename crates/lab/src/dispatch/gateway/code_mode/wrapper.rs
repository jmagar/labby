//! Shared JS wrapper constants and the execute-wrapper body builder.

/// The single contract error message for the execute wrapper, defined once for
/// the javy runner so the shape contract has a single source of truth.
const CODE_MODE_MAIN_SHAPE_ERROR: &str =
    "code_execute code must evaluate to an async arrow function: async () => { ... }";

pub(in crate::dispatch::gateway::code_mode) const CODE_MODE_VALUE_CODEC_JS: &str = r#"
function __labBase64FromBytes(bytes) {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const a = bytes[i];
    const b = i + 1 < bytes.length ? bytes[i + 1] : 0;
    const c = i + 2 < bytes.length ? bytes[i + 2] : 0;
    const triple = (a << 16) | (b << 8) | c;
    out += alphabet[(triple >> 18) & 63];
    out += alphabet[(triple >> 12) & 63];
    out += i + 1 < bytes.length ? alphabet[(triple >> 6) & 63] : "=";
    out += i + 2 < bytes.length ? alphabet[triple & 63] : "=";
  }
  return out;
}
function __labBytesFromBase64(data) {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let clean = String(data || "").replace(/=+$/, "");
  let buffer = 0;
  let bits = 0;
  const out = [];
  for (let i = 0; i < clean.length; i++) {
    const value = alphabet.indexOf(clean[i]);
    if (value < 0) continue;
    buffer = (buffer << 6) | value;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out.push((buffer >> bits) & 255);
    }
  }
  return new Uint8Array(out);
}
var __labBinaryTypes = {
  Int8Array: typeof Int8Array !== "undefined" ? Int8Array : null,
  Uint8Array: typeof Uint8Array !== "undefined" ? Uint8Array : null,
  Uint8ClampedArray: typeof Uint8ClampedArray !== "undefined" ? Uint8ClampedArray : null,
  Int16Array: typeof Int16Array !== "undefined" ? Int16Array : null,
  Uint16Array: typeof Uint16Array !== "undefined" ? Uint16Array : null,
  Int32Array: typeof Int32Array !== "undefined" ? Int32Array : null,
  Uint32Array: typeof Uint32Array !== "undefined" ? Uint32Array : null,
  Float32Array: typeof Float32Array !== "undefined" ? Float32Array : null,
  Float64Array: typeof Float64Array !== "undefined" ? Float64Array : null,
  BigInt64Array: typeof BigInt64Array !== "undefined" ? BigInt64Array : null,
  BigUint64Array: typeof BigUint64Array !== "undefined" ? BigUint64Array : null,
  DataView: typeof DataView !== "undefined" ? DataView : null,
  ArrayBuffer: typeof ArrayBuffer !== "undefined" ? ArrayBuffer : null
};
function __labEncodeResult(value) {
  if (value == null) return value;
  if (typeof ArrayBuffer !== "undefined" && value instanceof ArrayBuffer) {
    return { __labBinary: "base64", type: "ArrayBuffer", data: __labBase64FromBytes(new Uint8Array(value)) };
  }
  if (typeof ArrayBuffer !== "undefined" && ArrayBuffer.isView && ArrayBuffer.isView(value)) {
    return { __labBinary: "base64", type: value.constructor && value.constructor.name || "TypedArray", data: __labBase64FromBytes(new Uint8Array(value.buffer, value.byteOffset, value.byteLength)) };
  }
  if (Array.isArray(value)) return value.map(__labEncodeResult);
  if (typeof value === "object") {
    // Honor toJSON() (e.g. Date) exactly like JSON.stringify would, so a Date
    // round-trips as its ISO string instead of collapsing to {}.
    if (typeof value.toJSON === "function") {
      return __labEncodeResult(value.toJSON());
    }
    const out = {};
    for (const key of Object.keys(value)) out[key] = __labEncodeResult(value[key]);
    return out;
  }
  return value;
}
function __labDecodeResult(value) {
  if (value == null) return value;
  if (
    typeof value === "object" &&
    value.__labBinary === "base64" &&
    typeof value.data === "string" &&
    typeof value.type === "string" &&
    Object.prototype.hasOwnProperty.call(__labBinaryTypes, value.type) &&
    __labBinaryTypes[value.type]
  ) {
    const bytes = __labBytesFromBase64(value.data);
    if (value.type === "ArrayBuffer") {
      return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
    }
    const Ctor = __labBinaryTypes[value.type];
    if (value.type === "DataView") {
      return new DataView(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
    }
    if (value.type === "Uint8Array" || value.type === "Uint8ClampedArray") {
      // Already a byte view of the right width — wrap without re-walking bytes.
      return new Ctor(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    }
    // Reconstruct the original element width from the raw bytes. Use a copied
    // ArrayBuffer so byteOffset/length line up with the typed-array constructor.
    return new Ctor(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
  }
  if (Array.isArray(value)) return value.map(__labDecodeResult);
  if (typeof value === "object") {
    const out = {};
    for (const key of Object.keys(value)) out[key] = __labDecodeResult(value[key]);
    return out;
  }
  return value;
}
"#;

/// Build the shared inner body of the execute wrapper for `code`.
///
/// The javy runner invokes the result by assigning the user code to
/// `__codeModeMain`, verifying it is a function (throwing the shared contract
/// error otherwise), then `return await __codeModeMain();`. Built by
/// concatenation (not a brace-laden `format!`) so the literal JS braces need no
/// escaping.
pub(in crate::dispatch::gateway::code_mode) fn code_mode_main_invoker(code: &str) -> String {
    let mut body = String::new();
    body.push_str("  const __codeModeMain = (");
    body.push_str(code);
    body.push_str(");\n");
    body.push_str("  if (typeof __codeModeMain !== \"function\") {\n");
    body.push_str("    throw new TypeError(");
    // Embed the shared message as a JSON string literal — valid JS and safely
    // quoted regardless of its contents.
    body.push_str(
        &serde_json::to_string(CODE_MODE_MAIN_SHAPE_ERROR).unwrap_or_else(|_| {
            "\"code_execute code must be an async arrow function\"".to_string()
        }),
    );
    body.push_str(");\n");
    body.push_str("  }\n");
    body.push_str("  return __labEncodeResult(await __codeModeMain());\n");
    body
}
