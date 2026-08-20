use modeltap::mitm::MitmAuthority;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

#[tokio::test]
async fn generated_leaf_certificate_is_trusted_by_the_generated_root() {
    let authority = MitmAuthority::generate("modeltap test root").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_config = authority.server_config_for("api.openai.com").unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = TlsAcceptor::from(Arc::new(server_config))
            .accept(stream)
            .await
            .unwrap();
        let mut request = [0_u8; 4];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"ping");
        stream.write_all(b"pong").await.unwrap();
    });

    let mut roots = RootCertStore::empty();
    roots.add(authority.root_certificate()).unwrap();
    let client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut client = TlsConnector::from(Arc::new(client_config))
        .connect(
            ServerName::try_from("api.openai.com").unwrap(),
            TcpStream::connect(address).await.unwrap(),
        )
        .await
        .unwrap();
    client.write_all(b"ping").await.unwrap();
    let mut response = [0_u8; 4];
    client.read_exact(&mut response).await.unwrap();
    assert_eq!(&response, b"pong");
    server.await.unwrap();
}

#[test]
fn loaded_root_can_sign_a_leaf_certificate() {
    let generated = MitmAuthority::generate("modeltap test root").unwrap();
    let key = generated.root_private_key_pem().unwrap();
    let loaded = MitmAuthority::from_pem(&generated.root_certificate_pem().unwrap(), &key).unwrap();
    assert!(loaded.server_config_for("api.openai.com").is_ok());
}
