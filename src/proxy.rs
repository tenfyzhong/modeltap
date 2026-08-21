use crate::config::{Config, SiteConfig};
use crate::egress::EgressConnector;
use crate::logging::{BODY_PREVIEW_LIMIT, body_preview};
use crate::mitm::MitmAuthority;
use crate::pricing::PriceBook;
use crate::telemetry::Telemetry;
use crate::usage::{
    AutoStreamUsageParser, CursorUsageParser, WebSocketUsageParser, auto_parse_json,
    permessage_deflate_server_no_context_takeover,
};
use bytes::Bytes;
use http::header::{
    CONNECTION, HOST, PROXY_AUTHORIZATION, SEC_WEBSOCKET_EXTENSIONS, UPGRADE, USER_AGENT,
};
use http::{HeaderValue, Request, Response, Uri};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ServerBuilder;
use std::convert::Infallible;
use std::io;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, warn};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type ProxyBody = BoxBody<Bytes, BoxError>;

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid proxy request: {0}")]
    Request(String),
    #[error("egress configuration error: {0}")]
    Egress(String),
}

pub async fn run(
    config: Arc<Config>,
    mitm_authority: Option<Arc<MitmAuthority>>,
    telemetry: Option<Arc<Telemetry>>,
    prices: Arc<PriceBook>,
) -> Result<(), ProxyError> {
    let listener = tokio::net::TcpListener::bind(&config.proxy.listen).await?;
    loop {
        let (stream, _) = listener.accept().await?;
        let config = config.clone();
        let mitm_authority = mitm_authority.clone();
        let telemetry = telemetry.clone();
        let prices = prices.clone();
        tokio::spawn(async move {
            let _ =
                handle_connection_with_telemetry(stream, config, mitm_authority, telemetry, prices)
                    .await;
        });
    }
}

pub async fn handle_connection(client: TcpStream, config: Arc<Config>) -> Result<(), ProxyError> {
    handle_connection_with_mitm(client, config, None).await
}

pub async fn handle_connection_with_mitm(
    client: TcpStream,
    config: Arc<Config>,
    mitm_authority: Option<Arc<MitmAuthority>>,
) -> Result<(), ProxyError> {
    handle_connection_with_telemetry(
        client,
        config,
        mitm_authority,
        None,
        Arc::new(
            PriceBook::from_config(&crate::pricing::PricingConfig {
                timezone: "UTC".to_owned(),
                peak_windows: Vec::new(),
                rules: Vec::new(),
            })
            .expect("UTC pricing configuration is valid"),
        ),
    )
    .await
}

