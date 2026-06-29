use serde::{de::DeserializeOwned, Serialize};
use wasm_bindgen::JsValue;

use crate::error;

pub fn from_js<T>(value: &JsValue) -> Result<T, JsValue>
where
    T: DeserializeOwned,
{
    let json = js_sys::JSON::stringify(value)
        .map_err(|err| error::with_context("failed to stringify JS value", err))?
        .as_string()
        .ok_or_else(|| error::message("failed to stringify JS value"))?;
    serde_json::from_str(&json)
        .map_err(|err| error::message(format!("failed to parse JSON value: {err}")))
}

pub fn to_js<T>(value: &T) -> Result<JsValue, JsValue>
where
    T: Serialize,
{
    let json = serde_json::to_string(value)
        .map_err(|err| error::message(format!("failed to serialize JSON value: {err}")))?;
    js_sys::JSON::parse(&json).map_err(|err| error::with_context("failed to parse JS JSON", err))
}
