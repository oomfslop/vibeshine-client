//! A small blocking HTTP/1.1 client for GameStream-style hosts.
//!
//! Hosts use a self-signed certificate that we pin after pairing, and expect
//! our client certificate on HTTPS requests. Neither fits a general-purpose
//! HTTP library well, so this does HTTP/1.1 with `Connection: close` over a
//! `TcpStream`, with rustls for TLS.
#![allow(dead_code)]

use std::fmt;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};

use super::host::Address;
use super::identity::Identity;
use super::xml;

pub const DEFAULT_HTTP_PORT: u16 = 47989;
pub const DEFAULT_HTTPS_PORT: u16 = 47984;

pub const FAST_FAIL_TIMEOUT: Duration = Duration::from_millis(2000);
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(5000);
pub const LAUNCH_TIMEOUT: Duration = Duration::from_millis(120_000);
pub const QUIT_TIMEOUT: Duration = Duration::from_millis(30_000);

#[derive(Debug, Clone)]
pub enum HttpError {
    /// The host answered with a non-200 `status_code` (GFE-style error) or an
    /// HTTP error status.
    Status { code: i32, message: String },
    /// The pinned certificate didn't match the one the host presented.
    CertMismatch,
    /// Connect or request timeout.
    Timeout { connecting: bool },
    /// The OS refused to route to the host (EHOSTUNREACH / ENETUNREACH). On
    /// macOS this is what a denied Local Network permission looks like.
    HostUnreachable,
    /// Any other transport error.
    Network(String),
    /// Aborted through a `CancelToken`.
    Cancelled,
}

impl HttpError {
    pub fn status_code(&self) -> Option<i32> {
        match self {
            HttpError::Status { code, .. } => Some(*code),
            _ => None,
        }
    }

    pub fn is_service_unavailable(&self) -> bool {
        matches!(self, HttpError::Status { code: 503, .. })
    }

    /// A platform-specific hint on how to fix this error, when there is one.
    pub fn resolution(&self) -> Option<&'static str> {
        crate::backend::platform::resolution_for(self)
    }

    /// The error text followed by its resolution hint, if any.
    pub fn with_resolution(&self) -> String {
        match self.resolution() {
            Some(r) => format!("{self}. {r}"),
            None => self.to_string(),
        }
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::Status { code, message } if message.is_empty() => write!(f, "Error {code}"),
            HttpError::Status { code, message } => write!(f, "{message} (Error {code})"),
            HttpError::CertMismatch => write!(f, "Server certificate mismatch"),
            HttpError::Timeout { connecting: true } => write!(f, "Connection timed out"),
            HttpError::Timeout { connecting: false } => write!(f, "Request timed out"),
            HttpError::HostUnreachable => write!(f, "No route to host"),
            HttpError::Network(e) => write!(f, "{e}"),
            HttpError::Cancelled => write!(f, "Cancelled"),
        }
    }
}

impl std::error::Error for HttpError {}

/// Verifier that accepts exactly one pinned certificate.
#[derive(Debug)]
struct PinnedCertVerifier {
    pinned: CertificateDer<'static>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.pinned.as_ref() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(rustls::CertificateError::ApplicationVerificationFailure))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Lets another thread abort an in-flight request by shutting down its socket.
#[derive(Default)]
pub struct CancelToken {
    inner: std::sync::Mutex<(bool, Option<TcpStream>)>,
}

impl CancelToken {
    pub fn cancel(&self) {
        let mut g = self.inner.lock().unwrap();
        g.0 = true;
        if let Some(s) = g.1.take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.lock().unwrap().0
    }