async fn handle_connection_with_telemetry(
    mut client: TcpStream,
    config: Arc<Config>,
    mitm_authority: Option<Arc<MitmAuthority>>,
    telemetry: Option<Arc<Telemetry>>,
    prices: Arc<PriceBook>,
) -> Result<(), ProxyError> {
    let request = read_headers(&mut client, 32 * 1024).await?;
    let (method, target) = parse_request_line(&request)?;
    if method != "CONNECT" {
        client
            .write_all(b"HTTP/1.1 501 Not Implemented\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    }
    let authority = http::uri::Authority::from_str(target)
        .map_err(|_| ProxyError::Request(format!("invalid CONNECT target {target}")))?;
    if authority.port_u16().is_none() {
        return Err(ProxyError::Request(
            "CONNECT target must include a port".to_owned(),
        ));
    }
    let site = config.site_for_host(authority.host());
    let connector = connector_for_site(&config, site)?;
    debug!(
        target = target,
        site = site.map(|site| site.id.as_str()).unwrap_or("unconfigured"),
        mitm = site.is_some(),
        "processing CONNECT request"
    );
    if site.is_some() {
        let mitm_authority = mitm_authority.ok_or_else(|| {
            ProxyError::Egress("a MITM authority is required for a configured site".to_owned())
        })?;
        let host = authority.host().to_owned();
        let upstream_base = format!("https://{target}")
            .parse::<Uri>()
            .map_err(|_| ProxyError::Request(format!("invalid upstream URI {target}")))?;
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        client.flush().await?;
        let tls = TlsAcceptor::from(Arc::new(
            mitm_authority
                .server_config_for(&host)
                .map_err(|error| ProxyError::Egress(error.to_string()))?,
        ))
        .accept(client)
        .await
        .map_err(|error| ProxyError::Request(format!("MITM TLS handshake failed: {error}")))?;
        debug!(target = target, "MITM TLS handshake completed");
        let observer = site.and_then(|site| {
            telemetry.as_ref().map(|telemetry| UsageObserver {
                telemetry: telemetry.clone(),
                prices,
                site: site.id.clone(),
                agent_cli: "unknown".to_owned(),
            })
        });
        return serve_mitm_connection(tls, upstream_base, connector, observer).await;
    }
    let mut upstream = match connector.connect(target).await {
        Ok(stream) => stream,
        Err(error) if site.is_some_and(|site| site.direct_fallback) => EgressConnector::direct()
            .connect(target)
            .await
            .map_err(|direct_error| {
                ProxyError::Egress(format!(
                    "egress failed: {error}; direct fallback failed: {direct_error}"
                ))
            })?,
        Err(error) => {
            debug!(target = target, error = %error, "upstream connection failed");
            client
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                )
                .await?;
            let _ = error;
            return Ok(());
        }
    };
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    client.flush().await?;
    debug!(
        target = target,
        "starting transparent bidirectional forwarding"
    );
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

pub async fn serve_mitm_connection<S>(
    stream: S,
    upstream_base: Uri,
    egress: EgressConnector,
    observer: Option<UsageObserver>,
) -> Result<(), ProxyError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = service_fn(move |request| {
        forward_mitm_request(
            request,
            upstream_base.clone(),
            egress.clone(),
            observer.clone(),
        )
    });
    ServerBuilder::new(TokioExecutor::new())
        .serve_connection_with_upgrades(TokioIo::new(stream), service)
        .await
        .map_err(|error| ProxyError::Request(format!("MITM HTTP connection failed: {error}")))
}

async fn forward_mitm_request(
    mut request: Request<Incoming>,
    upstream_base: Uri,
    egress: EgressConnector,
    observer: Option<UsageObserver>,
) -> Result<Response<ProxyBody>, Infallible> {
    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let uri = match Uri::builder()
        .scheme(upstream_base.scheme_str().unwrap_or("https"))
        .authority(
            upstream_base
                .authority()
                .map(|value| value.as_str())
                .unwrap_or_default(),
        )
        .path_and_query(path_and_query)
        .build()
    {
        Ok(uri) => uri,
        Err(_) => {
            return Ok(error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid target URI",
            ));
        }
    };
    *request.uri_mut() = uri;
    request.headers_mut().remove(PROXY_AUTHORIZATION);
    if let Some(authority) = upstream_base.authority() {
        if let Ok(value) = HeaderValue::from_str(authority.as_str()) {
            request.headers_mut().insert(HOST, value);
        }
    }
    let method = request.method().clone();
    let target = request.uri().clone();
    let is_cursor_connect = request
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/connect+proto"));
    let is_oh_my_pi_cursor_request = is_cursor_connect
        && request
            .headers()
            .get("x-ghost-mode")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == "true");
    let is_oh_my_pi_cursor_cli_request = is_cursor_connect
        && request
            .headers()
            .get("x-cursor-client-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("cli"));
    let agent_cli = agent_cli(
        request
            .headers()
            .get(USER_AGENT)
            .and_then(|value| value.to_str().ok()),
        is_oh_my_pi_cursor_request,
        is_oh_my_pi_cursor_cli_request,
    );
    let observer = observer.map(|mut observer| {
        observer.agent_cli = agent_cli;
        observer
    });
    let cursor_usage = is_cursor_connect.then(|| Arc::new(Mutex::new(CursorUsageParser::new())));
    let cursor_request_observer = is_cursor_connect.then_some(observer.clone()).flatten();
    let upgrade_requested = websocket_upgrade_requested(&request);
    let client_upgrade = upgrade_requested.then(|| hyper::upgrade::on(&mut request));
    debug!(method = %method, target = %target, "forwarding MITM HTTP request");
    let request_method = method.clone();
    let request_target = target.clone();
    let cursor_request_parser = cursor_usage.clone();
    let request = request.map(move |body| {
        body.map_frame(move |frame| {
            if let Some(data) = frame.data_ref() {
                debug!(
                    method = %request_method,
                    target = %request_target,
                    bytes = data.len(),
                    content = %body_preview(data, BODY_PREVIEW_LIMIT),
                    "processing request body chunk"
                );
                if let (Some(parser), Some(observer)) = (
                    cursor_request_parser.as_ref(),
                    cursor_request_observer.as_ref(),
                ) {
                    if let Ok(mut parser) = parser.lock() {
                        if let Some(model) = parser.push_request(data) {
                            if !parser.request_reported() {
                                parser.mark_request_reported();
                                observer.record(&model, &crate::usage::TokenUsage::default());
                            }
                        }
                    }
                }
            }
            frame
        })
        .map_err(box_error)
        .boxed()
    });
    let request_started = Instant::now();
    match send_upstream_request(&egress, request, upgrade_requested).await {
        Ok(mut response) => {
            if let Some(observer) = observer.as_ref() {
                observer.telemetry.record_response_duration(
                    &observer.site,
                    request_started.elapsed().as_secs_f64(),
                );
            }
            let is_sse = response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("text/event-stream"));
            debug!(
                method = %method,
                target = %target,
                status = %response.status(),
                sse = is_sse,
                upstream_first_response_ms = request_started.elapsed().as_secs_f64() * 1_000.0,
                "received upstream response headers"
            );
            if response.status() == http::StatusCode::SWITCHING_PROTOCOLS {
                if let Some(client_upgrade) = client_upgrade {
                    let permessage_deflate = response
                        .headers()
                        .get_all(SEC_WEBSOCKET_EXTENSIONS)
                        .iter()
                        .filter_map(|value| value.to_str().ok())
                        .find_map(permessage_deflate_server_no_context_takeover);
                    let upstream_upgrade = hyper::upgrade::on(&mut response);
                    let websocket_observer = observer.clone();
                    tokio::spawn(async move {
                        match tokio::try_join!(client_upgrade, upstream_upgrade) {
                            Ok((client, upstream)) => {
                                if let Err(error) = tunnel_websocket(
                                    TokioIo::new(client),
                                    TokioIo::new(upstream),
                                    websocket_observer,
                                    permessage_deflate,
                                )
                                .await
                                {
                                    debug!(error = %error, "WebSocket upgrade tunnel failed");
                                }
                            }
                            Err(error) => {
                                debug!(error = %error, "WebSocket upgrade handshake failed");
                            }
                        }
                    });
                }
            }
            let mut parser = observer
                .clone()
                .filter(|_| is_sse)
                .map(|observer| (AutoStreamUsageParser::new(), observer));
            let cursor_observer = is_cursor_connect.then_some(observer.clone()).flatten();
            let direct_observer = (!is_sse && !is_cursor_connect)
                .then_some(observer)
                .flatten();
            let mut direct_recorded = false;
            Ok(response.map(move |body| {
                body.map_frame(move |frame| {
                    if let Some(data) = frame.data_ref() {
                        debug!(
                            bytes = data.len(),
                            content = %body_preview(data, BODY_PREVIEW_LIMIT),
                            "processing response body chunk"
                        );
                    }
                    if let (Some((stream, observer)), Some(data)) =
                        (parser.as_mut(), frame.data_ref())
                    {
                        if let Some((_protocol, usage)) = stream.push(data) {
                            observer
                                .record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
                        }
                    } else if let (Some(parser), Some(observer), Some(data)) = (
                        cursor_usage.as_ref(),
                        cursor_observer.as_ref(),
                        frame.data_ref(),
                    ) {
                        if let Ok(mut parser) = parser.lock() {
                            if let Some(usage) = parser.push_response(data) {
                                let model = usage.model.as_deref().unwrap_or("unknown");
                                if parser.request_reported() {
                                    observer.record_tokens(model, &usage.tokens);
                                } else {
                                    parser.mark_request_reported();
                                    observer.record(model, &usage.tokens);
                                }
                            }
                        }
                    } else if let (Some(observer), Some(data)) =
                        (direct_observer.as_ref(), frame.data_ref())
                    {
                        if !direct_recorded {
                            if let Some((_protocol, usage)) = auto_parse_json(data) {
                                observer.record(
                                    usage.model.as_deref().unwrap_or("unknown"),
                                    &usage.tokens,
                                );
                                direct_recorded = true;
                            }
                        }
                    }
                    frame
                })
                .map_err(box_error)
                .boxed()
            }))
        }
        Err(error) => {
            warn!(method = %method, target = %target, error = %error, "MITM upstream request failed");
            Ok(error_response(
                http::StatusCode::BAD_GATEWAY,
                "upstream connection failed",
            ))
        }
    }
}

