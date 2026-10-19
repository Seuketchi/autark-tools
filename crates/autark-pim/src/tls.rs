//! TLS for mail servers: rustls with the bundled Mozilla roots, over a TCP connection with timeouts.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

pub type Stream = StreamOwned<ClientConnection, TcpStream>;

const TIMEOUT: Duration = Duration::from_secs(30);

fn config() -> Result<Arc<ClientConfig>, String> {
    let roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| format!("TLS: {err}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// A plain connection to `host:port`, with timeouts.
pub fn tcp(host: &str, port: u16) -> Result<TcpStream, String> {
    let stream = TcpStream::connect((host, port)).map_err(|err| format!("{host}:{port}: {err}"))?;
    stream.set_read_timeout(Some(TIMEOUT)).and_then(|()| stream.set_write_timeout(Some(TIMEOUT))).map_err(|err| format!("{host}:{port}: {err}"))?;
    Ok(stream)
}

/// Starts TLS on `stream`, checking the server's certificate against `host`.
pub fn wrap(host: &str, stream: TcpStream) -> Result<Stream, String> {
    let name = ServerName::try_from(host.to_owned()).map_err(|err| format!("{host}: {err}"))?;
    let connection = ClientConnection::new(config()?, name).map_err(|err| format!("{host}: {err}"))?;
    Ok(StreamOwned::new(connection, stream))
}

/// A TLS connection from the start, as IMAP on 993 and SMTP on 465 use.
pub fn connect(host: &str, port: u16) -> Result<Stream, String> {
    wrap(host, tcp(host, port)?)
}

/// Anything that can be read and written, which is what IMAP and SMTP sessions need.
pub trait Io: Read + Write {}
impl<T: Read + Write> Io for T {}
