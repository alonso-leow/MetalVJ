//! 3D venue generation: stable camera images in, one venue TOML out.
//!
//! **Not the harness turn loop.** [`crate::agent_loop::run_turn`] is a
//! conversation with tools, gates and a door into the live app. Writing a
//! venue from photos is one request with a fixed shape: the prompt
//! (`data/agent/venue_prompt.md`), the few-shot examples
//! (`data/agent/venues/*.toml`) as earlier turns, then the images. No tool
//! calls, no state, nothing to look at afterwards but the text. That is what
//! makes it cheap to hand to a model that runs locally.
//!
//! **The images carry `locate`'s grid.** A vision model estimating a
//! coordinate on a photo errs ~16–24%; choosing cells of a numbered grid, it
//! errs ~3–4% ([`crate::locate`]). So every image goes out with the grid
//! `locate` draws on its first pass ([`GRID`], by
//! [`crate::locate::overlay_grid`] itself), and the prompt asks for every
//! image point a shot claims (surface corners, the horizon path) as the
//! cell it falls in, `[column, row, part]`. `metal_perform::venue` turns cells
//! back into coordinates when the answer is parsed.
//!
//! **Which model.** [`Backend::Provider`] (the default) is the model
//! configured for the harness in `config.toml`/`data/ai.json`, the big LLM.
//! [`Backend::Local`] is the experiment: any OpenAI-compatible server on this
//! machine (`llama-server` from llama.cpp, Ollama's `/v1`), with a vision
//! model such as a Qwen-VL build. Both get the same body; the runner
//! (`crates/metal-dash/examples/venue_gen.rs`) scores both with
//! `metal_perform::venue::Venue::check`.
//!
//! The wire is the chat-completions shape [`crate::agent_loop`] already
//! speaks, with images as `image_url` data URLs.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Stdio};
use std::time::Duration;
use metal_schema::platform::Command;

use serde_json::{json, Value};

use crate::config::{Preset, ProviderConfig};
use crate::http::{HttpResponse, Url};

/// Where the request goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Backend {
    /// The provider configured for the harness.
    Provider,
    /// An OpenAI-compatible server, usually on loopback, with no key.
    Local { url: String, model: String },
}

/// The llama.cpp server's default chat route.
pub const LOCAL_URL: &str = "http://127.0.0.1:8080/v1/chat/completions";

/// The grid drawn over every image: `locate`'s first pass, this many columns
/// numbered along the top and as many rows down the left. A venue's cells
/// are read on it, so `metal_perform::venue::GRID` is the same number (the
/// `venue_gen` example does not compile if they part).
pub const GRID: u32 = crate::turn::COARSE_GRID;

/// The longest side an image is sent at. A bigger photo is scaled down
/// before the grid goes on, so the grid's numbers are drawn at the size the
/// model reads them; providers shrink anything much larger on their own.
pub const MAX_SIDE: u32 = 1600;

/// How long a plain-HTTP backend may take to answer. A local model reading
/// a prompt of some fifteen thousand tokens and an image, then writing a
/// whole venue, takes minutes on a laptop GPU, and a non-streamed answer
/// sends nothing until it is done: [`crate::http::post_json`]'s two-minute
/// socket timeout cuts it off.
pub const LOCAL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// One few-shot example: a venue file, verbatim.
#[derive(Clone, Debug)]
pub struct Example {
    pub id: String,
    pub toml: String,
}

