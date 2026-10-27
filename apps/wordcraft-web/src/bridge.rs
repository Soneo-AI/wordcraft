//! JavaScript bridge for embedding WordCraft in another web app (KoAssist).
//!
//! The browser shell has no control channel (the desktop app listens on a loopback TCP port).
//! This module exposes the same control layer ([`wordcraft_ui_egui::control`]) to the page as
//! `window.koassistDoc`:
//!
//! ```js
//! koassistDoc.open(name, bytes)            // Uint8Array → the app opens it like a dropped file
//! koassistDoc.request(method, params)      // Promise<result>: any control method
//!                                        // (engine.execute, document.inspect, ui.inspect, …)
//! koassistDoc.execute(command, params)     // Promise<result>: shorthand for engine.execute
//! koassistDoc.save(path?)                  // Promise<{bytes: Uint8Array, name}>: the document's bytes
//!                                        // and the name it was opened under, no download
//! koassistDoc.onEvent(callback)            // callback({type: "ready" | "dirty", …})
//! koassistDoc.ready                        // true once the app runs
//! ```
//!
//! Requests are queued and answered from the UI thread on the next frame
//! ([`Bridge::drain`] runs in `WebShell::logic`), so they never race the app. A request's
//! promise resolves with the control reply's `result`, or rejects with its `error`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wordcraft_ui_egui::control::{self, Outcome};
use wordcraft_ui_egui::{ControlRequest, Inbox, WordApp};

/// Most queued requests; beyond that a request is rejected at once.
const MAX_QUEUED: usize = 256;

enum Pending {
    Request {
        method: String,
        params: Value,
        resolve: js_sys::Function,
        reject: js_sys::Function,
    },
    /// `save()`: the download hook hands the bytes to `resolve` instead of the browser.
    Save {
        path: Option<String>,
        resolve: js_sys::Function,
        reject: js_sys::Function,
    },
}

#[derive(Clone, Default)]
pub struct Bridge {
    queue: Rc<RefCell<VecDeque<Pending>>>,
    /// Armed while a `save()` runs: the next download's bytes go here (see [`Bridge::capture_download`]).
    save_sink: Rc<RefCell<Option<js_sys::Function>>>,
    listeners: Rc<RefCell<Vec<js_sys::Function>>>,
    ctx: Rc<RefCell<Option<egui::Context>>>,
    last_dirty: Rc<Cell<Option<bool>>>,
}

impl Bridge {
    /// The app's context, once it runs: queued requests wake it.
    pub fn attach(&self, ctx: egui::Context) {
        *self.ctx.borrow_mut() = Some(ctx);
    }

    fn wake(&self) {
        if let Some(ctx) = self.ctx.borrow().as_ref() {
            ctx.request_repaint();
        }
    }

    /// Tell the page something happened (`{type, …}`); a listener that throws is dropped.
    pub fn emit(&self, event: Value) {
        let Ok(js) = value_to_js(&event) else { return };
        let listeners = self.listeners.borrow().clone();
        for cb in listeners {
            if cb.call1(&JsValue::NULL, &js).is_err() {
                let failed: &JsValue = cb.as_ref();
                self.listeners.borrow_mut().retain(|l| {
                    let kept: &JsValue = l.as_ref();
                    kept != failed
                });
            }
        }
    }

    /// `Services::on_dirty` hook: forward changes to the page.
    pub fn report_dirty(&self, dirty: bool) {
        if self.last_dirty.get() == Some(dirty) {
            return;
        }
        self.last_dirty.set(Some(dirty));
        self.emit(json!({"type": "dirty", "dirty": dirty}));
    }