async fn send_upstream_request(
    egress: &EgressConnector,
    mut request: Request<ProxyBody>,
    force_http1: bool,
) -> Result<Response<Incoming>, BoxError> {
    let uri = request.uri().clone();
    let stream = if force_http1 {
        egress.connect_uri_http1(&uri).await?
    } else {
        egress.connect_uri(&uri).await?
    };
    if !force_http1 && stream.negotiated_h2() {
        *request.version_mut() = http::Version::HTTP_2;
        request.headers_mut().remove(HOST);
        let (mut sender, connection) =
            hyper::client::conn::http2::Builder::new(TokioExecutor::new())
                .handshake(TokioIo::new(stream))
                .await?;
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                debug!(error = %error, "HTTP/2 upstream connection failed");
            }
        });
        return sender.send_request(request).await.map_err(box_error);
    }

    *request.version_mut() = http::Version::HTTP_11;
    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    *request.uri_mut() = Uri::builder().path_and_query(path_and_query).build()?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    let connection = connection.with_upgrades();
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            debug!(error = %error, "HTTP/1.1 upstream connection failed");
        }
    });
    sender.send_request(request).await.map_err(box_error)
}

async fn tunnel_websocket<C, U>(
    client: C,
    upstream: U,
    observer: Option<UsageObserver>,
    permessage_deflate: Option<bool>,
) -> io::Result<()>
where
    C: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    U: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (mut upstream_read, mut upstream_write) = tokio::io::split(upstream);
    let client_to_upstream = async move {
        tokio::io::copy(&mut client_read, &mut upstream_write).await?;
        upstream_write.shutdown().await
    };
    let upstream_to_client = async move {
        let mut parser = observer.clone().map(|observer| {
            let parser = match permessage_deflate {
                Some(server_no_context_takeover) => {
                    WebSocketUsageParser::with_permessage_deflate(server_no_context_takeover)
                }
                None => WebSocketUsageParser::new(),
            };
            (parser, observer)
        });
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            let read = upstream_read.read(&mut buffer).await?;
            if read == 0 {
                return client_write.shutdown().await;
            }
            let data = &buffer[..read];
            debug!(
                bytes = data.len(),
                content = %body_preview(data, BODY_PREVIEW_LIMIT),
                "processing WebSocket server frame bytes"
            );
            if let Some((parser, observer)) = parser.as_mut() {
                if let Some((_protocol, usage)) = parser.push(data) {
                    observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
                }
            }
            client_write.write_all(data).await?;
        }
    };
    tokio::try_join!(client_to_upstream, upstream_to_client).map(|_| ())
}

