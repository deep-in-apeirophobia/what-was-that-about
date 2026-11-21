//! Anonymous usage analytics (PostHog) and crash reporting (Sentry).
//!
//! Both are on by default with an opt-out in Settings → Privacy, and both are compiled in only
//! when their keys are provided at build time (`POSTHOG_KEY`, `SENTRY_DSN`; see `.env.example`).
//! Events carry an anonymous install id and coarse facts (models, formats, durations, timings).
//! They never contain recordings, transcripts, notes, titles, file names, search text or keys.
//! Analytics events queue on disk and are sent in batches when the machine is online.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Map, Value};

pub const POSTHOG_KEY: Option<&str> = option_env!("POSTHOG_KEY");
pub const POSTHOG_HOST: Option<&str> = option_env!("POSTHOG_HOST");
pub const SENTRY_DSN: Option<&str> = option_env!("SENTRY_DSN");
/// "release" (tagged builds), "nightly" (CI builds of main), "dev"/"local" otherwise.
pub const CHANNEL: &str = match option_env!("WWTA_CHANNEL") {
    Some(c) => c,
    None if cfg!(debug_assertions) => "dev",
    None => "local",
};
const MAX_QUEUE: usize = 2000;

// ------------------------------------------------------------------ privacy scrubbing

/// Remove things that could identify a person or their content from free text: home
/// directories / absolute paths, and session tokens in URLs.
pub fn scrub(s: &str) -> String {
    static RES: OnceLock<Vec<(regex::Regex, &'static str)>> = OnceLock::new();
    let res = RES.get_or_init(|| {
        vec![
            (
                regex::Regex::new(r"(?i)\b[A-Z]:\\[^\s'\x22<>|]+").unwrap(),
                "<path>",
            ),
            (
                regex::Regex::new(
                    r"(?:/home/|/Users/|/root/|/tmp/|/var/|/mnt/|/media/|/run/)[^\s'\x22<>]+",
                )
                .unwrap(),
                "<path>",
            ),
            (
                regex::Regex::new(r"(?i)(session|token|key)=[^&\s'\x22]+").unwrap(),
                "$1=<redacted>",
            ),
            (
                regex::Regex::new(r"(?i)\b(sk|phc)-[A-Za-z0-9_\-]{8,}").unwrap(),
                "<redacted>",
            ),
        ]
    });
    let mut out = s.to_string();
    for (re, rep) in res {
        out = re.replace_all(&out, *rep).into_owned();
    }
    out
}

/// Coarse category for an error message, safe to send as an analytics property.
pub fn error_kind(msg: &str) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if m.contains("cancel") {
        "cancelled"
    } else if m.contains("401") || m.contains("api key") {
        "auth"
    } else if m.contains("402") || m.contains("credits") {
        "credits"
    } else if m.contains("429") || m.contains("rate limit") {
        "rate_limited"
    } else if m.contains("checksum") {
        "checksum"
    } else if m.contains("unsupported") || m.contains("decod") {
        "unsupported_media"
    } else if m.contains("vulkan") || m.contains("gpu") {
        "gpu"
    } else if m.contains("not installed") || m.contains("missing") {
        "missing_model"
    } else if m.contains("timed out")
        || m.contains("connect")
        || m.contains("network")
        || m.contains("dns")
    {
        "network"
    } else if m.contains("space") || m.contains("disk") {
        "disk"
    } else {
        "other"
    }
}

pub fn duration_bucket(secs: f64) -> &'static str {
    match secs {
        s if s < 60.0 => "<1m",
        s if s < 600.0 => "1-10m",
        s if s < 1800.0 => "10-30m",
        s if s < 3600.0 => "30-60m",
        s if s < 7200.0 => "1-2h",
        _ => ">2h",
    }
}

pub fn size_bucket_mb(bytes: u64) -> &'static str {
    match bytes / 1_000_000 {
        0..=9 => "<10MB",
        10..=99 => "10-100MB",
        100..=499 => "100-500MB",
        500..=1999 => "0.5-2GB",
        _ => ">2GB",
    }
}

/// Lowercase extension of a path ("folder" for directories), or "none".
pub fn format_of(path: &Path) -> String {
    if path.is_dir() {
        return "folder".into();
    }
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .filter(|e| e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "none".into())
}

// ------------------------------------------------------------------ PostHog

pub struct Analytics {
    enabled: AtomicBool,
    key: Option<&'static str>,
    host: String,
    install_id: String,
    session_id: String,
    queue_path: PathBuf,
    queue: Mutex<Vec<Value>>,
    base: Mutex<Map<String, Value>>,
}

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "Mac OS X",
        "linux" => "Linux",
        other => other,
    }
}