    /// `Services::download` hook: while a `save()` is pending, the bytes go to the page
    /// instead of the browser's download, with the name the document carries (what `open`
    /// was given, or the app's default for a document it created itself) — the page checks
    /// that it is still its document before it stores the bytes. Returns `true` when it took them.
    pub fn capture_download(&self, path: &str, bytes: &[u8]) -> bool {
        let Some(resolve) = self.save_sink.borrow_mut().take() else { return false };
        let result = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&result, &"bytes".into(), &js_sys::Uint8Array::from(bytes));
        let _ = js_sys::Reflect::set(&result, &"name".into(), &JsValue::from_str(path));
        let _ = resolve.call1(&JsValue::NULL, &result);
        true
    }

    /// Install `window.koassistDoc`. `inbox` receives opened files, exactly like dropped ones.
    pub fn install(&self, inbox: Inbox) -> Result<(), String> {
        let window = web_sys::window().ok_or("no window")?;
        let api = js_sys::Object::new();
        let js = |e: JsValue| format!("{e:?}");

        // open(name, bytes)
        let bridge = self.clone();
        let open = Closure::<dyn Fn(JsValue, JsValue)>::new(move |name: JsValue, bytes: JsValue| {
            let name = name.as_string().unwrap_or_else(|| "document.docx".into());
            let bytes = js_sys::Uint8Array::new(&bytes).to_vec();
            inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes));
            bridge.wake();
        });
        js_sys::Reflect::set(&api, &"open".into(), open.as_ref().unchecked_ref()).map_err(js)?;
        open.forget();

        // request(method, params) → Promise
        let bridge = self.clone();
        let request = Closure::<dyn Fn(JsValue, JsValue) -> js_sys::Promise>::new(move |method: JsValue, params: JsValue| {
            let bridge = bridge.clone();
            let method = method.as_string().unwrap_or_default();
            let params = js_to_value(&params);
            js_sys::Promise::new(&mut |resolve, reject| {
                bridge.push(Pending::Request { method: method.clone(), params: params.clone(), resolve, reject });
            })
        });
        js_sys::Reflect::set(&api, &"request".into(), request.as_ref().unchecked_ref()).map_err(js)?;
        request.forget();

        // execute(command, params) → Promise (engine.execute)
        let bridge = self.clone();
        let execute = Closure::<dyn Fn(JsValue, JsValue) -> js_sys::Promise>::new(move |command: JsValue, params: JsValue| {
            let bridge = bridge.clone();
            let command = command.as_string().unwrap_or_default();
            let params = js_to_value(&params);
            js_sys::Promise::new(&mut |resolve, reject| {
                bridge.push(Pending::Request {
                    method: "engine.execute".into(),
                    params: json!({"command": command, "params": params}),
                    resolve,
                    reject,
                });
            })
        });
        js_sys::Reflect::set(&api, &"execute".into(), execute.as_ref().unchecked_ref()).map_err(js)?;
        execute.forget();

        // save(path?) → Promise<Uint8Array>
        let bridge = self.clone();
        let save = Closure::<dyn Fn(JsValue) -> js_sys::Promise>::new(move |path: JsValue| {
            let bridge = bridge.clone();
            let path = path.as_string();
            js_sys::Promise::new(&mut |resolve, reject| {
                bridge.push(Pending::Save { path: path.clone(), resolve, reject });
            })
        });
        js_sys::Reflect::set(&api, &"save".into(), save.as_ref().unchecked_ref()).map_err(js)?;
        save.forget();

        // onEvent(callback)
        let listeners = self.listeners.clone();
        let on_event = Closure::<dyn Fn(JsValue)>::new(move |cb: JsValue| {
            if let Ok(f) = cb.dyn_into::<js_sys::Function>() {
                listeners.borrow_mut().push(f);
            }
        });
        js_sys::Reflect::set(&api, &"onEvent".into(), on_event.as_ref().unchecked_ref()).map_err(js)?;
        on_event.forget();

        js_sys::Reflect::set(&api, &"ready".into(), &JsValue::FALSE).map_err(js)?;
        js_sys::Reflect::set(&window, &"koassistDoc".into(), &api).map_err(js)?;
        Ok(())
    }

    /// Mark the API ready (the app runs) and tell listeners.
    pub fn set_ready(&self) {
        if let Some(window) = web_sys::window()
            && let Ok(api) = js_sys::Reflect::get(&window, &"koassistDoc".into())
        {
            let _ = js_sys::Reflect::set(&api, &"ready".into(), &JsValue::TRUE);
        }
        self.emit(json!({"type": "ready"}));
    }

    fn push(&self, pending: Pending) {
        let mut queue = self.queue.borrow_mut();
        if queue.len() >= MAX_QUEUED {
            drop(queue);
            let reject = match &pending {
                Pending::Request { reject, .. } | Pending::Save { reject, .. } => reject.clone(),
            };
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("too many pending requests"));
            return;
        }
        queue.push_back(pending);
        drop(queue);
        self.wake();
    }

    /// Answer every queued request (UI thread, once per frame).
    pub fn drain(&self, app: &mut WordApp, ctx: &egui::Context) {
        loop {
            let Some(pending) = self.queue.borrow_mut().pop_front() else { break };
            match pending {
                Pending::Request { method, params, resolve, reject } => {
                    let (req, _rx) = ControlRequest::new(method, params);
                    settle(control::handle(app, ctx, &req), &resolve, &reject);
                }
                Pending::Save { path, resolve, reject } => {
                    *self.save_sink.borrow_mut() = Some(resolve.clone());
                    let mut params = json!({});
                    if let Some(p) = path {
                        params = json!({"path": p});
                    }
                    let (req, _rx) = ControlRequest::new(
                        "engine.execute",
                        json!({"command": if params.get("path").is_some() { "file.saveAs" } else { "file.save" }, "params": params}),
                    );
                    let outcome = control::handle(app, ctx, &req);
                    // The download hook took the bytes and resolved; otherwise the save failed.
                    if self.save_sink.borrow_mut().take().is_some() {
                        let error = match outcome {
                            Outcome::Done(v) => v.get("error").and_then(Value::as_str).unwrap_or("save produced no bytes").to_string(),
                            Outcome::Screenshot { .. } => "unexpected reply".to_string(),
                        };
                        let _ = reject.call1(&JsValue::NULL, &JsValue::from_str(&error));
                    }
                }
            }
        }
    }
}

fn settle(outcome: Outcome, resolve: &js_sys::Function, reject: &js_sys::Function) {
    match outcome {
        Outcome::Done(v) => {
            if v.get("ok").and_then(Value::as_bool) == Some(true) {
                let result = v.get("result").cloned().unwrap_or(Value::Null);
                match value_to_js(&result) {
                    Ok(js) => {
                        let _ = resolve.call1(&JsValue::NULL, &js);
                    }
                    Err(e) => {
                        let _ = reject.call1(&JsValue::NULL, &JsValue::from_str(&e));
                    }
                }
            } else {
                let error = v.get("error").and_then(Value::as_str).unwrap_or("request failed");
                let _ = reject.call1(&JsValue::NULL, &JsValue::from_str(error));
            }
        }
        Outcome::Screenshot { .. } => {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("screenshots are not available in the browser"));
        }
    }
}

/// JS value → JSON (through `JSON.stringify`); `undefined`/`null` → `{}`.
fn js_to_value(v: &JsValue) -> Value {
    if v.is_undefined() || v.is_null() {
        return json!({});
    }
    js_sys::JSON::stringify(v).ok().and_then(|s| s.as_string()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({}))
}

/// JSON → JS value (through `JSON.parse`).
fn value_to_js(v: &Value) -> Result<JsValue, String> {
    let text = serde_json::to_string(v).map_err(|e| e.to_string())?;
    js_sys::JSON::parse(&text).map_err(|e| format!("{e:?}"))
}
