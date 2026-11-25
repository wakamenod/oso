//! oso's fake TypeWhisper, for tests only (ADR 0175): an HTTP server on the loopback that answers
//! the dictation API from a script and records what it was asked. A binary of dev builds only, so
//! it never ships (`just release-safety`). Not `main.rs`, so that cargo does not take its
//! directory for a test.
//!
//! Must not depend on: anything of oso's; its answers are written by hand, so a test of oso's
//! client can check the client.
//!
//! Run as `fake-typewhisper <script.json> <discovery.json> <requests.jsonl>`. It listens on a
//! port the system picks on `127.0.0.1`, writes the discovery file there, mode 0600, as
//! TypeWhisper does, and adds one JSON line to the requests file for each request:
//! `{"method", "path", "authorized", "body"}`, never the token itself. A script it cannot read
//! ends it with 64.
//!
//! The script is a JSON object:
//!
//! ```json
//! {
//!   "token": "a-token-of-sixteen-or-more",
//!   "discovery_token": "another-one-to-be-refused",
//!   "routes": {
//!     "POST /v1/dictation/start": [{"status": 200, "body": {"id": "…", "status": "recording"}}],
//!     "GET /v1/dictation/transcription": [{"status": 200, "body": {"status": "recording"}}]
//!   },
//!   "dictionary": {
//!     "terms": ["oso"],
//!     "corrections": [{"original": "ty", "replacement": "TypeWhisper", "caseSensitive": false}],
//!     "file": "/where/it/is/kept.json"
//!   },
//!   "exit_after": 3
//! }
//! ```
//!
//! Each route's answers are given in order, the last again once they run out; a request with no
//! route gets a 404, and one without the token a 401. Until a stop has come, a transcription's
//! look is answered `{"status": "recording"}` and takes none of its route's answers, as
//! TypeWhisper records until it is stopped. With a `dictionary`, its terms and corrections are
//! kept and answered as TypeWhisper does, a change of them too, unless a route says otherwise:
//! an original is matched without regard to case, and each change is kept in `file`, which a
//! test writes to play the person's own changes. `delay_ms` in an answer waits before it.
//! `exit_after` ends the server once it has answered that many requests, as TypeWhisper quitting.
//! A server that has heard nothing for [`IDLE`] ends too, so that a flow that was cut short
//! leaves no listener behind.

#![expect(
    clippy::disallowed_types,
    reason = "a fake of TypeWhisper's loopback server, in tests only, needs a TCP listener; oso itself has no network code (ADR 0010, ADR 0175)"
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};

/// The token unless the script gives one.
const TOKEN: &str = "fake-typewhisper-token-0123456789";

/// The route of a stop, and of a look at a dictation.
const STOP: &str = "POST /v1/dictation/stop";
const TRANSCRIPTION: &str = "GET /v1/dictation/transcription";

/// How long the server waits for a request before it ends.
const IDLE: Duration = Duration::from_secs(60);

/// The most bytes of a request read.
const MAX_REQUEST: u64 = 64 * 1024;

/// One answer of a route.
#[derive(Clone)]
struct Answer {
    status: u16,
    body: String,
    delay: Duration,
}

/// What the script says.
struct Script {
    token: String,
    discovery_token: String,
    routes: HashMap<String, Vec<Answer>>,
    exit_after: Option<usize>,
    /// Where the dictionary is kept, if the script gives one.
    dictionary: Option<PathBuf>,
    /// A stop has come.
    stopped: bool,
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [script, discovery, requests] = &args[..] else {
        return ExitCode::from(64);
    };
    let Some(mut script) = std::fs::read_to_string(script)
        .ok()
        .and_then(|text| parse(&text))
    else {
        return ExitCode::from(64);
    };
    let Ok(listener) = TcpListener::bind("127.0.0.1:0") else {
        return ExitCode::from(70);
    };
    let Ok(port) = listener.local_addr().map(|address| address.port()) else {
        return ExitCode::from(70);
    };
    if write_discovery(Path::new(discovery), port, &script.discovery_token).is_err() {
        return ExitCode::from(73);
    }
    let heard = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let last = std::sync::Arc::clone(&heard);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let idle = last.lock().map_or(IDLE, |at| at.elapsed());
            if idle >= IDLE {
                std::process::exit(0);
            }
        }
    });
    let mut answered = 0;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if let Ok(mut at) = heard.lock() {
            *at = std::time::Instant::now();
        }
        if serve(stream, &mut script, Path::new(requests)).is_ok() {
            answered += 1;
        }
        if script.exit_after.is_some_and(|after| answered >= after) {
            return ExitCode::SUCCESS;
        }
    }
    ExitCode::SUCCESS
}