#[derive(Clone)]
pub struct UsageObserver {
    telemetry: Arc<Telemetry>,
    prices: Arc<PriceBook>,
    site: String,
    agent_cli: String,
}

impl UsageObserver {
    fn record(&self, model: &str, usage: &crate::usage::TokenUsage) {
        self.telemetry
            .record_usage(&self.site, model, &self.agent_cli, usage, &self.prices);
    }

    fn record_tokens(&self, model: &str, usage: &crate::usage::TokenUsage) {
        self.telemetry
            .record_usage_tokens(&self.site, model, &self.agent_cli, usage, &self.prices);
    }
}

fn agent_cli(
    user_agent: Option<&str>,
    is_oh_my_pi_cursor_request: bool,
    is_oh_my_pi_cursor_cli_request: bool,
) -> String {
    let user_agent = user_agent.unwrap_or_default().to_ascii_lowercase();
    if is_oh_my_pi_cursor_request
        || is_oh_my_pi_cursor_cli_request
        || user_agent.contains("oh-my-pi")
        || user_agent.contains("oh_my_pi")
    {
        "oh_my_pi".to_owned()
    } else if user_agent.contains("claude") {
        "claude_code".to_owned()
    } else if user_agent.contains("codex") {
        "codex".to_owned()
    } else if user_agent.contains("gemini-cli") {
        "gemini_cli".to_owned()
    } else if user_agent.contains("opencode") {
        "opencode".to_owned()
    } else {
        "unknown".to_owned()
    }
}