/// One camera image.
#[derive(Clone, Debug)]
pub struct Image {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

impl Image {
    pub fn read(path: &Path) -> Result<Image, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mime = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase) {
            Some(e) if e == "png" => "image/png",
            Some(e) if e == "webp" => "image/webp",
            _ => "image/jpeg",
        };
        Ok(Image {
            name: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            mime: mime.to_string(),
            bytes,
        })
    }

    /// The image as the model reads it: a PNG no larger than [`MAX_SIDE`]
    /// with `locate`'s grid over it. A PNG that already fits is drawn on as
    /// it is; anything else (a JPEG, a WebP, a big or 16-bit PNG) goes
    /// through ffmpeg first, the tool the camera capture already runs
    /// (`metal_ingest::camera`).
    pub fn gridded(&self) -> Result<Image, String> {
        let fits = self.mime == "image/png"
            && crate::locate::png_size(&self.bytes).is_ok_and(|(w, h)| w.max(h) <= MAX_SIDE);
        let direct = fits.then(|| crate::locate::overlay_grid(&self.bytes, GRID).ok()).flatten();
        let bytes = match direct {
            Some(bytes) => bytes,
            None => crate::locate::overlay_grid(&to_png(&self.bytes, MAX_SIDE)?, GRID)?,
        };
        Ok(Image { name: self.name.clone(), mime: "image/png".into(), bytes })
    }

    fn part(&self) -> Value {
        json!({
            "type": "image_url",
            "image_url": { "url": format!("data:{};base64,{}", self.mime, crate::harness::base64_encode(&self.bytes)) },
        })
    }
}

