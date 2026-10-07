// src/netlayer/tls_cert.rs
//! TLS certificate management для anchor-узлов (Iter 2 — WS-over-TLS).
//!
//! При первом старте генерим self-signed cert + key и кладём в `~/.yandi/tls/`.
//! Mobile-клиент должен принимать сертификат по pinning fingerprint
//! (хранится при pairing'е, см. Iter 4), а не по chain-of-trust.
//!
//! Если пользователь хочет настоящий Let's Encrypt — кладёт свои файлы и указывает
//! путь в config; модуль их использует без перегенерации.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

/// Однократная инициализация process-wide CryptoProvider для rustls (ring).
/// Безопасно вызывается из любого места — повторные вызовы no-op.
fn ensure_crypto_provider() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Загружено готовое к использованию TLS-удостоверение.
pub struct TlsIdentity {
    pub cert_chain: Vec<CertificateDer<'static>>,
    pub private_key: PrivateKeyDer<'static>,
    pub cert_pem_path: PathBuf,
    pub key_pem_path: PathBuf,
    /// SHA-256 fingerprint первого сертификата (hex, без двоеточий).
    /// Mobile pin'ит его при pairing.
    pub fingerprint_hex: String,
}

impl TlsIdentity {
    /// Подгрузить из стандартного места `~/.yandi/tls/{cert.pem,key.pem}`.
    /// Если файлов нет — сгенерировать self-signed и сохранить.
    pub fn load_or_generate_default(node_id_hex: &str) -> Result<Self> {
        let dir = default_tls_dir()?;
        Self::load_or_generate_in(&dir, node_id_hex)
    }

    /// Подгрузить или создать в указанной директории.
    pub fn load_or_generate_in(dir: &Path, node_id_hex: &str) -> Result<Self> {
        fs::create_dir_all(dir).with_context(|| format!("create_dir_all {:?}", dir))?;
        let cert_pem = dir.join("cert.pem");
        let key_pem = dir.join("key.pem");

        if cert_pem.exists() && key_pem.exists() {
            return load_pems(&cert_pem, &key_pem);
        }

        // Only one thread creates the pair: simultaneous first connections must not each write their own certificate and key
        // (a certificate of one run with the key of another would break every later start).
        static CREATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = CREATE.lock().unwrap_or_else(|e| e.into_inner());
        if cert_pem.exists() && key_pem.exists() {
            return load_pems(&cert_pem, &key_pem);
        }

        // Генерим новый self-signed cert.
        let (cert_pem_str, key_pem_str) = generate_self_signed(node_id_hex)?;
        fs::write(&cert_pem, &cert_pem_str)
            .with_context(|| format!("write {:?}", cert_pem))?;
        crate::util::private_file::write_private(&key_pem, key_pem_str.as_bytes())
            .with_context(|| format!("write {:?}", key_pem))?;
        load_pems(&cert_pem, &key_pem)
    }
}

fn default_tls_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("home_dir not available")?;
    Ok(home.join(".yandi").join("tls"))
}

/// Генерирует self-signed TLS cert + EC P-256 key. Возвращает (cert_pem, key_pem).
/// CN = `yandi-<node_id_hex_first8>`. SAN: `localhost`, `127.0.0.1`.
pub fn generate_self_signed(node_id_hex: &str) -> Result<(String, String)> {
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    let cn = format!("yandi-{}", &node_id_hex.chars().take(8).collect::<String>());
    dn.push(DnType::CommonName, cn);
    params.distinguished_name = dn;
    params.subject_alt_names = vec![
        SanType::DnsName("localhost".try_into()?),
        SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
    ];
    params.not_before = time::OffsetDateTime::now_utc() - time::Duration::days(1);
    params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(365 * 5);

    let key_pair = KeyPair::generate().context("rcgen KeyPair::generate")?;
    let cert = params.self_signed(&key_pair).context("rcgen self_signed")?;
    Ok((cert.pem(), key_pair.serialize_pem()))
}

fn load_pems(cert_path: &Path, key_path: &Path) -> Result<TlsIdentity> {
    let cert_data = fs::read(cert_path)
        .with_context(|| format!("read {:?}", cert_path))?;
    let key_data = fs::read(key_path)
        .with_context(|| format!("read {:?}", key_path))?;

    let mut cert_reader = std::io::Cursor::new(&cert_data);
    let cert_chain: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<std::io::Result<Vec<_>>>()
        .context("parse cert.pem")?;
    if cert_chain.is_empty() {
        anyhow::bail!("no certificates in {:?}", cert_path);
    }

    let mut key_reader = std::io::Cursor::new(&key_data);
    let private_key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key_reader)
        .context("parse key.pem")?
        .ok_or_else(|| anyhow::anyhow!("no private key in {:?}", key_path))?;

    // SHA-256 fingerprint первого сертификата.
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(cert_chain[0].as_ref());
    let fp = hasher.finalize();
    let fingerprint_hex = hex::encode(fp);

    Ok(TlsIdentity {
        cert_chain,
        private_key,
        cert_pem_path: cert_path.to_path_buf(),
        key_pem_path: key_path.to_path_buf(),
        fingerprint_hex,
    })
}

/// Построить `rustls::ServerConfig` из identity (для Anchor-узла).
pub fn build_server_config(identity: &TlsIdentity) -> Result<Arc<rustls::ServerConfig>> {
    ensure_crypto_provider();
    let cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(identity.cert_chain.clone(), identity.private_key.clone_key())
        .context("rustls ServerConfig with_single_cert")?;
    Ok(Arc::new(cfg))
}