fn parse(text: &str) -> Option<Script> {
    let value: Value = serde_json::from_str(text).ok()?;
    let token = value
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or(TOKEN)
        .to_owned();
    let discovery_token = value
        .get("discovery_token")
        .and_then(Value::as_str)
        .map_or_else(|| token.clone(), str::to_owned);
    let mut routes = HashMap::new();
    for (route, answers) in value.get("routes")?.as_object()? {
        let answers = answers
            .as_array()?
            .iter()
            .map(|answer| {
                Some(Answer {
                    status: u16::try_from(answer.get("status")?.as_u64()?).ok()?,
                    body: answer
                        .get("body")
                        .map_or_else(String::new, Value::to_string),
                    delay: Duration::from_millis(
                        answer.get("delay_ms").and_then(Value::as_u64).unwrap_or(0),
                    ),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        routes.insert(route.clone(), answers);
    }
    let exit_after = value
        .get("exit_after")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok());
    let dictionary = match value.get("dictionary") {
        Some(given) => {
            let file = PathBuf::from(given.get("file")?.as_str()?);
            let kept =
                json!({"terms": given.get("terms")?, "corrections": given.get("corrections")?});
            std::fs::write(&file, kept.to_string()).ok()?;
            Some(file)
        }
        None => None,
    };
    Some(Script {
        token,
        discovery_token,
        routes,
        exit_after,
        dictionary,
        stopped: false,
    })
}

/// Writes the discovery file as TypeWhisper does: whole, then mode 0600.
fn write_discovery(path: &Path, port: u16, token: &str) -> std::io::Result<()> {
    let text = json!({ "port": port, "token": token, "version": 1 }).to_string();
    let partial = path.with_extension("partial");
    std::fs::write(&partial, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&partial, path)
}

/// Reads one request from `stream`, records it, and answers it as the script says.
fn serve(stream: TcpStream, script: &mut Script, requests: &Path) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new((&stream).take(MAX_REQUEST));
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let target = parts.next().unwrap_or("").to_owned();
    let path = target.split('?').next().unwrap_or("").to_owned();
    let mut length = 0;
    let mut authorized = false;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header == "\r\n" || header == "\n" {
            break;
        }
        let (name, value) = header.split_once(':').unwrap_or((&header, ""));
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().unwrap_or(0),
            "authorization" => authorized = value == format!("Bearer {}", script.token),
            _ => {}
        }
    }
    let mut body = vec![0; length.min(MAX_REQUEST as usize)];
    reader.read_exact(&mut body)?;
    let body = String::from_utf8_lossy(&body).into_owned();
    let record = json!({
        "method": method,
        "path": target,
        "authorized": authorized,
        "body": body,
    });
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(requests)?;
    // In one write, as a flow reads the file meanwhile.
    log.write_all(format!("{record}\n").as_bytes())?;
    let route = format!("{method} {path}");
    let answer = if authorized && route == TRANSCRIPTION && !script.stopped {
        Answer {
            status: 200,
            body: json!({"status": "recording"}).to_string(),
            delay: Duration::ZERO,
        }
    } else if authorized {
        script.stopped |= route == STOP;
        dictionary_answer(script, &route, &body).unwrap_or_else(|| next_answer(script, &route))
    } else {
        Answer {
            status: 401,
            body: json!({"error": {"code": "unauthorized", "message": "Missing or invalid API token"}})
                .to_string(),
            delay: Duration::ZERO,
        }
    };
    std::thread::sleep(answer.delay);
    let reason = if answer.status < 400 { "OK" } else { "Error" };
    let mut stream = stream;
    write!(
        stream,
        "HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        answer.status,
        answer.body.len(),
        answer.body
    )?;
    stream.flush()
}

