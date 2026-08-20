use modeltap::config::Config;
use modeltap::config::EgressProxyConfig;
use modeltap::egress::EgressConnector;
use modeltap::proxy::handle_connection;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::test]
async fn http_proxy_connects_to_the_requested_authority() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        assert!(
            String::from_utf8(request)
                .unwrap()
                .starts_with("CONNECT api.example:443 HTTP/1.1\r\n")
        );
        stream
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .unwrap();
        let mut payload = [0_u8; 4];
        stream.read_exact(&mut payload).await.unwrap();
        assert_eq!(&payload, b"ping");
        stream.write_all(b"pong").await.unwrap();
    });

    let proxy = EgressProxyConfig {
        id: "gost".to_owned(),
        url: format!("http://{address}"),
        auth: None,
        tls: None,
        target_tls: None,
    };
    let connector = EgressConnector::from_proxy(&proxy).unwrap();
    let mut tunnel = connector.connect("api.example:443").await.unwrap();
    tunnel.write_all(b"ping").await.unwrap();
    let mut response = [0_u8; 4];
    tunnel.read_exact(&mut response).await.unwrap();
    assert_eq!(&response, b"pong");
    server.await.unwrap();
}

#[tokio::test]
async fn socks5_proxy_uses_domain_name_and_username_password_authentication() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut greeting = [0_u8; 4];
        stream.read_exact(&mut greeting).await.unwrap();
        assert_eq!(greeting, [5, 2, 0, 2]);
        stream.write_all(&[5, 2]).await.unwrap();
        let mut auth = [0_u8; 10];
        stream.read_exact(&mut auth).await.unwrap();
        assert_eq!(&auth, b"\x01\x03bob\x04pass");
        stream.write_all(&[1, 0]).await.unwrap();
        let mut header = [0_u8; 5];
        stream.read_exact(&mut header).await.unwrap();
        assert_eq!(&header, &[5, 1, 0, 3, 11]);
        let mut host_and_port = [0_u8; 13];
        stream.read_exact(&mut host_and_port).await.unwrap();
        assert_eq!(&host_and_port, b"api.example\x01\xbb");
        stream
            .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
            .await
            .unwrap();
    });

    unsafe {
        std::env::set_var("TEST_GOST_USER", "bob");
        std::env::set_var("TEST_GOST_PASSWORD", "pass");
    }
    let proxy = EgressProxyConfig {
        id: "socks".to_owned(),
        url: format!("socks5://{address}"),
        auth: Some(modeltap::config::ProxyAuthConfig {
            username_env: "TEST_GOST_USER".to_owned(),
            password_env: "TEST_GOST_PASSWORD".to_owned(),
        }),
        tls: None,
        target_tls: None,
    };
    let connector = EgressConnector::from_proxy(&proxy).unwrap();
    connector.connect("api.example:443").await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn transparent_connect_is_forwarded_through_the_configured_egress_proxy() {
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    let upstream = tokio::spawn(async move {
        let (mut stream, _) = upstream_listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        assert!(
            String::from_utf8(request)
                .unwrap()
                .starts_with("CONNECT api.openai.com:443")
        );
        stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
        let mut payload = [0_u8; 4];
        stream.read_exact(&mut payload).await.unwrap();
        stream.write_all(&payload).await.unwrap();
    });

    let config = Arc::new(Config::from_yaml(&format!(
        "proxy: {{listen: 127.0.0.1:8080}}\negress:\n  default: gost\n  proxies:\n    - id: gost\n      url: http://{upstream_address}\nsites:\n  - id: openai\n    provider: openai\n    hosts: [api.openai.com]\n    mitm: false\npricing: {{timezone: Asia/Shanghai}}\n"
    )).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_address = listener.local_addr().unwrap();
    let proxy = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        handle_connection(stream, config).await.unwrap();
    });

    let mut client = TcpStream::connect(proxy_address).await.unwrap();
    client
        .write_all(b"CONNECT api.openai.com:443 HTTP/1.1\r\nHost: api.openai.com:443\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        client.read_exact(&mut byte).await.unwrap();
        response.push(byte[0]);
        if response.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    assert_eq!(response, b"HTTP/1.1 200 Connection Established\r\n\r\n");
    client.write_all(b"ping").await.unwrap();
    let mut echoed = [0_u8; 4];
    client.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"ping");
    drop(client);
    proxy.await.unwrap();
    upstream.await.unwrap();
}

#[tokio::test]
async fn unavailable_egress_fails_closed_before_connect_response() {
    let config = Arc::new(Config::from_yaml(
        "proxy: {listen: 127.0.0.1:8080}\negress:\n  default: unavailable\n  proxies:\n    - id: unavailable\n      url: http://127.0.0.1:1\nsites: []\npricing: {timezone: Asia/Shanghai}\n",
    ).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_address = listener.local_addr().unwrap();
    let proxy = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        handle_connection(stream, config).await.unwrap();
    });

    let mut client = TcpStream::connect(proxy_address).await.unwrap();
    client
        .write_all(
            b"CONNECT unavailable.example:443 HTTP/1.1\r\nHost: unavailable.example:443\r\n\r\n",
        )
        .await
        .unwrap();
    let mut response = [0_u8; 64];
    let size = client.read(&mut response).await.unwrap();
    assert!(
        std::str::from_utf8(&response[..size])
            .unwrap()
            .starts_with("HTTP/1.1 502")
    );
    proxy.await.unwrap();
}
