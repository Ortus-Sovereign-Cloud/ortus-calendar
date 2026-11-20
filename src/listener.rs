//! A listener for calendar and contacts apps (the "dav" role task): plain
//! HTTP/1.1 on `ORTUS_DAV_LISTEN` (default 127.0.0.1:8008, for a proxy in
//! front), and with TLS on `ORTUS_DAVS_LISTEN` (default 0.0.0.0:8443)
//! when a certificate is set (`ORTUS_DAV_TLS_CERT`/`_KEY`, else the mail
//! one). It exists because the web port's server takes only the common
//! HTTP methods, and CalDAV needs PROPFIND, REPORT and the rest. Domains
//! publish `_caldavs._tcp` and `_carddavs._tcp` records pointing here.
//!
//! It serves only `/dav/calendar/…`, `/dav/people/…` and the two
//! `/.well-known/` names; requests are bounded in size and time.

use std::sync::Arc;
use std::time::Duration;

use ortus_core::instance::Instance;
use rust_web_server::header::Header;
use rust_web_server::prelude::*;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const MAX_HEAD: usize = 64 * 1024;
const MAX_BODY: usize = 4 * 1024 * 1024;
const IDLE: Duration = Duration::from_secs(60);
const MAX_CONNECTIONS: usize = 500;

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

fn tls() -> Option<Arc<tokio_rustls::rustls::ServerConfig>> {
    use tokio_rustls::rustls::pki_types::pem::PemObject;
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let cert = std::env::var("ORTUS_DAV_TLS_CERT")
        .or_else(|_| std::env::var("ORTUS_MAIL_TLS_CERT"))
        .ok()?;
    let key = std::env::var("ORTUS_DAV_TLS_KEY")
        .or_else(|_| std::env::var("ORTUS_MAIL_TLS_KEY"))
        .ok()?;
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(&cert)
        .ok()?
        .filter_map(Result::ok)
        .collect();
    let key = PrivateKeyDer::from_pem_file(&key).ok()?;
    let config = tokio_rustls::rustls::ServerConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .ok()?
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .map_err(|e| eprintln!("ortus: calendar: the DAV certificate: {e}"))
    .ok()?;
    Some(Arc::new(config))
}

/// The role task.
pub fn run(
    instance: Arc<Instance>,
    mut stop: ortus_core::app::Stop,
) -> ortus_core::storage::BoxFuture<'static, Result<(), String>> {
    Box::pin(async move {
        let plain = std::env::var("ORTUS_DAV_LISTEN").unwrap_or_else(|_| "127.0.0.1:8008".into());
        let listener = TcpListener::bind(&plain)
            .await
            .map_err(|e| format!("DAV on {plain}: {e}"))?;
        println!("ortus: calendar: CalDAV and CardDAV on {plain}");
        if let Some(config) = tls() {
            let secure =
                std::env::var("ORTUS_DAVS_LISTEN").unwrap_or_else(|_| "0.0.0.0:8443".into());
            let tls_listener = TcpListener::bind(&secure)
                .await
                .map_err(|e| format!("DAV on {secure}: {e}"))?;
            println!("ortus: calendar: CalDAV and CardDAV with TLS on {secure}");
            let (instance2, mut stop2) = (Arc::clone(&instance), stop.clone());
            tokio::spawn(async move {
                let _ = serve(instance2, tls_listener, &mut stop2, Some(config)).await;
            });
        }
        serve(instance, listener, &mut stop, None).await
    })
}

/// Accepts connections until `stop` turns true.
pub async fn serve(
    instance: Arc<Instance>,
    listener: TcpListener,
    stop: &mut ortus_core::app::Stop,
    tls: Option<Arc<tokio_rustls::rustls::ServerConfig>>,
) -> Result<(), String> {
    let open = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((socket, peer)) = accepted else {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                };
                let Ok(permit) = Arc::clone(&open).try_acquire_owned() else { continue };
                let (instance, tls) = (Arc::clone(&instance), tls.clone());
                tokio::spawn(async move {
                    let _permit = permit;
                    let stream: Box<dyn Stream> = match tls {
                        Some(config) => match tokio::time::timeout(Duration::from_secs(30), tokio_rustls::TlsAcceptor::from(config).accept(socket)).await {
                            Ok(Ok(s)) => Box::new(s),
                            _ => return,
                        },
                        None => Box::new(socket),
                    };
                    let _ = connection(instance, stream, peer).await;
                });
            }
            _ = stop.changed() => return Ok(()),
        }
    }
}

fn simple(
    code: &'static rust_web_server::response::StatusCodeReasonPhrase,
    text: &str,
) -> Response {
    Response::text(code, text)
}