impl Analytics {
    pub fn new(data_dir: &Path, enabled: bool) -> Arc<Self> {
        let key = POSTHOG_KEY.filter(|k| !k.trim().is_empty());
        if key.is_none() && cfg!(debug_assertions) {
            log::error!(
                "POSTHOG_KEY variable required by PostHog is missing or un-configured, this causes events to be silently \
                 missed. This error stops appearing once POSTHOG_KEY is configured"
            );
        }
        let id_path = data_dir.join("telemetry.json");
        let install_id = std::fs::read(&id_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v["installId"].as_str().map(String::from))
            .unwrap_or_else(|| {
                let id = uuid::Uuid::new_v4().to_string();
                let _ = std::fs::write(&id_path, json!({ "installId": id }).to_string());
                id
            });
        let queue_path = data_dir.join("telemetry-queue.jsonl");
        let queue: Vec<Value> = if enabled {
            std::fs::read_to_string(&queue_path)
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        } else {
            let _ = std::fs::remove_file(&queue_path);
            vec![]
        };
        let mut base = Map::new();
        base.insert("$app_version".into(), json!(env!("CARGO_PKG_VERSION")));
        base.insert("$os".into(), json!(os_name()));
        base.insert("arch".into(), json!(std::env::consts::ARCH));
        base.insert("channel".into(), json!(CHANNEL));
        base.insert("$lib".into(), json!("wwta-desktop"));
        Arc::new(Self {
            enabled: AtomicBool::new(enabled),
            key,
            host: POSTHOG_HOST
                .filter(|h| !h.trim().is_empty())
                .unwrap_or("https://us.i.posthog.com")
                .trim_end_matches('/')
                .to_string(),
            install_id,
            session_id: uuid::Uuid::new_v4().to_string(),
            queue_path,
            queue: Mutex::new(queue),
            base: Mutex::new(base),
        })
    }

    pub fn active(&self) -> bool {
        self.key.is_some() && self.enabled.load(Ordering::Relaxed)
    }

    /// Properties attached to every event (e.g. GPU details once detected).
    pub fn set_base(&self, key: &str, value: Value) {
        self.base.lock().unwrap().insert(key.into(), value);
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        if !on {
            self.queue.lock().unwrap().clear();
            let _ = std::fs::remove_file(&self.queue_path);
        }
    }