    fn register(&self, stream: &TcpStream) -> bool {
        let mut g = self.inner.lock().unwrap();
        if g.0 {
            return false;
        }
        g.1 = stream.try_clone().ok();
        true
    }
}

/// Client for one host. Cheap to clone.
#[derive(Clone)]
pub struct NvHttp {
    address: Address,
    https_port: u16,
    server_cert: Option<CertificateDer<'static>>,
    identity: Arc<Identity>,
    /// GFE expects a fixed placeholder unique ID so clients can quit each
    /// other's games. Sunshine/Apollo want the real one.
    use_true_uid: bool,
    tls: Option<Arc<ClientConfig>>,
    cancel: Option<Arc<CancelToken>>,
}

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl NvHttp {
    pub fn new(address: Address, https_port: u16, server_cert: Option<CertificateDer<'static>>, use_true_uid: bool, identity: Arc<Identity>) -> Self {
        let mut http = NvHttp { address, https_port, server_cert: None, identity, use_true_uid, tls: None, cancel: None };
        http.set_server_cert(server_cert);
        http
    }

    pub fn address(&self) -> &Address {
        &self.address
    }

    pub fn https_port(&self) -> u16 {
        self.https_port
    }

    pub fn server_cert(&self) -> Option<&CertificateDer<'static>> {
        self.server_cert.as_ref()
    }

    pub fn set_address(&mut self, address: Address) {
        self.address = address;
    }

    pub fn set_https_port(&mut self, port: u16) {
        self.https_port = port;
    }

    pub fn set_true_uid(&mut self, use_true_uid: bool) {
        self.use_true_uid = use_true_uid;
    }

    pub fn set_cancel_token(&mut self, token: Arc<CancelToken>) {
        self.cancel = Some(token);
    }

    pub fn set_server_cert(&mut self, cert: Option<CertificateDer<'static>>) {
        self.tls = cert.as_ref().map(|c| Arc::new(build_tls_config(c.clone(), &self.identity)));
        self.server_cert = cert;
    }

    fn unique_id(&self) -> &str {
        if self.use_true_uid { &self.identity.unique_id } else { "0123456789ABCDEF" }
    }

    /// Fetches `serverinfo`, preferring HTTPS when a certificate is pinned.
    /// Mirrors `NvHTTP::getServerInfo`.
    pub fn get_server_info(&mut self, fast_fail: bool) -> Result<String, HttpError> {
        let timeout = if fast_fail { FAST_FAIL_TIMEOUT } else { REQUEST_TIMEOUT };
        if self.server_cert.is_some() && self.https_port != 0 {
            match self.get_string(true, "serverinfo", "", timeout) {
                Ok(info) => match xml::verify_status(&info) {
                    Ok(()) => Ok(info),
                    Err(HttpError::Status { code: 401, .. }) => self.plain_server_info(timeout),
                    Err(e) => Err(e),
                },
                Err(HttpError::CertMismatch) | Err(HttpError::Status { code: 401, .. }) => self.plain_server_info(timeout),
                Err(e) => Err(e),
            }
        } else {
            let info = self.plain_server_info(timeout)?;
            let port = xml::get_string(&info, "HttpsPort").and_then(|p| p.parse::<u16>().ok()).filter(|p| *p != 0).unwrap_or(DEFAULT_HTTPS_PORT);
            self.https_port = port;
            if self.server_cert.is_some() {
                // Now that we know the HTTPS port, try again securely
                return self.get_server_info(fast_fail);
            }
            Ok(info)
        }
    }

    fn plain_server_info(&self, timeout: Duration) -> Result<String, HttpError> {
        let info = self.get_string(false, "serverinfo", "", timeout)?;
        xml::verify_status(&info)?;
        Ok(info)
    }

    pub fn get_app_list(&self) -> Result<Vec<super::host::App>, HttpError> {
        let xml_text = self.get_string(true, "applist", "", REQUEST_TIMEOUT)?;
        xml::verify_status(&xml_text)?;
        super::host::parse_app_list(&xml_text).map_err(HttpError::Network)
    }

    pub fn get_box_art(&self, app_id: i32) -> Result<Vec<u8>, HttpError> {
        let resp = self.request(true, "appasset", &format!("appid={app_id}&AssetType=2&AssetIdx=0"), REQUEST_TIMEOUT, None)?;
        Ok(resp.body)
    }