fn error_response(status: http::StatusCode, message: &'static str) -> Response<ProxyBody> {
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(
            Full::new(Bytes::from_static(message.as_bytes()))
                .map_err(|never: Infallible| match never {})
                .boxed(),
        )
        .expect("static error response is valid")
}

fn box_error<E>(error: E) -> BoxError
where
    E: std::error::Error + Send + Sync + 'static,
{
    Box::new(error)
}

fn websocket_upgrade_requested(request: &Request<Incoming>) -> bool {
    request.headers().contains_key(UPGRADE)
        && request
            .headers()
            .get(CONNECTION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
            })
}

fn connector_for_site(
    config: &Config,
    site: Option<&SiteConfig>,
) -> Result<EgressConnector, ProxyError> {
    let site_id = site.map(|site| site.id.as_str()).unwrap_or("__unknown__");
    let resolved = config
        .egress_for_site(site_id)
        .map_err(|error| ProxyError::Egress(error.to_string()))?;
    match resolved.proxy {
        Some(proxy) => EgressConnector::from_proxy(proxy)
            .map_err(|error| ProxyError::Egress(error.to_string())),
        None => Ok(EgressConnector::direct()),
    }
}

#[cfg(test)]
mod tests {
    use super::agent_cli;

    #[test]
    fn identifies_oh_my_pi_from_cursor_cli_headers() {
        assert_eq!(agent_cli(None, false, true), "oh_my_pi",);
    }
}

async fn read_headers(stream: &mut TcpStream, limit: usize) -> Result<String, ProxyError> {
    let mut bytes = Vec::new();
    while bytes.len() < limit {
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).await?;
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            return String::from_utf8(bytes)
                .map_err(|_| ProxyError::Request("request headers are not UTF-8".to_owned()));
        }
    }
    Err(ProxyError::Request(
        "request headers exceeded limit".to_owned(),
    ))
}

fn parse_request_line(request: &str) -> Result<(&str, &str), ProxyError> {
    let line = request
        .split("\r\n")
        .next()
        .ok_or_else(|| ProxyError::Request("missing request line".to_owned()))?;
    let mut parts = line.split_ascii_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| ProxyError::Request("missing request method".to_owned()))?;
    let target = parts
        .next()
        .ok_or_else(|| ProxyError::Request("missing request target".to_owned()))?;
    let version = parts
        .next()
        .ok_or_else(|| ProxyError::Request("missing HTTP version".to_owned()))?;
    if parts.next().is_some() || !version.starts_with("HTTP/") {
        return Err(ProxyError::Request("invalid request line".to_owned()));
    }
    Ok((method, target))
}
