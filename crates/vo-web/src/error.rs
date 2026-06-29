use wasm_bindgen::JsValue;

pub fn message(message: impl Into<String>) -> JsValue {
    JsValue::from_str(&message.into())
}

pub fn with_context(context: &str, err: JsValue) -> JsValue {
    let detail = err
        .as_string()
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| format!("{err:?}"));
    message(format!("{context}: {detail}"))
}
