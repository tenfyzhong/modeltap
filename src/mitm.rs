use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MitmError {
    #[error("certificate I/O failed: {0}")]
    Io(String),
    #[error("certificate generation failed: {0}")]
    Certificate(#[from] rcgen::Error),
    #[error("invalid leaf host {0}")]
    Host(String),
    #[error("rustls server configuration failed: {0}")]
    Rustls(#[from] rustls::Error),
}

pub struct MitmAuthority {
    issuer: Issuer<'static, KeyPair>,
    root_certificate: CertificateDer<'static>,
}

impl MitmAuthority {
    pub fn generate(common_name: &str) -> Result<Self, MitmError> {
        let mut params = CertificateParams::new(Vec::<String>::new())?;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params
            .distinguished_name
            .push(DnType::CommonName, common_name);
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
        ];
        let key = KeyPair::generate()?;
        let certificate = params.self_signed(&key)?;
        Ok(Self {
            issuer: Issuer::new(params, key),
            root_certificate: CertificateDer::from(certificate.der().to_vec()),
        })
    }

    pub fn from_pem(certificate_pem: &str, key_pem: &str) -> Result<Self, MitmError> {
        let key = KeyPair::from_pem(key_pem)?;
        let issuer = Issuer::from_ca_cert_pem(certificate_pem, key)?;
        let certificate = rustls_pemfile::certs(&mut certificate_pem.as_bytes())
            .next()
            .ok_or_else(|| MitmError::Certificate(rcgen::Error::CouldNotParseCertificate))?
            .map_err(|_| MitmError::Certificate(rcgen::Error::CouldNotParseCertificate))?;
        Ok(Self {
            issuer,
            root_certificate: certificate,
        })
    }

    pub fn from_pem_files(certificate_file: &str, key_file: &str) -> Result<Self, MitmError> {
        let certificate_pem = std::fs::read_to_string(certificate_file)
            .map_err(|error| MitmError::Io(error.to_string()))?;
        let key_pem =
            std::fs::read_to_string(key_file).map_err(|error| MitmError::Io(error.to_string()))?;
        Self::from_pem(&certificate_pem, &key_pem)
    }

    pub fn root_certificate(&self) -> CertificateDer<'static> {
        self.root_certificate.clone()
    }

    pub fn root_certificate_pem(&self) -> Result<String, MitmError> {
        Ok(pem_encode("CERTIFICATE", self.root_certificate.as_ref()))
    }

    pub fn root_private_key_pem(&self) -> Result<String, MitmError> {
        Ok(self.issuer.key().serialize_pem())
    }

    pub fn server_config_for(&self, host: &str) -> Result<ServerConfig, MitmError> {
        if host.is_empty() || host.contains('/') || host.contains('\0') {
            return Err(MitmError::Host(host.to_owned()));
        }
        let mut params = CertificateParams::new(vec![host.to_owned()])?;
        params.distinguished_name.push(DnType::CommonName, host);
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let key = KeyPair::generate()?;
        let certificate = params.signed_by(&key, &self.issuer)?;
        let certificate = CertificateDer::from(certificate.der().to_vec());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let mut config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate], key)?;
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(config)
    }
}

fn pem_encode(label: &str, der: &[u8]) -> String {
    use base64::Engine;

    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let mut output = format!("-----BEGIN {label}-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        output.push_str(std::str::from_utf8(chunk).expect("base64 output is UTF-8"));
        output.push('\n');
    }
    output.push_str(&format!("-----END {label}-----\n"));
    output
}