/// The dictionary's answer to `route` with `body`, if the script keeps one and gives the route
/// no answers of its own.
fn dictionary_answer(script: &Script, route: &str, body: &str) -> Option<Answer> {
    let file = script.dictionary.as_ref()?;
    if script.routes.contains_key(route) {
        return None;
    }
    let mut kept: Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    let asked: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let same = |a: &Value, b: &Value| {
        let text = |value: &Value| value.as_str().map(str::to_lowercase);
        text(a).is_some() && text(a) == text(b)
    };
    let body = match route.split_once(' ')? {
        ("GET", path) => {
            let list = path.strip_prefix("/v1/dictionary/")?;
            json!({list: kept.get(list)?})
        }
        ("PUT", "/v1/dictionary/terms") => {
            let terms = kept["terms"].as_array_mut()?;
            for term in asked["terms"].as_array()? {
                terms.retain(|other| !same(other, term));
                terms.push(term.clone());
            }
            json!({"terms": terms})
        }
        ("PUT", "/v1/dictionary/corrections") => {
            let all = kept["corrections"].as_array_mut()?;
            all.retain(|other| !same(&other["original"], &asked["original"]));
            all.push(asked);
            json!({"corrections": all})
        }
        ("DELETE", "/v1/dictionary/terms") => {
            let terms = kept["terms"].as_array_mut()?;
            let before = terms.len();
            terms.retain(|other| !same(other, &asked["term"]));
            json!({"deleted": terms.len() < before})
        }
        ("DELETE", "/v1/dictionary/corrections") => {
            let all = kept["corrections"].as_array_mut()?;
            let before = all.len();
            all.retain(|other| !same(&other["original"], &asked["original"]));
            json!({"deleted": all.len() < before})
        }
        _ => return None,
    };
    if !route.starts_with("GET") {
        std::fs::write(file, kept.to_string()).ok()?;
    }
    Some(Answer {
        status: 200,
        body: body.to_string(),
        delay: Duration::ZERO,
    })
}

/// The route's next answer: the first left, or its last again; a 404 with no route.
fn next_answer(script: &mut Script, route: &str) -> Answer {
    match script.routes.get_mut(route) {
        Some(answers) if answers.len() > 1 => answers.remove(0),
        Some(answers) if !answers.is_empty() => answers[0].clone(),
        _ => Answer {
            status: 404,
            body: json!({"error": {"code": "not_found", "message": format!("Not found: {route}")}})
                .to_string(),
            delay: Duration::ZERO,
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{TOKEN, dictionary_answer, next_answer, parse};

    #[test]
    fn a_route_gives_its_answers_in_order_then_its_last_again() {
        let mut script = parse(
            r#"{"routes": {"GET /a": [{"status": 200, "body": {"n": 1}}, {"status": 409}]}}"#,
        )
        .unwrap();
        assert_eq!(script.token, TOKEN);
        assert_eq!(script.discovery_token, TOKEN);
        assert_eq!(next_answer(&mut script, "GET /a").body, r#"{"n":1}"#);
        assert_eq!(next_answer(&mut script, "GET /a").status, 409);
        assert_eq!(next_answer(&mut script, "GET /a").status, 409);
        assert_eq!(next_answer(&mut script, "GET /b").status, 404);
    }

    #[test]
    fn a_dictionary_is_kept_and_matched_without_regard_to_case() {
        let file = std::env::temp_dir().join(format!("fake-tw-{}.json", std::process::id()));
        let given = json!({"routes": {}, "dictionary": {
            "terms": ["Oso"], "corrections": [], "file": file,
        }});
        let script = parse(&given.to_string()).unwrap();
        let ask = |route: &str, body: &str| dictionary_answer(&script, route, body).unwrap().body;
        assert_eq!(ask("GET /v1/dictionary/terms", ""), r#"{"terms":["Oso"]}"#);
        let added = ask(
            "PUT /v1/dictionary/terms",
            r#"{"terms":["oso","Vite"],"replace":false}"#,
        );
        assert_eq!(added, r#"{"terms":["oso","Vite"]}"#);
        let gone = ask("DELETE /v1/dictionary/terms", r#"{"term":"OSO"}"#);
        assert_eq!(gone, r#"{"deleted":true}"#);
        assert_eq!(ask("GET /v1/dictionary/terms", ""), r#"{"terms":["Vite"]}"#);
        let put = r#"{"original":"a","replacement":"b","caseSensitive":true}"#;
        ask("PUT /v1/dictionary/corrections", put);
        let all: Value = serde_json::from_str(&ask("GET /v1/dictionary/corrections", "")).unwrap();
        assert_eq!(all["corrections"][0]["replacement"], "b");
        let gone = ask("DELETE /v1/dictionary/corrections", r#"{"original":"A"}"#);
        assert_eq!(gone, r#"{"deleted":true}"#);
        assert!(dictionary_answer(&script, "GET /v1/other", "").is_none());
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_bad_script_is_refused() {
        assert!(parse("[]").is_none());
        assert!(parse(r#"{"routes": {"GET /a": [{"body": 1}]}}"#).is_none());
    }
}
