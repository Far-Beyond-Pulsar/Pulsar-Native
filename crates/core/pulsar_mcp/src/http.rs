//! The HTTP side: one request in, one response out (`Connection: close`).
//!
//! This is the Streamable HTTP transport's request/response half: a client
//! `POST`s a JSON-RPC message to `/mcp` and reads the JSON answer (or a `202`
//! for a notification). `GET`/`DELETE` on `/mcp` are answered `405`: the server
//! has no stream to offer and no session to end.
//!
//! Defences, in the order a request meets them:
//!
//! 1. **Host** must be this server's own loopback address and port, and
//!    **Origin**, when the client sends one, must be a loopback origin. A web
//!    page in the person's browser can reach `127.0.0.1` and can steer a DNS
//!    name to it; neither survives these two checks (`403`).
//! 2. A **bearer token** the server generated and wrote to the endpoint file
//!    (`401`). Compared in constant time.
//! 3. Size limits: headers, body, and the time to send them.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::protocol::{self, Reply};

/// Most header bytes read before the request is refused.
const MAX_HEADERS: usize = 64 * 1024;
/// Largest request body.
const MAX_BODY: usize = 16 * 1024 * 1024;
/// How long a client may take to send its request.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// What a connection is checked against.
#[derive(Clone)]
pub(crate) struct Policy {
    pub port: u16,
    pub token: String,
}