    /// Sends `cancel` and verifies the game actually stopped.
    pub fn quit_app(&mut self) -> Result<(), HttpError> {
        let resp = self.get_string(true, "cancel", "", QUIT_TIMEOUT).map_err(|e| {
            log::warn!("cancel request failed: {e:?}");
            e
        })?;
        log::info!("Quit response: {resp}");
        xml::verify_status(&resp)?;
        // Newer GFE returns success even if we weren't the original requester
        let info = self.get_server_info(false)?;
        if super::host::current_game(&info) != 0 {
            return Err(HttpError::Status { code: 599, message: String::new() });
        }
        Ok(())
    }

    pub fn get_clipboard_text(&self) -> Result<String, HttpError> {
        self.get_string(true, "actions/clipboard", "type=text", REQUEST_TIMEOUT)
    }

    pub fn set_clipboard_text(&self, text: &str) -> Result<(), HttpError> {
        self.request(true, "actions/clipboard", "type=text", REQUEST_TIMEOUT, Some(("text/plain; charset=utf-8", text.as_bytes())))?;
        Ok(())
    }

    /// GET and decode the body as UTF-8. `timeout == 0` disables the read
    /// timeout (pairing waits for the user to type a PIN on the host).
    pub fn get_string(&self, https: bool, command: &str, arguments: &str, timeout: Duration) -> Result<String, HttpError> {
        let resp = self.request(https, command, arguments, timeout, None)?;
        Ok(String::from_utf8_lossy(&resp.body).into_owned())
    }

    pub fn request(
        &self,
        https: bool,
        command: &str,
        arguments: &str,
        timeout: Duration,
        post: Option<(&str, &[u8])>,
    ) -> Result<Response, HttpError> {
        let port = if https { self.https_port } else { self.address.port };
        if port == 0 {
            return Err(HttpError::Network("port not known".into()));
        }
        let mut query = format!("uniqueid={}&uuid={}", self.unique_id(), uuid::Uuid::new_v4().simple());
        if !arguments.is_empty() {
            query.push('&');
            query.push_str(arguments);
        }
        let path = format!("/{command}?{query}");
        let url_for_log = format!("{}://{}:{}{}", if https { "https" } else { "http" }, self.address.host, port, path);
        log::debug!("Executing request: {url_for_log}");

        let start = Instant::now();
        if self.cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false) {
            return Err(HttpError::Cancelled);
        }
        let connect_timeout = if timeout.is_zero() { REQUEST_TIMEOUT } else { timeout.min(REQUEST_TIMEOUT) };
        let stream = connect(&self.address.host, port, connect_timeout)?;
        if let Some(c) = &self.cancel
            && !c.register(&stream) {
                return Err(HttpError::Cancelled);
            }
        let read_timeout = if timeout.is_zero() { None } else { Some(timeout.saturating_sub(start.elapsed()).max(Duration::from_millis(50))) };
        stream.set_read_timeout(read_timeout).ok();
        stream.set_write_timeout(Some(REQUEST_TIMEOUT)).ok();
        stream.set_nodelay(true).ok();

        let host_header = if self.address.host.contains(':') { format!("[{}]:{}", self.address.host, port) } else { format!("{}:{}", self.address.host, port) };
        let mut req = String::new();
        let method = if post.is_some() { "POST" } else { "GET" };
        req.push_str(&format!("{method} {path} HTTP/1.1\r\nHost: {host_header}\r\nUser-Agent: sloplight\r\nAccept: */*\r\nConnection: close\r\n"));
        if let Some((content_type, body)) = post {
            req.push_str(&format!("Content-Type: {content_type}\r\nContent-Length: {}\r\n", body.len()));
        }
        req.push_str("\r\n");

