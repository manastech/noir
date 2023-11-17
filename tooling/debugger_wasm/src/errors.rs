use js_sys::{Error, JsString};
use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen(typescript_custom_section)]
const DEBUGGER_ERROR: &'static str = r#"
export type DebuggerError = Error;
"#;

/// DebuggerError is a raw js error.
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(extends = Error, js_name = "DebuggerError", typescript_type = "DebuggerError")]
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub type JsDebuggerError;

    #[wasm_bindgen(constructor, js_class = "Error")]
    fn constructor(message: JsString) -> JsDebuggerError;
}

impl JsDebuggerError {
    /// Creates a new execution error with the given call stack.
    /// Call stacks won't be optional in the future, after removing ErrorLocation in ACVM.
    pub fn new(message: String) -> Self {
        JsDebuggerError::constructor(JsString::from(message))
    }
}

impl From<String> for JsDebuggerError {
    fn from(value: String) -> Self {
        JsDebuggerError::new(value)
    }
}
