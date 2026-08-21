use crate::config::{EgressProtocol, EgressProxyConfig, ProxyAuthConfig};
use base64::Engine;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use std::io::{self, BufReader};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

pub trait AsyncIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T> AsyncIo for T where T: AsyncRead + AsyncWrite + Unpin + Send {}
pub type BoxStream = Box<dyn AsyncIo>;

pub enum UpstreamStream {
    Plain(BoxStream),
    Tls(Box<tokio_rustls::client::TlsStream<BoxStream>>),
}

impl UpstreamStream {
    pub(crate) fn negotiated_h2(&self) -> bool {
        match self {
            Self::Plain(_) => false,
            Self::Tls(stream) => stream
                .get_ref()
                .1
                .alpn_protocol()
                .is_some_and(|protocol| protocol == b"h2"),
        }
    }
}

impl AsyncRead for UpstreamStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_read(cx, buffer),
            Self::Tls(stream) => Pin::new(stream).poll_read(cx, buffer),
        }
    }
}

impl AsyncWrite for UpstreamStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_write(cx, buffer),
            Self::Tls(stream) => Pin::new(stream).poll_write(cx, buffer),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Self::Tls(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Tls(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

impl hyper_util::client::legacy::connect::Connection for UpstreamStream {
    fn connected(&self) -> hyper_util::client::legacy::connect::Connected {
        let connected = hyper_util::client::legacy::connect::Connected::new();
        if self.negotiated_h2() {
            connected.negotiated_h2()
        } else {
            connected
        }
    }
}

#[derive(Debug, Error)]
pub enum EgressError {
    #[error("invalid egress proxy: {0}")]
    Config(String),
    #[error("missing proxy credential environment variable {0}")]
    MissingCredential(String),
    #[error("invalid target authority {0}")]
    Authority(String),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("TLS setup failed: {0}")]
    Tls(String),
    #[error("HTTP proxy rejected CONNECT: {0}")]
    HttpConnect(String),
    #[error("SOCKS5 proxy rejected request with code {0:#x}")]
    SocksReply(u8),
    #[error("SOCKS5 proxy selected unsupported authentication method {0:#x}")]
    SocksAuthentication(u8),
}

#[derive(Debug, Clone)]
struct Credentials {
    username: String,
    password: String,
}

impl Credentials {
    fn load(config: &ProxyAuthConfig) -> Result<Self, EgressError> {
        let username = std::env::var(&config.username_env)
            .map_err(|_| EgressError::MissingCredential(config.username_env.clone()))?;
        let password = std::env::var(&config.password_env)
            .map_err(|_| EgressError::MissingCredential(config.password_env.clone()))?;
        if username.len() > u8::MAX as usize || password.len() > u8::MAX as usize {
            return Err(EgressError::Config(
                "SOCKS5 credentials exceed 255 bytes".to_owned(),
            ));
        }
        Ok(Self { username, password })
    }
}

#[derive(Clone)]
pub enum EgressConnector {
    Direct,
    Proxy(ProxyConnector),
}

#[derive(Clone)]
pub struct ProxyConnector {
    protocol: EgressProtocol,
    address: String,
    credentials: Option<Credentials>,
    tls_server_name: Option<String>,
    tls_ca_file: Option<String>,
    target_tls_server_name: Option<String>,
    target_tls_ca_file: Option<String>,
}

impl EgressConnector {
    pub fn direct() -> Self {
        Self::Direct
    }

    pub fn from_proxy(proxy: &EgressProxyConfig) -> Result<Self, EgressError> {
        let url = proxy
            .parsed_url()
            .map_err(|error| EgressError::Config(error.to_string()))?;
        let protocol = proxy
            .protocol()
            .map_err(|error| EgressError::Config(error.to_string()))?;
        let host = url
            .host_str()
            .ok_or_else(|| EgressError::Config("proxy URL has no host".to_owned()))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| EgressError::Config("proxy URL has no port".to_owned()))?;
        let credentials = proxy.auth.as_ref().map(Credentials::load).transpose()?;
        let tls_server_name = proxy
            .tls
            .as_ref()
            .and_then(|tls| tls.server_name.clone())
            .or_else(|| matches!(protocol, EgressProtocol::Https).then(|| host.to_owned()));
        let tls_ca_file = proxy.tls.as_ref().and_then(|tls| tls.ca_file.clone());
        let target_tls_server_name = proxy
            .target_tls
            .as_ref()
            .and_then(|tls| tls.server_name.clone());
        let target_tls_ca_file = proxy
            .target_tls
            .as_ref()
            .and_then(|tls| tls.ca_file.clone());
        Ok(Self::Proxy(ProxyConnector {
            protocol,
            address: format!("{host}:{port}"),
            credentials,
            tls_server_name,
            tls_ca_file,
            target_tls_server_name,
            target_tls_ca_file,
        }))
    }

    pub async fn connect(&self, authority: &str) -> Result<BoxStream, EgressError> {
        let target = TargetAuthority::parse(authority)?;
        match self {
            Self::Direct => Ok(Box::new(TcpStream::connect(authority).await?)),
            Self::Proxy(proxy) => proxy.connect(target).await,
        }
    }

    pub async fn connect_uri(&self, uri: &http::Uri) -> Result<UpstreamStream, EgressError> {
        self.connect_uri_with_protocols(uri, true).await
    }

    pub async fn connect_uri_http1(&self, uri: &http::Uri) -> Result<UpstreamStream, EgressError> {
        self.connect_uri_with_protocols(uri, false).await
    }

    async fn connect_uri_with_protocols(
        &self,
        uri: &http::Uri,
        enable_http2: bool,
    ) -> Result<UpstreamStream, EgressError> {
        let authority = uri
            .authority()
            .ok_or_else(|| EgressError::Authority(uri.to_string()))?;
        let stream = self.connect(authority.as_str()).await?;
        if uri.scheme_str() != Some("https") {
            return Ok(UpstreamStream::Plain(stream));
        }
        let host = authority.host();
        let (server_name, ca_file) = match self {
            Self::Direct => (host, None),
            Self::Proxy(proxy) => (
                proxy.target_tls_server_name.as_deref().unwrap_or(host),
                proxy.target_tls_ca_file.as_deref(),
            ),
        };
        let server_name = ServerName::try_from(server_name.to_owned())
            .map_err(|_| EgressError::Tls(format!("invalid target server name {host}")))?;
        let config = client_config(ca_file, enable_http2)?;
        let stream = TlsConnector::from(Arc::new(config))
            .connect(server_name, stream)
            .await
            .map_err(|error| EgressError::Tls(error.to_string()))?;
        Ok(UpstreamStream::Tls(Box::new(stream)))
    }
}

