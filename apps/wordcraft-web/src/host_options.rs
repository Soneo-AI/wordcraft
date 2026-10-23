//! Options the embedding page sets before WordCraft starts (KoAssist).
//!
//! The page assigns `window.wordcraftOptions` before it loads the bundle:
//!
//! ```js
//! window.wordcraftOptions = {
//!   embedded: { tabs: ["Home", "Insert"], hidden_groups: ["Clipboard"], status_bar: true },
//!   language: "de",              // ui.language
//!   theme: "light" | "dark" | "system",   // ui.theme
//!   ruler: false,                // view.ruler
//!   tokens: { light: { accent: "#0f4f4a", … }, dark: { … } },   // ui.hostTokens
//! };
//! ```
//!
//! They are applied once, after the stored preferences and before the first frame, so the app
//! never shows the chrome the host doesn't want. Missing or malformed options leave the app as
//! it is; a wrong value is logged and skipped, never fatal.

use serde_json::{Value, json};
use wordcraft_ui_egui::WordApp;
use wordcraft_ui_egui::embedded::Embedded;

#[derive(Debug, Default)]
pub struct HostOptions {
    pub embedded: Option<Embedded>,
    pub language: Option<String>,
    pub theme: Option<String>,
    pub ruler: Option<bool>,
    pub tokens: Option<Value>,
}

/// `window.wordcraftOptions`, if the page set it.
pub fn read() -> HostOptions {
    let Some(window) = web_sys::window() else { return HostOptions::default() };
    let Ok(raw) = js_sys::Reflect::get(&window, &"wordcraftOptions".into()) else { return HostOptions::default() };
    if raw.is_undefined() || raw.is_null() {
        return HostOptions::default();
    }
    let text = js_sys::JSON::stringify(&raw).ok().and_then(|s| s.as_string());
    let value: Value = match text.as_deref().map(serde_json::from_str::<Value>) {
        Some(Ok(v)) => v,
        _ => {
            log::warn!("wordcraftOptions: not JSON; ignored");
            return HostOptions::default();
        }
    };
    from_value(&value)
}

/// The options from their JSON form (also what `read` parses).
pub fn from_value(value: &Value) -> HostOptions {
    let embedded = match value.get("embedded") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => None,
        Some(Value::Bool(true)) => Some(Embedded::default()),
        Some(other) => match serde_json::from_value::<Embedded>(other.clone()) {
            Ok(e) => Some(e),
            Err(e) => {
                log::warn!("wordcraftOptions.embedded: {e}; using the defaults");
                Some(Embedded::default())
            }
        },
    };
    HostOptions {
        embedded,
        language: value.get("language").and_then(Value::as_str).map(str::to_string),
        theme: value.get("theme").and_then(Value::as_str).map(str::to_string),
        ruler: value.get("ruler").and_then(Value::as_bool),
        tokens: value.get("tokens").filter(|t| t.is_object()).cloned(),
    }
}

/// Apply the options to the app (before its first frame).
pub fn apply(app: &mut WordApp, options: &HostOptions) {
    app.embedded = options.embedded.clone();
    let mut run = |command: &str, params: Value| {
        if let Err(e) = app.run(command, params) {
            log::warn!("wordcraftOptions: {command}: {e}");
        }
    };
    if let Some(language) = &options.language {
        run("ui.language", json!({"value": language}));
    }
    if let Some(theme) = &options.theme {
        run("ui.theme", json!({"value": theme}));
    }
    if let Some(ruler) = options.ruler {
        run("view.ruler", json!({"value": ruler}));
    }
    if let Some(tokens) = &options.tokens {
        run("ui.hostTokens", tokens.clone());
    }
}
