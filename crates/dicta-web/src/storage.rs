use wasm_bindgen::{JsValue, prelude::wasm_bindgen};

use crate::provider::WebProviderConfig;
use crate::{error, json};

const PROVIDER_CONFIG_KEY: &str = "dicta.provider.config";

#[wasm_bindgen]
pub fn save_provider_config(config: JsValue) -> Result<(), JsValue> {
    let config: WebProviderConfig = json::from_js(&config)?;
    let value = serde_json::to_string(&config)
        .map_err(|err| error::message(format!("failed to serialize provider config: {err}")))?;
    local_storage()?.set_item(PROVIDER_CONFIG_KEY, &value)
}

#[wasm_bindgen]
pub fn load_provider_config() -> Result<JsValue, JsValue> {
    let Some(value) = local_storage()?.get_item(PROVIDER_CONFIG_KEY)? else {
        return Ok(JsValue::NULL);
    };
    let config: WebProviderConfig = serde_json::from_str(&value)
        .map_err(|err| error::message(format!("failed to parse provider config: {err}")))?;
    json::to_js(&config)
}

#[wasm_bindgen]
pub fn delete_provider_config() -> Result<(), JsValue> {
    local_storage()?.remove_item(PROVIDER_CONFIG_KEY)
}

fn local_storage() -> Result<web_sys::Storage, JsValue> {
    web_sys::window()
        .ok_or_else(|| error::message("window is unavailable"))?
        .local_storage()?
        .ok_or_else(|| error::message("localStorage is unavailable"))
}
