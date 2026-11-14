//! `metal-mcp`: MetalVJ's agent tools as an MCP server.
//!
//!   metal-mcp --dash http://127.0.0.1:7900            # stdio (default)
//!   metal-mcp --dash http://127.0.0.1:7900 --http :7910 --http-token $T

use metal_mcp::dash::{Dash, DEFAULT_DASH_URL};
use metal_mcp::transport;
use std::process::ExitCode;

const USAGE: &str = "usage: metal-mcp [--dash URL] [--http [HOST]:PORT [--http-token TOKEN]]

  --dash URL      metal-dash's base URL (default: $FW_DASH_URL or http://127.0.0.1:7900)
  --http ADDR     serve Streamable HTTP at POST /mcp instead of stdio
                  (`:PORT` binds 127.0.0.1). Every request needs
                  `Authorization: Bearer <token>`; a non-loopback address
                  refuses to start without a token
  --http-token T  the bearer token for --http (default: the agent-token file)
  --allow-ask    let MCP callers run tools whose policy is `ask` (without it
                 they answer \"denied by operator\"; `off` tools never run)";

fn main() -> ExitCode {
    let mut dash_url = std::env::var("FW_DASH_URL").unwrap_or_else(|_| DEFAULT_DASH_URL.to_string());
    let mut http: Option<String> = None;
    let mut http_token: Option<String> = None;
    let mut allow_ask = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dash" => match args.next() {
                Some(v) => dash_url = v,
                None => return fail("--dash needs a URL"),
            },
            "--http" => match args.next() {
                Some(v) => http = Some(v),
                None => return fail("--http needs an address, e.g. :7910"),
            },
            "--http-token" => match args.next() {
                Some(v) if !v.trim().is_empty() => http_token = Some(v.trim().to_string()),
                _ => return fail("--http-token needs a non-empty token"),
            },
            "--allow-ask" => allow_ask = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => return fail(&format!("unknown argument `{other}`")),
        }
    }

    // The token metal-dash issued: FW_DASH_TOKEN wins, else the install's file.
    let file_token = metal_agent::auth::read_token(&metal_agent::auth::token_path());
    let token = std::env::var("FW_DASH_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| file_token.clone());
    if token.is_none() {
        eprintln!(
            "metal-mcp: no agent token (FW_DASH_TOKEN, or {}); in-process tools will be refused",
            metal_agent::auth::token_path().display()
        );
    }
    // The HTTP wire's own credential: `--http-token`, else the agent-token
    // file. It is checked on every POST /mcp, so it must exist before the
    // socket is bound — and a remote bind without one is refused outright.
    let http_token = http_token.or(file_token).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    if let Some(bind) = &http {
        if http_token.is_none() {
            return fail(&format!(
                "--http {bind} needs a bearer token: pass --http-token, or create the agent-token file at {}",
                metal_agent::auth::token_path().display()
            ));
        }
        if !transport::is_loopback_bind(bind) {
            eprintln!("metal-mcp: binding {bind}, which is reachable from other machines; the bearer token is the only guard");
        }
    }
    let dash = match Dash::new(&dash_url) {
        Ok(d) => d.with_allow_ask(allow_ask).with_token(token.map(|t| t.trim().to_string())),
        Err(e) => return fail(&e),
    };
    let result = match (http, http_token) {
        (Some(bind), Some(t)) => transport::serve_http(&bind, dash, &t),
        (Some(_), None) => unreachable!("checked above"),
        (None, _) => transport::serve_stdio(&dash),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e.to_string()),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("metal-mcp: {message}\n{USAGE}");
    ExitCode::from(2)
}