    pub fn capture(&self, event: &str, props: Value) {
        if !self.active() {
            return;
        }
        let mut p = self.base.lock().unwrap().clone();
        if let Value::Object(extra) = props {
            p.extend(extra);
        }
        p.insert("distinct_id".into(), json!(self.install_id));
        p.insert("$session_id".into(), json!(self.session_id));
        p.insert("$process_person_profile".into(), json!(false));
        let ev = json!({
            "event": event,
            "properties": p,
            "timestamp": wwta_core::types::now_iso(),
            "uuid": uuid::Uuid::new_v4().to_string(),
        });
        let mut q = self.queue.lock().unwrap();
        if q.len() >= MAX_QUEUE {
            q.remove(0);
        }
        q.push(ev.clone());
        drop(q);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.queue_path)
        {
            use std::io::Write;
            let _ = writeln!(f, "{ev}");
        }
    }

    async fn flush(&self, client: &wwta_core::reqwest::Client) {
        let Some(key) = self.key else { return };
        if !self.active() {
            return;
        }
        let batch: Vec<Value> = {
            let q = self.queue.lock().unwrap();
            if q.is_empty() {
                return;
            }
            q.iter().take(500).cloned().collect()
        };
        let n = batch.len();
        let resp = client
            .post(format!("{}/batch/", self.host))
            .json(&json!({ "api_key": key, "batch": batch }))
            .send()
            .await;
        let sent = match resp {
            Ok(r) if r.status().is_success() => true,
            // malformed batch: drop it instead of retrying forever
            Ok(r) if r.status().is_client_error() && r.status().as_u16() != 429 => {
                log::warn!("analytics batch rejected: {}", r.status());
                true
            }
            Ok(r) => {
                log::debug!("analytics flush failed: {}", r.status());
                false
            }
            Err(e) => {
                log::debug!("analytics offline: {e}");
                false
            }
        };
        if sent {
            let mut q = self.queue.lock().unwrap();
            let k = n.min(q.len());
            q.drain(..k);
            let rest: String = q.iter().map(|e| format!("{e}\n")).collect();
            let _ = std::fs::write(&self.queue_path, rest);
        }
    }

    /// Send queued events every minute (and shortly after start).
    pub fn spawn_flusher(self: &Arc<Self>) {
        if self.key.is_none() {
            return;
        }
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            wwta_core::ensure_crypto_provider();
            let Ok(client) = wwta_core::reqwest::Client::builder()
                .user_agent(concat!("what-was-that-about/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(20))
                .build()
            else {
                return;
            };
            tokio::time::sleep(Duration::from_secs(5)).await;
            loop {
                this.flush(&client).await;
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
    }
}

// ------------------------------------------------------------------ Sentry

static CRASH_REPORTS: AtomicBool = AtomicBool::new(false);

pub fn set_crash_reports(on: bool) {
    CRASH_REPORTS.store(on, Ordering::Relaxed);
}

/// Read the opt-outs straight from settings.json: Sentry must start before Tauri (and runs
/// again inside its crash-reporter process), when Tauri's path helpers aren't available.
pub fn early_settings(identifier: &str) -> (bool, bool) {
    let path = dirs::data_dir().map(|d| d.join(identifier).join("settings.json"));
    let v: Value = path
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    (
        v["analyticsEnabled"].as_bool().unwrap_or(true),
        v["crashReportsEnabled"].as_bool().unwrap_or(true),
    )
}

fn scrub_event(
    mut event: sentry::protocol::Event<'static>,
) -> Option<sentry::protocol::Event<'static>> {
    if !CRASH_REPORTS.load(Ordering::Relaxed) {
        return None;
    }
    event.server_name = None;
    event.user = None;
    if let Some(m) = event.message.as_mut() {
        *m = scrub(m);
    }
    for ex in event.exception.values.iter_mut() {
        if let Some(v) = ex.value.as_mut() {
            *v = scrub(v);
        }
    }
    for b in event.breadcrumbs.values.iter_mut() {
        if let Some(m) = b.message.as_mut() {
            *m = scrub(m);
        }
        for v in b.data.values_mut() {
            if let Value::String(s) = v {
                *s = scrub(s);
            }
        }
    }
    if let Some(sentry::protocol::Context::Device(d)) = event.contexts.get_mut("device") {
        d.name = None;
    }
    Some(event)
}

/// Initialise Sentry if a DSN was compiled in and crash reports are enabled. Must run before
/// the Tauri builder. Returns the guard (keep it alive) and the plugin to register.
pub fn init_sentry(
    crash_reports: bool,
) -> Option<(sentry::ClientInitGuard, tauri_plugin_sentry::Sentry)> {
    let dsn = SENTRY_DSN.filter(|d| !d.trim().is_empty())?;
    if !crash_reports {
        return None;
    }
    set_crash_reports(true);
    wwta_core::ensure_crypto_provider();
    let plugin = tauri_plugin_sentry::Sentry::new();
    let options = sentry::ClientOptions::new()
        .add_integration(plugin.clone())
        .dsn(dsn)
        .maybe_release(sentry::release_name!())
        .environment(CHANNEL)
        .auto_session_tracking(true)
        .before_send(scrub_event)
        .before_breadcrumb(|mut b| {
            if let Some(m) = b.message.as_mut() {
                *m = scrub(m);
            }
            Some(b)
        });
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    let options =
        options.add_integration(sentry::integrations::minidump::MinidumpIntegration::new());
    // Everything before here also runs in the crash-reporter process.
    let guard = sentry::init(options);
    Some((guard, plugin))
}

/// Report a handled failure (already scrubbed of paths and tokens) with coarse tags.
pub fn report_error(what: &str, msg: &str, tags: &[(&str, &str)]) {
    if !CRASH_REPORTS.load(Ordering::Relaxed) || sentry::Hub::current().client().is_none() {
        return;
    }
    sentry::with_scope(
        |scope| {
            scope.set_tag("failure", what);
            for (k, v) in tags {
                scope.set_tag(k, v);
            }
        },
        || sentry::capture_message(&format!("{what}: {}", scrub(msg)), sentry::Level::Error),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_paths_and_tokens() {
        let s = scrub(
            "failed to open /home/atrin/Music/جلسه.mp3 and C:\\Users\\Ali\\Desktop\\x.zip via \
             https://vc.example.ir/p1/?session=breezabc123&proto=true key sk-or-v1-abcdef123456",
        );
        assert!(!s.contains("atrin") && !s.contains("Ali") && !s.contains("breezabc123"));
        assert!(!s.contains("abcdef123456"));
        assert!(s.contains("<path>") && s.contains("session=<redacted>"));
    }

    #[test]
    fn buckets_and_kinds() {
        assert_eq!(duration_bucket(30.0), "<1m");
        assert_eq!(duration_bucket(3700.0), "1-2h");
        assert_eq!(size_bucket_mb(250_000_000), "100-500MB");
        assert_eq!(error_kind("OpenRouter rejected the API key (401)"), "auth");
        assert_eq!(error_kind("cancelled"), "cancelled");
        assert_eq!(format_of(Path::new("/x/y/Lecture.M4A")), "m4a");
        assert_eq!(format_of(Path::new("/x/y/noext")), "none");
    }
}
