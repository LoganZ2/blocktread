//! QUIC plumbing shared by client and server: endpoint setup and length-prefixed stream frames.
//!
//! Servers prove who they are with a self-signed certificate. A public server keeps one on disk
//! (`Identity::load_or_create`) and clients that know its fingerprint (`Target::pin`) accept that
//! certificate and no other, so nobody can stand in for it. Without a pin (a server on this
//! machine, one a player added by hand) any certificate goes, which is fine for play but not for
//! passwords.

use anyhow::{Context, Result, bail};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, Endpoint, RecvStream, SendStream, ServerConfig, TransportConfig, VarInt};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, ring};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// The handshake only goes through when both sides send the same one; an outdated client still
/// gets that far, so it can be told to update.
pub const ALPN: &[u8] = b"blocktread/1";
/// Close code of a connection made only to measure the round trip (the client's server picker);
/// the server lets it go without logging a failed join.
pub const PING_CLOSE: u32 = 0x5049_4e47;
/// Largest reliable frame we accept (a full design is ~1 KB, a scoreboard a few KB).
pub const MAX_FRAME: usize = 256 * 1024;

fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

/// SHA-256 of a server's certificate.
pub type Pin = [u8; 32];

/// What a server's certificate is known by: the hex of its SHA-256.
pub fn fingerprint(cert: &[u8]) -> String {
    ::ring::digest::digest(&::ring::digest::SHA256, cert).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// A fingerprint as written (64 hex digits, `:` between pairs allowed).
pub fn parse_pin(text: &str) -> Option<Pin> {
    let hex: String = text.chars().filter(|c| *c != ':' && !c.is_whitespace()).collect();
    if hex.len() != 64 {
        return None;
    }
    let mut pin = [0u8; 32];
    for (i, b) in pin.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(pin)
}

/// A server to connect to: its address, and the certificate it must show (None: any).
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub addr: String,
    pub pin: Option<Pin>,
}

impl From<&str> for Target {
    fn from(addr: &str) -> Self {
        Target { addr: addr.to_string(), pin: None }
    }
}

impl From<&String> for Target {
    fn from(addr: &String) -> Self {
        Target { addr: addr.clone(), pin: None }
    }
}

impl From<String> for Target {
    fn from(addr: String) -> Self {
        Target { addr, pin: None }
    }
}

/// A server's certificate and its key.
pub struct Identity {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

impl Identity {
    /// A new one, kept nowhere (local play, tests).
    pub fn generate() -> Result<Identity> {
        let ck = rcgen::generate_simple_self_signed(vec!["blocktread".to_string(), "localhost".to_string()])?;
        Ok(Identity { cert: ck.cert.der().clone(), key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der())) })
    }

    /// The one kept in `dir` (`cert.der`, `key.der`), made and saved there the first time, so
    /// the fingerprint clients pin stays the same across restarts and releases.
    pub fn load_or_create(dir: &Path) -> Result<Identity> {
        let (cert_path, key_path) = (dir.join("cert.der"), dir.join("key.der"));
        if cert_path.exists() || key_path.exists() {
            let cert = std::fs::read(&cert_path).with_context(|| format!("读不了 {}", cert_path.display()))?;
            let key = std::fs::read(&key_path).with_context(|| format!("读不了 {}", key_path.display()))?;
            return Ok(Identity { cert: CertificateDer::from(cert), key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)) });
        }
        let id = Identity::generate()?;
        std::fs::create_dir_all(dir)?;
        std::fs::write(&cert_path, id.cert.as_ref())?;
        write_private(&key_path, id.key.secret_der())?;
        Ok(id)
    }

    pub fn fingerprint(&self) -> String {
        fingerprint(self.cert.as_ref())
    }
}

/// A file only its owner can read.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
        f.write_all(bytes)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

fn transport() -> Arc<TransportConfig> {
    let mut t = TransportConfig::default();
    t.max_idle_timeout(Some(VarInt::from_u32(10_000).into()));
    t.keep_alive_interval(Some(Duration::from_secs(2)));
    t.datagram_receive_buffer_size(Some(1 << 20));
    t.datagram_send_buffer_size(1 << 20);
    Arc::new(t)
}

pub fn server_endpoint(addr: SocketAddr, id: Identity) -> Result<Endpoint> {
    let Identity { cert, key } = id;
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut cfg = ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls)?));
    cfg.transport_config(transport());
    Endpoint::server(cfg, addr).with_context(|| format!("无法监听 {addr}（端口可能被占用）"))
}

/// An endpoint for talking to `server`, which must show the certificate `pin` names (any, if None).
pub fn client_endpoint(server: SocketAddr, pin: Option<Pin>) -> Result<Endpoint> {
    let bind: SocketAddr = if server.is_ipv6() { "[::]:0".parse()? } else { "0.0.0.0:0".parse()? };
    let mut tls = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(ServerCert { provider: provider(), pin }))
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut cfg = ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls)?));
    cfg.transport_config(transport());
    let mut ep = Endpoint::client(bind)?;
    ep.set_default_client_config(cfg);
    Ok(ep)
}

pub async fn write_frame(s: &mut SendStream, bytes: &[u8]) -> Result<()> {
    s.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    s.write_all(bytes).await?;
    Ok(())
}

/// Next frame, or None when the peer finished the stream cleanly.
pub async fn read_frame(r: &mut RecvStream) -> Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let n = u32::from_le_bytes(len) as usize;
    if n > MAX_FRAME {
        bail!("frame too large: {n}");
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

/// Takes the certificate whose fingerprint is pinned, or with no pin any certificate. The
/// handshake signatures are checked either way, so the server holds the certificate's key.
#[derive(Debug)]
struct ServerCert {
    provider: Arc<CryptoProvider>,
    pin: Option<Pin>,
}

impl ServerCertVerifier for ServerCert {
    fn verify_server_cert(&self, cert: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        match self.pin {
            Some(pin) if ::ring::digest::digest(&::ring::digest::SHA256, cert.as_ref()).as_ref() != pin => Err(rustls::Error::General("服务器证书和内置的指纹对不上".into())),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }
    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}