        let raw = if https {
            let config = self.tls.clone().ok_or_else(|| HttpError::Network("HTTPS request without a pinned certificate".into()))?;
            let server_name = ServerName::try_from(self.address.host.clone())
                .or_else(|_| ServerName::try_from("moonlight.invalid".to_string()))
                .map_err(|e| HttpError::Network(e.to_string()))?;
            let conn = ClientConnection::new(config, server_name).map_err(|e| HttpError::Network(e.to_string()))?;
            let mut tls = StreamOwned::new(conn, stream);
            write_all(&mut tls, req.as_bytes(), post.map(|p| p.1)).map_err(map_tls_io)?;
            read_to_end(&mut tls, read_timeout.is_some()).map_err(map_tls_io)?
        } else {
            let mut s = stream;
            write_all(&mut s, req.as_bytes(), post.map(|p| p.1)).map_err(map_io)?;
            read_to_end(&mut s, read_timeout.is_some()).map_err(map_io)?
        };

        if self.cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false) {
            return Err(HttpError::Cancelled);
        }
        let (status, body) = parse_response(&raw)?;
        if status >= 400 {
            let text = String::from_utf8_lossy(&body);
            if let Some((code, message)) = xml::root_status(&text) {
                log::warn!("{command} failed: {code} {message}");
                return Err(HttpError::Status { code, message });
            }
            log::warn!("{command} failed with HTTP {status}");
            return Err(HttpError::Status { code: status as i32, message: http_reason(status).to_string() });
        }
        Ok(Response { status, body })
    }
}

fn http_reason(status: u16) -> &'static str {
    match status {
        400 => "Bad request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not found",
        500 => "Internal server error",
        503 => "Service unavailable",
        _ => "HTTP error",
    }
}

fn map_io(e: std::io::Error) -> HttpError {
    match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => HttpError::Timeout { connecting: false },
        _ => HttpError::Network(e.to_string()),
    }
}

fn map_tls_io(e: std::io::Error) -> HttpError {
    // rustls surfaces certificate failures as InvalidData with an inner rustls::Error
    if e.kind() == std::io::ErrorKind::InvalidData {
        let msg = e.to_string();
        log::debug!("TLS error: {msg}");
        if msg.contains("certificate") || msg.contains("Certificate") || msg.contains("HandshakeFailure") || msg.contains("BadCertificate") {
            return HttpError::CertMismatch;
        }
        return HttpError::Network(msg);
    }
    map_io(e)
}

fn write_all<W: Write>(w: &mut W, head: &[u8], body: Option<&[u8]>) -> std::io::Result<()> {
    w.write_all(head)?;
    if let Some(body) = body {
        w.write_all(body)?;
    }
    w.flush()
}

fn read_to_end<R: Read>(r: &mut R, has_timeout: bool) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 8192];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if response_complete(&buf) {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            // rustls close_notify is sometimes missing; a clean TCP close is fine
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && !buf.is_empty() => break,
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionAborted && !buf.is_empty() => break,
            Err(e) => {
                if has_timeout && (e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut) && response_complete(&buf) {
                    break;
                }
                return Err(e);
            }
        }
    }
    Ok(buf)
}

/// True when `raw` holds a full response (headers plus Content-Length body or
/// a terminated chunked body).
fn response_complete(raw: &[u8]) -> bool {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut resp = httparse::Response::new(&mut headers);
    let Ok(httparse::Status::Complete(header_len)) = resp.parse(raw) else {
        return false;
    };
    let body = &raw[header_len..];
    let mut content_length = None;
    let mut chunked = false;
    for h in resp.headers.iter() {
        if h.name.eq_ignore_ascii_case("content-length") {
            content_length = std::str::from_utf8(h.value).ok().and_then(|v| v.trim().parse::<usize>().ok());
        } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = std::str::from_utf8(h.value).map(|v| v.to_ascii_lowercase().contains("chunked")).unwrap_or(false);
        }
    }
    if let Some(len) = content_length {
        body.len() >= len
    } else if chunked {
        body.ends_with(b"0\r\n\r\n")
    } else {
        false
    }
}