/// Построить `rustls::ClientConfig` который доверяет ИСКЛЮЧИТЕЛЬНО peer'ам с
/// конкретным SHA-256 fingerprint'ом сертификата (pinning). Используется на
/// Mobile при подключении к paired anchor'у.
pub fn build_client_config_pinned(expected_fingerprint_hex: &str) -> Result<Arc<rustls::ClientConfig>> {
    ensure_crypto_provider();
    let verifier = Arc::new(PinnedFingerprintVerifier {
        expected: expected_fingerprint_hex.to_lowercase(),
    });
    let cfg = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

/// Клиент TLS **без проверки сертификата**: для запасного пути по TCP (`tcp_carrier`), где TLS — маскировка под HTTPS, а
/// подлинность собеседника и тайна содержимого обеспечиваются уровнем звена (подписанные приветствия, шифрование сеанса).
/// Для всего остального использовать нельзя.
pub fn build_client_config_insecure() -> Result<Arc<rustls::ClientConfig>> {
    ensure_crypto_provider();
    let cfg = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCert))
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

#[derive(Debug)]
struct AcceptAnyCert;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(&self, _m: &[u8], _c: &CertificateDer<'_>, _d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(&self, _m: &[u8], _c: &CertificateDer<'_>, _d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
        ]
    }
}

/// Verifier который принимает любой cert если его SHA-256 fingerprint совпадает
/// с ожидаемым (заданным при pairing).
#[derive(Debug)]
struct PinnedFingerprintVerifier {
    expected: String,
}

impl rustls::client::danger::ServerCertVerifier for PinnedFingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(end_entity.as_ref());
        let fp = hex::encode(hasher.finalize());
        if fp == self.expected {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "TLS pin mismatch: expected {}, got {}",
                self.expected, fp
            )))
        }
    }

    // The pin only says "this is the certificate we expect"; the handshake signature proves that the server holds the matching
    // PRIVATE key. Without checking it, anyone could replay the (public) certificate and complete the handshake with their own key.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &rustls::crypto::ring::default_provider().signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &rustls::crypto::ring::default_provider().signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn many_threads_asking_at_once_get_the_same_certificate_and_a_matching_key() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let p = path.clone();
                std::thread::spawn(move || TlsIdentity::load_or_generate_in(&p, "racecafe00").map(|i| i.fingerprint_hex))
            })
            .collect();
        let prints: Vec<String> = handles.into_iter().map(|h| h.join().unwrap().expect("every caller gets a usable identity")).collect();
        assert!(prints.windows(2).all(|w| w[0] == w[1]), "all callers see one certificate");
        // and the stored pair works as a server configuration
        let id = TlsIdentity::load_or_generate_in(&path, "racecafe00").unwrap();
        assert!(build_server_config(&id).is_ok());
    }

    /// A pinned certificate is public: someone who replays it must still prove they hold its private key.
    /// The rogue server below presents the pinned certificate but signs the handshake with a different key.
    #[tokio::test]
    async fn a_server_that_replays_the_pinned_certificate_without_its_key_is_refused() {
        use rustls::server::{ClientHello, ResolvesServerCert};
        use rustls::sign::CertifiedKey;
        ensure_crypto_provider();
        let (da, db) = (tempdir().unwrap(), tempdir().unwrap());
        let real = TlsIdentity::load_or_generate_in(da.path(), "aaaa000000").unwrap();
        let rogue_key = TlsIdentity::load_or_generate_in(db.path(), "bbbb000000").unwrap();
        let signing = rustls::crypto::ring::sign::any_supported_type(&rogue_key.private_key.clone_key()).unwrap();

        #[derive(Debug)]
        struct Replay(Arc<CertifiedKey>);
        impl ResolvesServerCert for Replay {
            fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
                Some(self.0.clone())
            }
        }
        let certified = Arc::new(CertifiedKey::new(real.cert_chain.clone(), signing));
        let cfg = rustls::ServerConfig::builder().with_no_client_auth().with_cert_resolver(Arc::new(Replay(certified)));
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::task::spawn(async move {
            if let Ok((tcp, _)) = listener.accept().await {
                let _ = acceptor.accept(tcp).await;
            }
        });

        let client = tokio_rustls::TlsConnector::from(build_client_config_pinned(&real.fingerprint_hex).unwrap());
        let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        assert!(client.connect(name, tcp).await.is_err(), "the replayed certificate must not be enough");
    }


    #[test]
    fn generate_then_load_roundtrip() {
        let dir = tempdir().unwrap();
        let id1 = TlsIdentity::load_or_generate_in(dir.path(), "0123456789abcdef").unwrap();
        // Загрузим повторно — fingerprint должен совпасть, файлы те же.
        let id2 = TlsIdentity::load_or_generate_in(dir.path(), "0123456789abcdef").unwrap();
        assert_eq!(id1.fingerprint_hex, id2.fingerprint_hex);
        assert!(!id1.cert_chain.is_empty());
        assert_eq!(id1.fingerprint_hex.len(), 64);
    }

    #[test]
    fn server_config_builds() {
        let dir = tempdir().unwrap();
        let id = TlsIdentity::load_or_generate_in(dir.path(), "deadbeef00").unwrap();
        let cfg = build_server_config(&id).unwrap();
        assert!(Arc::strong_count(&cfg) >= 1);
    }

    #[test]
    fn pinned_client_config_builds() {
        let dir = tempdir().unwrap();
        let id = TlsIdentity::load_or_generate_in(dir.path(), "cafebabe00").unwrap();
        let cfg = build_client_config_pinned(&id.fingerprint_hex).unwrap();
        assert!(Arc::strong_count(&cfg) >= 1);
    }
}
