//! `wwta-doc://localhost/<doc-id>` serves AI-generated HTML docs from their own origin with a
//! strict CSP (inline code allowed, no network). Loading them by URL instead of `srcdoc`
//! means they don't inherit the app's CSP, and the app's `frame-src` keeps them from
//! navigating anywhere else. The iframe is additionally `sandbox="allow-scripts"`.

use std::borrow::Cow;

use serde_json::json;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, Manager};

use crate::state::AppState;

pub const DOC_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
    img-src data: blob:; font-src data:; media-src data: blob:; form-action 'none'; base-uri 'none'";

const BRIDGE: &str = r#"<script>
window.WWTA = (function () {
  var data = __WWTA_DATA__;
  var listeners = [];
  window.addEventListener("message", function (e) {
    if (e.source !== window.parent) return;
    var m = e.data;
    if (m && m.type === "wwta:time" && typeof m.time === "number") {
      listeners.forEach(function (cb) { try { cb(m.time, !!m.playing); } catch (_) {} });
    }
  });
  function post(msg) { window.parent.postMessage(msg, "*"); }
  // focus inside the frame swallows Escape; let the app close the viewer
  window.addEventListener("keydown", function (e) { if (e.key === "Escape") post({ type: "wwta:escape" }); });
  return {
    recording: data.recording,
    segments: data.segments,
    analysis: data.analysis,
    notes: data.notes || [],
    seek: function (t) { var n = Number(t); if (isFinite(n) && n >= 0) post({ type: "wwta:seek", time: n }); },
    play: function () { post({ type: "wwta:play" }); },
    pause: function () { post({ type: "wwta:pause" }); },
    onTime: function (cb) { if (typeof cb === "function") listeners.push(cb); }
  };
})();
</script>"#;

/// JSON that is safe to embed inside a <script> element.
pub fn script_safe_json(v: &serde_json::Value) -> String {
    serde_json::to_string(v)
        .unwrap_or_else(|_| "null".into())
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

pub fn inject(html: &str, data: &serde_json::Value) -> String {
    let prelude = format!(
        "<meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{DOC_CSP}\">{}",
        BRIDGE.replace("__WWTA_DATA__", &script_safe_json(data))
    );
    let lower = html.to_ascii_lowercase();
    if let Some(i) = lower.find("<head") {
        if let Some(j) = lower[i..].find('>') {
            let at = i + j + 1;
            return format!("{}{}{}", &html[..at], prelude, &html[at..]);
        }
    }
    format!("<!doctype html><html><head>{prelude}</head><body>{html}</body></html>")
}

fn respond(status: StatusCode, body: String) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header("Content-Security-Policy", DOC_CSP)
        // the app grants microphone access (voice messages); generated docs never get it
        .header(
            "Permissions-Policy",
            "microphone=(), camera=(), geolocation=(), display-capture=()",
        )
        .header("X-Content-Type-Options", "nosniff")
        .header("Cache-Control", "no-store")
        .body(Cow::Owned(body.into_bytes()))
        .unwrap()
}

pub fn handle(app: &AppHandle, req: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let id = req.uri().path().trim_start_matches('/').to_string();
    let state = app.state::<AppState>();
    let db = state.db.lock().unwrap();
    let Ok(doc) = db.doc(&id) else {
        return respond(StatusCode::NOT_FOUND, "<p>Document not found.</p>".into());
    };
    let rec = db.recording(&doc.recording_id).ok();
    let segs = db.segments(&doc.recording_id).unwrap_or_default();
    let analysis = db.analysis(&doc.recording_id).ok().flatten();
    let notes = db.notes(&doc.recording_id).unwrap_or_default();
    drop(db);
    let data = json!({
        "recording": rec.as_ref().map(|r| json!({ "id": r.id, "title": r.title, "durationSec": r.duration_sec })),
        "segments": segs.iter().map(|s| json!({ "id": s.id, "start": s.start, "end": s.end, "text": s.text })).collect::<Vec<_>>(),
        "analysis": analysis,
        "notes": notes.iter().map(|n| json!({ "start": n.start, "end": n.end, "body": n.body, "color": n.color })).collect::<Vec<_>>(),
    });
    state.analytics.capture(
        "doc viewed",
        serde_json::json!({ "doc_chars": doc.html.len() }),
    );
    respond(StatusCode::OK, inject(&doc.html, &data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_after_head_and_escapes_script_breakouts() {
        let data =
            json!({ "segments": [{ "text": "</script><script>alert(1)</script>\u{2028}" }] });
        let out = inject(
            "<!DOCTYPE html><html><HEAD><title>x</title></HEAD><body></body></html>",
            &data,
        );
        assert!(out.starts_with("<!DOCTYPE html><html><HEAD><meta charset"));
        assert!(!out.contains("</script><script>alert"));
        assert!(out.contains("\\u003c/script>"));
        assert!(out.contains("\\u2028"));
        let bare = inject("<div>hi</div>", &data);
        assert!(bare.starts_with("<!doctype html><html><head><meta charset"));
    }
}