impl ProxyConnector {
    async fn connect(&self, target: TargetAuthority) -> Result<BoxStream, EgressError> {
        let mut stream = self.open_stream().await?;
        match self.protocol {
            EgressProtocol::Http | EgressProtocol::Https => {
                let authorization = self.credentials.as_ref().map(http_proxy_authorization);
                let request = match authorization {
                    Some(authorization) => format!(
                        "CONNECT {} HTTP/1.1\r\nHost: {}\r\nProxy-Authorization: {authorization}\r\nConnection: keep-alive\r\n\r\n",
                        target.authority, target.authority
                    ),
                    None => format!(
                        "CONNECT {} HTTP/1.1\r\nHost: {}\r\nConnection: keep-alive\r\n\r\n",
                        target.authority, target.authority
                    ),
                };
                stream.write_all(request.as_bytes()).await?;
                stream.flush().await?;
                let response = read_headers(&mut stream, 32 * 1024).await?;
                let status_line = response.split("\r\n").next().unwrap_or_default();
                if !(status_line.starts_with("HTTP/1.1 200")
                    || status_line.starts_with("HTTP/1.0 200"))
                {
                    return Err(EgressError::HttpConnect(status_line.to_owned()));
                }
                Ok(stream)
            }
            EgressProtocol::Socks5 => {
                socks5_connect(&mut stream, &target, self.credentials.as_ref()).await?;
                Ok(stream)
            }
        }
    }

    async fn open_stream(&self) -> Result<BoxStream, EgressError> {
        let stream = TcpStream::connect(&self.address).await?;
        if !matches!(self.protocol, EgressProtocol::Https) {
            return Ok(Box::new(stream));
        }
        let server_name = self
            .tls_server_name
            .as_deref()
            .ok_or_else(|| EgressError::Config("HTTPS proxy has no server name".to_owned()))?;
        let config = client_config(self.tls_ca_file.as_deref(), false)?;
        let server_name = ServerName::try_from(server_name.to_owned())
            .map_err(|_| EgressError::Tls("invalid HTTPS proxy server name".to_owned()))?;
        let stream = TlsConnector::from(Arc::new(config))
            .connect(server_name, stream)
            .await
            .map_err(|error| EgressError::Tls(error.to_string()))?;
        Ok(Box::new(stream))
    }
}