fn parse_response(raw: &[u8]) -> Result<(u16, Vec<u8>), HttpError> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut resp = httparse::Response::new(&mut headers);
    let header_len = match resp.parse(raw) {
        Ok(httparse::Status::Complete(n)) => n,
        Ok(httparse::Status::Partial) => return Err(HttpError::Network("incomplete HTTP response".into())),
        Err(e) => return Err(HttpError::Network(format!("bad HTTP response: {e}"))),
    };
    let status = resp.code.unwrap_or(0);
    let mut content_length = None;
    let mut chunked = false;
    for h in resp.headers.iter() {
        if h.name.eq_ignore_ascii_case("content-length") {
            content_length = std::str::from_utf8(h.value).ok().and_then(|v| v.trim().parse::<usize>().ok());
        } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = std::str::from_utf8(h.value).map(|v| v.to_ascii_lowercase().contains("chunked")).unwrap_or(false);
        }
    }
    let body = &raw[header_len..];
    let body = if chunked {
        decode_chunked(body)
    } else if let Some(len) = content_length {
        body[..body.len().min(len)].to_vec()
    } else {
        body.to_vec()
    };
    Ok((status, body))
}

fn decode_chunked(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    loop {
        let Some(line_end) = body.windows(2).position(|w| w == b"\r\n") else { break };
        let size_str = std::str::from_utf8(&body[..line_end]).unwrap_or("0");
        let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
        body = &body[line_end + 2..];
        if size == 0 || body.len() < size {
            break;
        }
        out.extend_from_slice(&body[..size]);
        body = body.get(size + 2..).unwrap_or(&[]);
    }
    out
}

fn connect(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, HttpError> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| HttpError::Network(format!("Could not resolve {host}: {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(HttpError::Network(format!("Could not resolve {host}")));
    }
    let mut last_err = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => return Ok(s),
            Err(e) => last_err = Some(e),
        }
    }
    let e = last_err.unwrap();
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => Err(HttpError::Timeout { connecting: true }),
        std::io::ErrorKind::HostUnreachable | std::io::ErrorKind::NetworkUnreachable => Err(HttpError::HostUnreachable),
        _ if is_unreachable_errno(&e) => Err(HttpError::HostUnreachable),
        _ => Err(HttpError::Network(e.to_string())),
    }
}

fn is_unreachable_errno(e: &std::io::Error) -> bool {
    // EHOSTUNREACH / ENETUNREACH: 65 / 51 on the BSDs and macOS, 113 / 101 on Linux.
    matches!(e.raw_os_error(), Some(65) | Some(51) | Some(113) | Some(101))
}

fn build_tls_config(pinned: CertificateDer<'static>, identity: &Identity) -> ClientConfig {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedCertVerifier { pinned, provider: provider.clone() });
    let builder = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("rustls protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(verifier);
    let mut config = builder
        .with_client_auth_cert(vec![identity.cert_der().clone()], identity.key_der())
        .expect("client certificate");
    config.enable_sni = false;
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore]
    fn fake_host_https() {
        let _ = env_logger::try_init();
        let s = std::env::var("FAKEHOST_DIR").unwrap();
        let cert = super::super::identity::pem_to_der(&std::fs::read_to_string(format!("{s}/cert.pem")).unwrap()).unwrap();
        let identity = Identity::load_or_create(std::path::Path::new(&format!("{s}/id"))).unwrap();
        let http = NvHttp::new(Address::new("127.0.0.1", 47989), 47984, Some(cert), true, identity);
        match http.get_string(true, "serverinfo", "", REQUEST_TIMEOUT) {
            Ok(s) => println!("OK {}", &s[..60]),
            Err(e) => println!("ERR {e:?}"),
        }
    }
}
