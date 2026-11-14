//! H.6 — voice: speech to text (STT) and text to speech (TTS), as a layer
//! over the H.3 loop, not part of it. The harness (`agent_loop`) keeps
//! receiving and returning text unchanged; this module only converts
//! audio bytes into text and text into audio bytes at the edges, over the
//! existing sync HTTP client (`crate::http`). None of this is called
//! from `agent_loop` — wiring voice into the real loop is work for another phase.
//!
//! **Chosen request shape.** STT uses the multipart shape of
//! `POST /audio/transcriptions` (OpenAI-compatible): it is the most widely
//! supported — OpenAI's Whisper, DeepInfra, and nearly every
//! OpenAI-compatible proxy implement it the same way, and there is already a
//! sync HTTP client without TLS that only needs to build a body and parse JSON, not an SDK. TTS uses
//! the JSON shape of `POST /audio/speech` (`{model, input, voice}` -> audio
//! bytes in the body), the symmetric counterpart in the same ecosystem.
//! Neither adapts presets per provider (same judgment call as
//! `agent_loop::send_request` for text): if `stt`/`tts` point at Claude
//! (which has no such endpoints) or at a provider with a different shape, the
//! call fails with an explicit error, not a panic.
//!
//! **Degradation.** Missing voice is the normal case, not an error: if
//! `data/ai.json` has no `stt`/`tts`, [`transcribe`] and [`synthesize`]
//! return `Ok(None)` instead of failing. A real error (network, unexpected
//! JSON, provider down) is still `Err`, so genuine failures are not hidden
//! behind "not configured".

use crate::config::VoiceConfig;
use crate::http;
use serde_json::json;

/// Transcribes audio to text (STT), if `cfg` is configured. `audio` is
/// the already-encoded bytes (e.g. WAV — `fw-click::wav::write` over the
/// `f32` samples that `metal-listen` captures produces a valid buffer for
/// `file_name` `"clip.wav"`); this module does not impose how they were
/// generated, it only uploads them as-is.
///
/// `Ok(None)` means "STT not configured" — the caller must treat it
/// as silence, not as a failure.
pub fn transcribe(cfg: Option<&VoiceConfig>, audio: &[u8], file_name: &str) -> Result<Option<String>, String> {
    let Some(cfg) = cfg else {
        return Ok(None);
    };
    if cfg.url.is_empty() {
        return Err("stt configured without url".to_string());
    }

    let (content_type, body) = http::build_multipart(&[("model", &cfg.model)], "file", file_name, audio);
    let auth = format!("Bearer {}", cfg.key);
    let response = http::post_bytes(&cfg.url, &[("Authorization", &auth)], &content_type, &body)?;

    if response.status >= 400 {
        return Err(format!("stt provider returned status {}", response.status));
    }
    let value = response.json().ok_or("stt response was not JSON")?;
    let text = value
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or("stt response missing \"text\"")?
        .to_string();
    Ok(Some(text))
}

/// Synthesizes text to audio (TTS), if `cfg` is configured. `voice`, if
/// passed, is the provider's voice name (e.g. `"alloy"`); `None` lets
/// the provider use its default.
///
/// `Ok(None)` means "TTS not configured" — silence, not a failure.
pub fn synthesize(cfg: Option<&VoiceConfig>, text: &str, voice: Option<&str>) -> Result<Option<Vec<u8>>, String> {
    let Some(cfg) = cfg else {
        return Ok(None);
    };
    if cfg.url.is_empty() {
        return Err("tts configured without url".to_string());
    }

    let mut body = json!({ "model": cfg.model, "input": text });
    if let Some(voice) = voice {
        body["voice"] = json!(voice);
    }

    let auth = format!("Bearer {}", cfg.key);
    let response = http::post_json(&cfg.url, &[("Authorization", &auth)], &body)?;

    if response.status >= 400 {
        return Err(format!("tts provider returned status {}", response.status));
    }
    Ok(Some(response.body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    fn voice_cfg(url: String) -> VoiceConfig {
        VoiceConfig { url, model: "whisper-1".to_string(), key: "sk-test1234".to_string() }
    }

    #[test]
    fn transcribe_returns_none_when_unconfigured() {
        // The normal case: without `stt` in `data/ai.json`, no socket is opened.
        assert_eq!(transcribe(None, b"whatever bytes", "clip.wav"), Ok(None));
    }

    #[test]
    fn synthesize_returns_none_when_unconfigured() {
        assert_eq!(synthesize(None, "hello", None), Ok(None));
    }

    #[test]
    fn transcribe_sends_multipart_and_parses_text() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let req = metal_web::Request::read(&s).unwrap().unwrap();
            let mut s = s;
            assert_eq!(req.route(), ("POST", "/v1/audio/transcriptions"));
            let body_text = String::from_utf8_lossy(&req.body);
            assert!(body_text.contains("whisper-1"));
            assert!(body_text.contains("clip.wav"));

            let resp_body = json!({ "text": "hello world" }).to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                resp_body.len()
            );
            s.write_all(resp.as_bytes()).unwrap();
            s.write_all(resp_body.as_bytes()).unwrap();
        });

        let cfg = voice_cfg(format!("http://127.0.0.1:{port}/v1/audio/transcriptions"));
        let text = transcribe(Some(&cfg), b"fake wav bytes", "clip.wav").unwrap();
        assert_eq!(text, Some("hello world".to_string()));
    }

    #[test]
    fn synthesize_sends_json_and_returns_raw_audio_bytes() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let fake_audio: &[u8] = b"RIFF-fake-audio-bytes";

        std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let req = metal_web::Request::read(&s).unwrap().unwrap();
            let mut s = s;
            assert_eq!(req.route(), ("POST", "/v1/audio/speech"));
            let json: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(json["input"], "hello world");
            assert_eq!(json["voice"], "alloy");

            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nContent-Length: {}\r\n\r\n",
                fake_audio.len()
            );
            s.write_all(resp.as_bytes()).unwrap();
            s.write_all(fake_audio).unwrap();
        });

        let cfg = voice_cfg(format!("http://127.0.0.1:{port}/v1/audio/speech"));
        let audio = synthesize(Some(&cfg), "hello world", Some("alloy")).unwrap();
        assert_eq!(audio, Some(fake_audio.to_vec()));
    }

    #[test]
    fn transcribe_surfaces_a_real_error_instead_of_masking_it_as_unconfigured() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let _req = metal_web::Request::read(&s).unwrap();
            let mut s = s;
            let resp_body = b"nope";
            let resp = format!(
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\n\r\n",
                resp_body.len()
            );
            s.write_all(resp.as_bytes()).unwrap();
            s.write_all(resp_body).unwrap();
        });

        let cfg = voice_cfg(format!("http://127.0.0.1:{port}/v1/audio/transcriptions"));
        let err = transcribe(Some(&cfg), b"bytes", "clip.wav").unwrap_err();
        assert!(err.contains("401"));
    }
}