/// A parsed request.
pub(crate) struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// An HTTP response.
pub(crate) struct Response {
    pub status: u16,
    pub reason: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Response {
    fn new(status: u16, reason: &'static str) -> Self {
        Self {
            status,
            reason,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn text(status: u16, reason: &'static str, text: &str) -> Self {
        let mut response = Self::new(status, reason);
        response
            .headers
            .push(("Content-Type", "text/plain; charset=utf-8".into()));
        response.body = text.as_bytes().to_vec();
        response
    }

    fn json(body: String) -> Self {
        let mut response = Self::new(200, "OK");
        response
            .headers
            .push(("Content-Type", "application/json".into()));
        response.body = body.into_bytes();
        response
    }

    fn write_to(&self, stream: &mut impl Write) -> io::Result<()> {
        write!(stream, "HTTP/1.1 {} {}\r\n", self.status, self.reason)?;
        for (name, value) in &self.headers {
            write!(stream, "{name}: {value}\r\n")?;
        }
        write!(
            stream,
            "Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
            self.body.len()
        )?;
        stream.write_all(&self.body)?;
        stream.flush()
    }
}

/// Read one request from `stream`.
pub(crate) fn read_request(stream: &mut TcpStream) -> Result<Request, Response> {
    stream.set_read_timeout(Some(READ_TIMEOUT)).ok();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let mut used = 0usize;
    let mut next_line = |reader: &mut BufReader<&mut TcpStream>, line: &mut String| {
        line.clear();
        match reader.read_line(line) {
            Ok(0) => Err(Response::text(400, "Bad Request", "the connection closed mid-request")),
            Ok(n) => {
                used += n;
                if used > MAX_HEADERS {
                    Err(Response::text(431, "Request Header Fields Too Large", "headers too large"))
                } else {
                    Ok(())
                }
            }
            Err(error) => Err(Response::text(408, "Request Timeout", &error.to_string())),
        }
    };
    next_line(&mut reader, &mut line)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return Err(Response::text(400, "Bad Request", "not an HTTP request line"));
    };
    let (method, path) = (method.to_owned(), path.to_owned());
    let mut headers = Vec::new();
    loop {
        next_line(&mut reader, &mut line)?;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    let length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.parse::<usize>())
        .transpose()
        .map_err(|_| Response::text(400, "Bad Request", "bad Content-Length"))?
        .unwrap_or(0);
    if length > MAX_BODY {
        return Err(Response::text(413, "Payload Too Large", "body too large"));
    }
    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .map_err(|error| Response::text(400, "Bad Request", &format!("short body: {error}")))?;
    Ok(Request {
        method,
        path,
        headers,
        body,
    })
}

/// The host part of a `Host` header or an origin's authority, without port.
fn host_of(authority: &str) -> (&str, Option<u16>) {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']').unwrap_or((rest, ""));
        let port = tail.strip_prefix(':').and_then(|p| p.parse().ok());
        return (host, port);
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => (host, port.parse().ok()),
        _ => (authority, None),
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(
        host.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

/// `Host` is this server, on loopback.
pub(crate) fn host_allowed(host: Option<&str>, port: u16) -> bool {
    let Some(host) = host else { return false };
    let (name, given) = host_of(host.trim());
    is_loopback_host(name) && given == Some(port)
}

/// `Origin`, when present, is a loopback origin (any port: another local tool).
pub(crate) fn origin_allowed(origin: Option<&str>) -> bool {
    let Some(origin) = origin else { return true };
    let origin = origin.trim();
    let Some(rest) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or("");
    is_loopback_host(host_of(authority).0)
}

/// Equal strings, in time that does not depend on where they differ.
pub(crate) fn same_secret(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut difference = a.len() ^ b.len();
    for index in 0..a.len().max(b.len()) {
        difference |= usize::from(
            a.get(index).copied().unwrap_or(0) ^ b.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

fn authorized(request: &Request, token: &str) -> bool {
    request
        .header("authorization")
        .and_then(|value| value.strip_prefix("Bearer ").or_else(|| value.strip_prefix("bearer ")))
        .is_some_and(|given| same_secret(given.trim(), token))
}

/// Decide what a request gets.
pub(crate) fn respond(request: &Request, policy: &Policy) -> Response {
    if !host_allowed(request.header("host"), policy.port) {
        return Response::text(
            403,
            "Forbidden",
            "the Host header must be this server's loopback address (127.0.0.1, localhost or [::1]) and port",
        );
    }
    if !origin_allowed(request.header("origin")) {
        return Response::text(403, "Forbidden", "the Origin is not a local origin");
    }
    if !authorized(request, &policy.token) {
        let mut response = Response::text(
            401,
            "Unauthorized",
            "send `Authorization: Bearer <token>`; the token is in the endpoint file the editor wrote (see .agents/MCP.md)",
        );
        response
            .headers
            .push(("WWW-Authenticate", "Bearer realm=\"pulsar\"".into()));
        return response;
    }
    let path = request.path.split('?').next().unwrap_or("");
    if path != "/mcp" && path != "/mcp/" {
        return Response::text(404, "Not Found", "the MCP endpoint is /mcp");
    }
    match request.method.as_str() {
        "POST" => match std::str::from_utf8(&request.body) {
            Ok(body) => match protocol::handle(body) {
                Reply::None => Response::new(202, "Accepted"),
                Reply::Json(answer) => Response::json(answer),
            },
            Err(_) => Response::text(400, "Bad Request", "the body is not UTF-8"),
        },
        "GET" | "DELETE" => {
            let mut response = Response::text(405, "Method Not Allowed", "POST JSON-RPC messages to /mcp");
            response.headers.push(("Allow", "POST".into()));
            response
        }
        _ => Response::text(405, "Method Not Allowed", "POST JSON-RPC messages to /mcp"),
    }
}

/// Serve one connection.
pub(crate) fn serve(mut stream: TcpStream, policy: &Policy) {
    let response = match read_request(&mut stream) {
        Ok(request) => respond(&request, policy),
        Err(response) => response,
    };
    if let Err(error) = response.write_to(&mut stream) {
        tracing::debug!(%error, "mcp: could not write a response");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_must_be_loopback_with_the_servers_port() {
        assert!(host_allowed(Some("127.0.0.1:4000"), 4000));
        assert!(host_allowed(Some("localhost:4000"), 4000));
        assert!(host_allowed(Some("[::1]:4000"), 4000));
        assert!(!host_allowed(Some("127.0.0.1:4001"), 4000));
        assert!(!host_allowed(Some("evil.example:4000"), 4000));
        assert!(!host_allowed(Some("127.0.0.1"), 4000), "a port is required");
        assert!(!host_allowed(None, 4000));
        // A name that merely starts like loopback.
        assert!(!host_allowed(Some("localhost.evil.example:4000"), 4000));
    }

    #[test]
    fn origin_must_be_loopback_when_present() {
        assert!(origin_allowed(None));
        assert!(origin_allowed(Some("http://localhost:3000")));
        assert!(origin_allowed(Some("http://127.0.0.1")));
        assert!(origin_allowed(Some("https://[::1]:8443")));
        assert!(!origin_allowed(Some("https://evil.example")));
        assert!(!origin_allowed(Some("null")));
        assert!(!origin_allowed(Some("http://localhost.evil.example")));
        assert!(!origin_allowed(Some("vscode-webview://abc")));
    }

    #[test]
    fn secrets_compare_equal_only_when_equal() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abd"));
        assert!(!same_secret("abc", "abcd"));
        assert!(!same_secret("", "a"));
    }
}