/// Any image ffmpeg can read, as an 8-bit RGB PNG whose longest side is at
/// most `max_side`: the decoding [`crate::locate::overlay_grid`] leaves out.
fn to_png(bytes: &[u8], max_side: u32) -> Result<Vec<u8>, String> {
    let scale =
        format!("scale=w=min({max_side}\\,iw):h=min({max_side}\\,ih):force_original_aspect_ratio=decrease");
    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-i", "pipe:0", "-frames:v", "1", "-vf", &scale])
        .args(["-pix_fmt", "rgb24", "-f", "image2", "-vcodec", "png", "pipe:1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run ffmpeg to read the image: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("ffmpeg: no stdin")?;
    let input = bytes.to_vec();
    // Written from a thread: ffmpeg may start answering before it has read
    // everything, and a full stdout pipe would otherwise stall both sides.
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let out = child.wait_with_output().map_err(|e| format!("ffmpeg: {e}"))?;
    let _ = writer.join();
    if !out.status.success() || out.stdout.is_empty() {
        return Err(format!("ffmpeg could not read the image: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

/// Every example under `dir`, sorted by id so the prompt is the same on
/// every run.
pub fn examples(dir: &Path) -> Result<Vec<Example>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut out: Vec<Example> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .map(|p| {
            let toml = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let id = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            Ok(Example { id, toml })
        })
        .collect::<Result<_, String>>()?;
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// The shot image names an example's `[[shot]]` tables declare, in order,
/// so its example turn names the same images the answer reads.
fn shot_images(toml: &str) -> Vec<String> {
    toml.lines()
        .filter_map(|l| l.trim().strip_prefix("image = "))
        .map(|v| v.trim().trim_matches('"').to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

/// The request body: the prompt as the system message, each example as a
/// user turn naming its images and an assistant turn holding its TOML, then
/// the real images, each under the grid ([`Image::gridded`]). `ask` is the
/// operator's own words for this venue ("the bar, two cameras from the
/// door"); it may be empty.
pub fn request(
    model: &str,
    prompt: &str,
    examples: &[Example],
    images: &[Image],
    ask: &str,
    temperature: Option<f32>,
) -> Result<Value, String> {
    let mut messages = vec![json!({ "role": "system", "content": prompt })];
    for ex in examples {
        let names = shot_images(&ex.toml);
        // An invented example has designed viewpoints and no photo: its turn
        // says so, rather than claiming "0 stable image(s)".
        let content = if names.is_empty() {
            format!(
                "Example `{}`. No image: an invented room, read from designed viewpoints. Write the venue.",
                ex.id
            )
        } else {
            format!(
                "Example `{}`. {} stable image(s), each under the grid: {}. (Example turn: images not attached.) Write the venue.",
                ex.id,
                names.len(),
                names.join(", ")
            )
        };
        messages.push(json!({ "role": "user", "content": content }));
        messages.push(json!({ "role": "assistant", "content": ex.toml }));
    }
    let names: Vec<&str> = images.iter().map(|i| i.name.as_str()).collect();
    let mut text = format!(
        "{} stable image(s), one per camera, all of the same place: {}. Each has the grid drawn over it: \
         {GRID} columns numbered along the top, {GRID} rows numbered down the left. Read one [[shot]] per \
         image, every image point as the cell it falls in, [column, row, part]. Then write the venue they \
         agree on. Answer with the TOML only.",
        images.len(),
        names.join(", ")
    );
    if !ask.trim().is_empty() {
        text.push_str(&format!("\n\nThe operator says: {}", ask.trim()));
    }
    let mut content = vec![json!({ "type": "text", "text": text })];
    for image in images {
        content.push(image.gridded()?.part());
    }
    messages.push(json!({ "role": "user", "content": content }));
    let mut body = json!({ "model": model, "messages": messages });
    if let Some(t) = temperature {
        body["temperature"] = json!(t);
    }
    Ok(body)
}

/// The images a request body carries, decoded: what the model was shown.
pub fn sent_images(body: &Value) -> Vec<Vec<u8>> {
    let Some(last) = body["messages"].as_array().and_then(|m| m.last()) else {
        return Vec::new();
    };
    last["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["image_url"]["url"].as_str())
        .filter_map(|url| url.split_once(";base64,").map(|(_, b64)| b64))
        .filter_map(|b64| crate::harness::base64_decode(b64).ok())
        .collect()
}

/// The config a backend sends through. `provider` is the harness's own
/// (`ProviderConfig::load`), needed only for [`Backend::Provider`].
pub fn config_for(backend: &Backend, provider: Option<ProviderConfig>) -> Result<ProviderConfig, String> {
    match backend {
        Backend::Provider => provider
            .filter(|p| !p.url.is_empty() && !p.model.is_empty())
            .ok_or_else(|| "no provider configured ([agent.provider] in config.toml, key in data/ai.json)".to_string()),
        Backend::Local { url, model } => Ok(ProviderConfig {
            url: url.clone(),
            model: model.clone(),
            key: String::new(),
            preset: Preset::Manual,
            stt: None,
            tts: None,
        }),
    }
}

/// What a backend answered: the venue text, fences stripped, and what it
/// reported about the work (`usage`, token counts; llama-server adds
/// `timings`), for the experiment's log.
#[derive(Clone, Debug)]
pub struct Answer {
    pub text: String,
    pub usage: Value,
}

/// Sends `body` and returns the answer. A plain `http://` URL is a server
/// on this machine or the LAN and gets [`LOCAL_TIMEOUT`]; `https://` goes
/// through the shared client ([`crate::http::post_json`]).
pub fn generate(config: &ProviderConfig, body: &Value) -> Result<Answer, String> {
    let auth = format!("Bearer {}", config.key);
    let headers: Vec<(&str, &str)> = if config.key.is_empty() {
        Vec::new()
    } else {
        vec![("Authorization", auth.as_str())]
    };
    let response = if config.url.starts_with("http://") {
        post_plain(&config.url, &headers, body, LOCAL_TIMEOUT)?
    } else {
        crate::http::post_json(&config.url, &headers, body)?
    };
    let Some(value) = response.json() else {
        let head: String = String::from_utf8_lossy(&response.body).chars().take(300).collect();
        return Err(format!("the backend answered HTTP {} and no JSON: {head}", response.status));
    };
    if let Some(err) = value.get("error") {
        return Err(format!("the backend refused: {err}"));
    }
    let text = value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or("the answer has no choices[0].message.content")?;
    Ok(Answer {
        text: strip_fences(text),
        usage: json!({ "usage": value.get("usage"), "timings": value.get("timings") }),
    })
}

/// One POST over plain HTTP with a caller-chosen read timeout, answered in
/// full (`Content-Length`, chunked, or read to the close). What
/// [`generate`] uses for a local server, whose answer can take longer than
/// the shared client waits.
fn post_plain(url: &str, headers: &[(&str, &str)], body: &Value, timeout: Duration) -> Result<HttpResponse, String> {
    let url = Url::parse(url)?;
    let payload = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let mut stream = TcpStream::connect((url.host.as_str(), url.port))
        .map_err(|e| format!("connect {}:{}: {e}", url.host, url.port))?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(60))).map_err(|e| e.to_string())?;
    let mut head = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        url.path,
        url.host,
        payload.len()
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(&payload).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("reading the answer from {}:{}: {e}", url.host, url.port))?;
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> Result<HttpResponse, String> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("the answer has no end of headers")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad status line: {status_line}"))?;
    let headers: std::collections::HashMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))
        .collect();
    let rest = &raw[split + 4..];
    let body = if headers.get("transfer-encoding").is_some_and(|v| v.to_lowercase().contains("chunked")) {
        dechunk(rest)?
    } else {
        match headers.get("content-length").and_then(|v| v.parse::<usize>().ok()) {
            Some(len) if len <= rest.len() => rest[..len].to_vec(),
            Some(len) => return Err(format!("the answer was cut short: {} of {len} bytes", rest.len())),
            None => rest.to_vec(),
        }
    };
    Ok(HttpResponse { status, headers, body })
}

/// A chunked body, joined.
fn dechunk(mut rest: &[u8]) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
        let eol = rest.windows(2).position(|w| w == b"\r\n").ok_or("a chunk has no size line")?;
        let line = String::from_utf8_lossy(&rest[..eol]);
        let size = usize::from_str_radix(line.split(';').next().unwrap_or_default().trim(), 16)
            .map_err(|_| format!("bad chunk size {line:?}"))?;
        rest = &rest[eol + 2..];
        if size == 0 {
            return Ok(body);
        }
        if rest.len() < size {
            return Err("the answer was cut short inside a chunk".into());
        }
        body.extend_from_slice(&rest[..size]);
        rest = rest.get(size + 2..).unwrap_or_default();
    }
}

/// A model that wraps its TOML in a Markdown fence, or says a sentence
/// first, still answered: keep what is inside the first fence, or the text
/// from the first table header on.
pub fn strip_fences(text: &str) -> String {
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        let body = after.split_once('\n').map_or(after, |(_, rest)| rest);
        let end = body.find("```").unwrap_or(body.len());
        return body[..end].trim().to_string() + "\n";
    }
    match text.find("[venue]") {
        Some(i) => text[i..].trim().to_string() + "\n",
        None => text.trim().to_string() + "\n",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/agent")
    }

    /// A small real PNG: a gradient, so a grid over it changes pixels.
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % w) as u8, (i / w) as u8, 90]).collect();
        writer.write_image_data(&px).unwrap();
        drop(writer);
        out
    }

    fn cam(bytes: Vec<u8>) -> Image {
        Image { name: "cam-a.png".into(), mime: "image/png".into(), bytes }
    }

    #[test]
    fn the_examples_become_turns_before_the_real_images() {
        let ex = examples(&data().join("venues")).expect("examples");
        assert!(ex.len() >= 2);
        let body = request("m", "prompt", &ex, &[cam(png(64, 36))], "the bar", Some(0.2)).unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1 + 2 * ex.len() + 1);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[2]["content"], json!(ex[0].toml));
        let last = &messages[messages.len() - 1]["content"];
        assert!(last[0]["text"].as_str().unwrap().contains("The operator says: the bar"));
        assert!(last[1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,iVBOR"));
        assert_eq!(body["temperature"], json!(0.2f32));
    }

    #[test]
    fn the_image_sent_is_the_locate_grid_over_the_camera_image() {
        let raw = png(90, 60);
        let body = request("m", "p", &[], &[cam(raw.clone())], "", None).unwrap();
        let sent = sent_images(&body);
        assert_eq!(sent.len(), 1);
        assert_eq!(GRID, crate::turn::COARSE_GRID, "the grid is locate's first pass");
        assert_eq!(sent[0], crate::locate::overlay_grid(&raw, crate::turn::COARSE_GRID).unwrap());
        assert_ne!(sent[0], raw, "the model never sees the bare image");
        let text = body["messages"][1]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(&format!("{GRID} columns")) && text.contains("[column, row, part]"), "{text}");
    }

    #[test]
    fn an_image_that_is_not_a_png_is_decoded_before_the_grid() {
        // A PNG too big to send as it is goes the same way a JPEG does:
        // through ffmpeg, scaled to fit, then under the same grid.
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            panic!("ffmpeg is not installed: the camera capture needs it too");
        }
        let big = cam(png(MAX_SIDE + 200, 100));
        let sent = big.gridded().unwrap();
        let (w, h) = crate::locate::png_size(&sent.bytes).unwrap();
        assert_eq!(w, MAX_SIDE);
        assert!(h <= 100 && h >= 80, "{w}x{h}");
        assert_eq!(sent.mime, "image/png");
    }

    #[test]
    fn an_example_turn_names_its_shots_images() {
        let ex = examples(&data().join("venues")).unwrap();
        let bar = ex.iter().find(|e| e.id == "bar").unwrap();
        assert_eq!(shot_images(&bar.toml), ["bar-cam-a.jpg", "bar-cam-b.jpg"]);
    }

    #[test]
    fn an_invented_example_says_it_has_no_image() {
        let ex = examples(&data().join("venues")).unwrap();
        let desk = ex.iter().find(|e| e.id == "desk").expect("the desk example");
        assert!(shot_images(&desk.toml).is_empty());
        let body = request("m", "p", std::slice::from_ref(desk), &[], "", None).unwrap();
        assert!(body["messages"][1]["content"].as_str().unwrap().contains("No image"));
    }

    #[test]
    fn fences_and_preambles_are_stripped() {
        assert_eq!(strip_fences("Here:\n```toml\n[venue]\nid = \"x\"\n```\nbye"), "[venue]\nid = \"x\"\n");
        assert_eq!(strip_fences("Sure. [venue]\nid = \"x\""), "[venue]\nid = \"x\"\n");
    }

    #[test]
    fn a_local_backend_needs_no_key_and_the_provider_needs_a_config() {
        let local = Backend::Local { url: LOCAL_URL.into(), model: "llama".into() };
        let c = config_for(&local, None).unwrap();
        assert_eq!((c.url.as_str(), c.key.as_str()), (LOCAL_URL, ""));
        assert!(config_for(&Backend::Provider, None).is_err());
    }

    /// A local server that answers slowly, chunked (Ollama's Go server does
    /// that for a long answer), or with a length: all three come back whole.
    #[test]
    fn a_local_answer_is_read_whole_however_it_is_framed() {
        use std::net::TcpListener;
        let answer = json!({ "choices": [{ "message": { "content": "```toml\n[venue]\nid = \"x\"\n```" } }],
                             "usage": { "completion_tokens": 7 } })
        .to_string();
        let (a, b) = answer.split_at(answer.len() / 2);
        let chunked = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{a}\r\n{:x};x=y\r\n{b}\r\n0\r\n\r\n",
            a.len(),
            b.len()
        );
        let sized = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{answer}", answer.len());
        for reply in [chunked, sized] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = std::thread::spawn(move || {
                let (mut s, _) = listener.accept().unwrap();
                // Read the whole request (headers, then Content-Length bytes)
                // before answering: closing with unread bytes in the socket
                // sends a reset, and the client sees that instead of the reply.
                let mut got = Vec::new();
                let mut buf = [0u8; 65536];
                loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    got.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&got);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse().ok())?
                            })
                            .unwrap_or(0usize);
                        if got.len() >= end + 4 + len {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
                s.write_all(reply.as_bytes()).unwrap();
            });
            let config = config_for(
                &Backend::Local { url: format!("http://127.0.0.1:{port}/v1/chat/completions"), model: "m".into() },
                None,
            )
            .unwrap();
            let got = generate(&config, &json!({ "model": "m" })).unwrap();
            server.join().unwrap();
            assert_eq!(got.text, "[venue]\nid = \"x\"\n");
            assert_eq!(got.usage["usage"]["completion_tokens"], 7);
        }
    }

    #[test]
    fn the_prompt_is_on_disk_and_names_the_six_fields_and_the_grid() {
        let prompt = std::fs::read_to_string(data().join("venue_prompt.md")).expect("venue_prompt.md");
        for field in [
            "camera",
            "projection",
            "projector_distance",
            "floor_brightness",
            "background_brightness",
            "horizon_path",
            "[column, row, part]",
        ] {
            assert!(prompt.contains(field), "the prompt names {field}");
        }
    }
}
