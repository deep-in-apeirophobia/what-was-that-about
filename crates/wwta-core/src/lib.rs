//! Core library for What Was That About.

pub mod adobe_connect;
pub mod audio;
pub mod db;
pub mod export;
pub mod flv;
pub mod ggml;
pub mod llm;
pub mod models;
pub mod search;
pub mod text;
pub mod types;

#[cfg(feature = "asr")]
pub mod asr;
pub mod chunking;

/// Install the ring-based rustls provider once (reqwest is built without a default provider).
pub fn ensure_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