/// One connection: requests one after another while the client keeps it.
async fn connection(
    instance: Arc<Instance>,
    stream: Box<dyn Stream>,
    peer: std::net::SocketAddr,
) -> Result<(), String> {
    let mut reader = BufReader::new(stream);
    loop {
        // The head: the request line and headers.
        let mut head = Vec::new();
        loop {
            let mut line = Vec::new();
            let read = tokio::time::timeout(
                IDLE,
                (&mut reader)
                    .take((MAX_HEAD - head.len()) as u64 + 1)
                    .read_until(b'\n', &mut line),
            )
            .await
            .map_err(|_| "idle")?
            .map_err(|e| e.to_string())?;
            if read == 0 {
                return Ok(());
            }
            head.extend_from_slice(&line);
            if head.len() > MAX_HEAD {
                return Err("head too big".into());
            }
            if line == b"\r\n" || line == b"\n" {
                if head.len() <= 2 {
                    head.clear();
                    continue;
                }
                break;
            }
        }
        let text = String::from_utf8_lossy(&head).into_owned();
        let mut lines = text.split("\r\n").flat_map(|l| l.split('\n'));
        let request_line = lines.next().unwrap_or_default().to_string();
        let mut parts = request_line.split_whitespace();
        let (method, target, version) = (
            parts.next().unwrap_or_default().to_ascii_uppercase(),
            parts.next().unwrap_or_default().to_string(),
            parts.next().unwrap_or("HTTP/1.1").to_string(),
        );
        let headers: Vec<Header> = lines
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| Header {
                name: n.trim().to_string().into(),
                value: v.trim().to_string().into(),
            })
            .collect();
        let find = |name: &str| {
            headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case(name))
                .map(|h| h.value.to_string())
        };
        // The body: by length, or chunked.
        let mut body = Vec::new();
        if find("transfer-encoding").is_some_and(|t| t.to_ascii_lowercase().contains("chunked")) {
            loop {
                let mut size_line = String::new();
                tokio::time::timeout(IDLE, (&mut reader).take(1024).read_line(&mut size_line))
                    .await
                    .map_err(|_| "idle")?
                    .map_err(|e| e.to_string())?;
                let size =
                    usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0"), 16)
                        .map_err(|_| "bad chunk")?;
                if size == 0 {
                    let mut end = String::new();
                    let _ = (&mut reader).take(1024).read_line(&mut end).await;
                    break;
                }
                if body.len() + size > MAX_BODY {
                    return Err("body too big".into());
                }
                let before = body.len();
                tokio::time::timeout(IDLE, (&mut reader).take(size as u64).read_to_end(&mut body))
                    .await
                    .map_err(|_| "idle")?
                    .map_err(|e| e.to_string())?;
                if body.len() - before != size {
                    return Err("short chunk".into());
                }
                let mut crlf = [0u8; 2];
                reader
                    .read_exact(&mut crlf)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        } else if let Some(length) = find("content-length") {
            let length: usize = length.trim().parse().map_err(|_| "bad length")?;
            if length > MAX_BODY {
                return Err("body too big".into());
            }
            tokio::time::timeout(
                IDLE,
                (&mut reader).take(length as u64).read_to_end(&mut body),
            )
            .await
            .map_err(|_| "idle")?
            .map_err(|e| e.to_string())?;
            if body.len() != length {
                return Err("short body".into());
            }
        }
        let keep = version == "HTTP/1.1"
            && !find("connection").is_some_and(|c| c.eq_ignore_ascii_case("close"));
        let request = Request {
            method: method.clone(),
            request_uri: target.clone(),
            http_version: version.clone(),
            headers,
            body: body.into(),
        };
        let response = route(&instance, request.clone(), peer).await;
        let bytes = Response::generate_response(response, request);
        let writer = reader.get_mut();
        writer.write_all(&bytes).await.map_err(|e| e.to_string())?;
        writer.flush().await.map_err(|e| e.to_string())?;
        if !keep {
            return Ok(());
        }
    }
}

async fn route(instance: &Arc<Instance>, request: Request, peer: std::net::SocketAddr) -> Response {
    let path = request
        .request_uri
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_string();
    let lowered = path.to_ascii_lowercase();
    if ["..", "%2e", "%2f", "%5c", "\\", "//", "%00"]
        .iter()
        .any(|t| lowered.contains(t))
    {
        return simple(STATUS_CODE_REASON_PHRASE.n404_not_found, "Not found");
    }
    let redirect = |to: &str| {
        let mut r = simple(STATUS_CODE_REASON_PHRASE.n301_moved_permanently, "");
        r.headers.push(Header {
            name: "Location".to_string().into(),
            value: to.to_string().into(),
        });
        r
    };
    match path.as_str() {
        "/.well-known/caldav" | "/.well-known/caldav/" => return redirect("/dav/calendar/"),
        "/.well-known/carddav" | "/.well-known/carddav/" => return redirect("/dav/people/"),
        "/" if request.method == "PROPFIND" => return redirect("/dav/calendar/"),
        _ => {}
    }
    let connection = ConnectionInfo {
        client: rust_web_server::server::Address {
            ip: peer.ip().to_string(),
            port: i32::from(peer.port()),
        },
        server: rust_web_server::server::Address {
            ip: String::new(),
            port: 0,
        },
        request_size: 0,
        sni_hostname: None,
        tls: false,
    };
    for (prefix, id) in [("/dav/calendar", "calendar"), ("/dav/people", "people")] {
        if path == prefix || path.starts_with(&format!("{prefix}/")) {
            if !instance.switches.is_on(id).await.unwrap_or(false) {
                break;
            }
            let rest = path[prefix.len()..].to_string();
            let handler = if id == "calendar" {
                crate::dav::calendar
            } else {
                crate::dav::people
            };
            return handler(request, connection, Arc::clone(instance), rest).await;
        }
    }
    simple(STATUS_CODE_REASON_PHRASE.n404_not_found, "Not found")
}