fn client_config(ca_file: Option<&str>, enable_http2: bool) -> Result<ClientConfig, EgressError> {
    let mut roots = RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for certificate in native.certs {
        roots
            .add(certificate)
            .map_err(|error| EgressError::Tls(error.to_string()))?;
    }
    if let Some(path) = ca_file {
        let file = std::fs::File::open(path)?;
        let mut reader = BufReader::new(file);
        for certificate in rustls_pemfile::certs(&mut reader) {
            roots
                .add(certificate.map_err(|error| EgressError::Tls(error.to_string()))?)
                .map_err(|error| EgressError::Tls(error.to_string()))?;
        }
    }
    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    if enable_http2 {
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    }
    Ok(config)
}

fn http_proxy_authorization(credentials: &Credentials) -> String {
    let raw = format!("{}:{}", credentials.username, credentials.password);
    let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
    format!("Basic {encoded}")
}

async fn read_headers<S>(stream: &mut S, max_size: usize) -> Result<String, EgressError>
where
    S: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    while bytes.len() < max_size {
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).await?;
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            return String::from_utf8(bytes)
                .map_err(|_| EgressError::HttpConnect("non-UTF-8 response headers".to_owned()));
        }
    }
    Err(EgressError::HttpConnect(
        "response headers exceeded limit".to_owned(),
    ))
}

#[derive(Debug)]
struct TargetAuthority {
    authority: String,
    host: String,
    port: u16,
}

impl TargetAuthority {
    fn parse(authority: &str) -> Result<Self, EgressError> {
        let authority = http::uri::Authority::from_str(authority)
            .map_err(|_| EgressError::Authority(authority.to_owned()))?;
        let port = authority
            .port_u16()
            .ok_or_else(|| EgressError::Authority(authority.as_str().to_owned()))?;
        Ok(Self {
            authority: authority.as_str().to_owned(),
            host: authority.host().to_owned(),
            port,
        })
    }
}

async fn socks5_connect<S>(
    stream: &mut S,
    target: &TargetAuthority,
    credentials: Option<&Credentials>,
) -> Result<(), EgressError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let methods: &[u8] = if credentials.is_some() { &[0, 2] } else { &[0] };
    stream.write_all(&[5, methods.len() as u8]).await?;
    stream.write_all(methods).await?;
    stream.flush().await?;
    let mut selected = [0_u8; 2];
    stream.read_exact(&mut selected).await?;
    if selected[0] != 5 {
        return Err(EgressError::SocksAuthentication(selected[1]));
    }
    match selected[1] {
        0 => {}
        2 => {
            socks5_authenticate(
                stream,
                credentials.ok_or(EgressError::SocksAuthentication(2))?,
            )
            .await?
        }
        method => return Err(EgressError::SocksAuthentication(method)),
    }

    let host = target.host.as_bytes();
    if host.len() > u8::MAX as usize {
        return Err(EgressError::Authority(target.authority.clone()));
    }
    stream.write_all(&[5, 1, 0, 3, host.len() as u8]).await?;
    stream.write_all(host).await?;
    stream.write_all(&target.port.to_be_bytes()).await?;
    stream.flush().await?;

    let mut response = [0_u8; 4];
    stream.read_exact(&mut response).await?;
    if response[0] != 5 {
        return Err(EgressError::SocksReply(response[1]));
    }
    if response[1] != 0 {
        return Err(EgressError::SocksReply(response[1]));
    }
    let address_size = match response[3] {
        1 => 4,
        3 => {
            let mut length = [0_u8; 1];
            stream.read_exact(&mut length).await?;
            length[0] as usize
        }
        4 => 16,
        _ => return Err(EgressError::SocksReply(response[3])),
    };
    let mut bound = vec![0_u8; address_size + 2];
    stream.read_exact(&mut bound).await?;
    Ok(())
}

async fn socks5_authenticate<S>(
    stream: &mut S,
    credentials: &Credentials,
) -> Result<(), EgressError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    stream
        .write_all(&[1, credentials.username.len() as u8])
        .await?;
    stream.write_all(credentials.username.as_bytes()).await?;
    stream
        .write_all(&[credentials.password.len() as u8])
        .await?;
    stream.write_all(credentials.password.as_bytes()).await?;
    stream.flush().await?;
    let mut response = [0_u8; 2];
    stream.read_exact(&mut response).await?;
    if response != [1, 0] {
        return Err(EgressError::SocksAuthentication(response[1]));
    }
    Ok(())
}
