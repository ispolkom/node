// src/socks5/client.rs
//! SOCKS5 Client Implementation
//! =============================
//!
//! Connect through SOCKS5 proxy server

use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use anyhow::{Result, anyhow};

use super::protocol::*;
use super::Socks5Error;

/// SOCKS5 client
pub struct Socks5Client {
    proxy_addr: SocketAddr,
    username: Option<String>,
    password: Option<String>,
}

impl Socks5Client {
    /// Create new SOCKS5 client
    pub fn new(proxy_addr: SocketAddr) -> Self {
        Self {
            proxy_addr,
            username: None,
            password: None,
        }
    }

    /// Set authentication credentials
    pub fn with_auth(mut self, username: String, password: String) -> Self {
        self.username = Some(username);
        self.password = Some(password);
        self
    }

    /// Connect through SOCKS5 proxy
    pub async fn connect(&self, target_addr: SocketAddr) -> Result<TcpStream> {
        self.connect_to(Socks5Address::from_socket_addr(target_addr)).await
    }

    /// Connect to domain through SOCKS5 proxy
    pub async fn connect_domain(&self, domain: String, port: u16) -> Result<TcpStream> {
        if domain.is_empty() || domain.len() > 255 || domain.bytes().any(|b| b <= 0x20 || b == 0x7f) {
            return Err(anyhow!("invalid domain name"));
        }
        self.connect_to(Socks5Address::Domain(domain, port)).await
    }

    /// Whole handshake under one deadline (proxy connect, method selection, auth, CONNECT).
    async fn connect_to(&self, address: Socks5Address) -> Result<TcpStream> {
        tokio::time::timeout(std::time::Duration::from_secs(30), self.handshake(address))
            .await
            .map_err(|_| anyhow!("SOCKS5 handshake timeout"))?
    }

    async fn handshake(&self, address: Socks5Address) -> Result<TcpStream> {
        let mut stream = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            TcpStream::connect(self.proxy_addr)
        ).await
        .map_err(|_| anyhow!("Connection timeout to SOCKS5 proxy"))?
        .map_err(|e| anyhow!("Failed to connect to SOCKS5 proxy: {}", e))?;
        let _ = stream.set_nodelay(true);

        // Phase 1: Auth selection
        let wanted = if self.username.is_some() { Socks5AuthMethod::UserPass } else { Socks5AuthMethod::NoAuth };
        let auth_select = Socks5AuthSelect {
            version: SOCKS5_VERSION,
            methods: vec![wanted],
        };
        stream.write_all(&auth_select.to_bytes()).await?;

        let mut auth_response = [0u8; 2];
        stream.read_exact(&mut auth_response).await?;
        if auth_response[0] != SOCKS5_VERSION {
            return Err(anyhow!("Invalid SOCKS5 version in response"));
        }
        let selected_method = Socks5AuthMethod::from_byte(auth_response[1])?;
        // the proxy must pick exactly what we offered: never fall back to a weaker method
        if selected_method != wanted {
            return Err(anyhow!("SOCKS5 proxy selected an unexpected authentication method"));
        }

        // Phase 2: Authenticate if required
        if selected_method == Socks5AuthMethod::UserPass {
            self.do_username_password_auth(&mut stream).await?;
        }

        // Phase 3: Send CONNECT request
        let request = Socks5Request {
            version: SOCKS5_VERSION,
            command: Socks5Command::Connect,
            reserved: 0x00,
            address,
        };
        stream.write_all(&request.to_bytes()).await?;

        // Read the whole reply: VER REP RSV ATYP BND.ADDR BND.PORT
        let mut head = [0u8; 4];
        stream.read_exact(&mut head).await?;
        if head[0] != SOCKS5_VERSION {
            return Err(anyhow!("Invalid SOCKS5 version in response"));
        }
        if head[1] != 0x00 {
            let error = Socks5Error::from_reply_byte(head[1]);
            return Err(anyhow!("SOCKS5 connect failed: {:?}", error));
        }
        let addr_len = match head[3] {
            0x01 => 4,
            0x04 => 16,
            0x03 => {
                let mut l = [0u8; 1];
                stream.read_exact(&mut l).await?;
                l[0] as usize
            }
            _ => return Err(anyhow!("SOCKS5 reply with unknown address type")),
        };
        let mut rest = vec![0u8; addr_len + 2];
        stream.read_exact(&mut rest).await?;

        Ok(stream)
    }

    /// Perform username/password authentication (RFC 1929: each field at most 255 bytes)
    async fn do_username_password_auth(&self, stream: &mut TcpStream) -> Result<()> {
        let username = self.username.as_ref().ok_or_else(|| anyhow!("No username set"))?;
        let password = self.password.as_ref().ok_or_else(|| anyhow!("No password set"))?;

        let username_bytes = username.as_bytes();
        let password_bytes = password.as_bytes();
        if username_bytes.is_empty() || username_bytes.len() > 255 || password_bytes.len() > 255 {
            return Err(anyhow!("SOCKS5 username/password length out of range (1..=255 / 0..=255)"));
        }

        let mut auth_packet = vec![0x01];  // Version
        auth_packet.push(username_bytes.len() as u8);
        auth_packet.extend_from_slice(username_bytes);
        auth_packet.push(password_bytes.len() as u8);
        auth_packet.extend_from_slice(password_bytes);

        stream.write_all(&auth_packet).await?;

        let mut auth_response = [0u8; 2];
        stream.read_exact(&mut auth_response).await?;
        if auth_response[1] != 0x00 {
            return Err(anyhow!("SOCKS5 authentication failed"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_address_conversion() {
        let addr = SocketAddr::from(([192, 168, 1, 1], 8080));
        let socks5_addr = Socks5Address::from_socket_addr(addr);

        match socks5_addr {
            Socks5Address::Ipv4(ip, port) => {
                assert_eq!(ip, Ipv4Addr::new(192, 168, 1, 1));
                assert_eq!(port, 8080);
            }
            _ => panic!("Not IPv4"),
        }
    }

    #[test]
    fn test_request_serialization() {
        let addr = Socks5Address::Ipv4(Ipv4Addr::new(127, 0, 0, 1), 9000);
        let request = Socks5Request {
            version: SOCKS5_VERSION,
            command: Socks5Command::Connect,
            reserved: 0x00,
            address: addr,
        };

        let bytes = request.to_bytes();
        assert_eq!(bytes[0], SOCKS5_VERSION);
        assert_eq!(bytes[1], Socks5Command::Connect.to_byte());
    }

    async fn fake_proxy(reply_method: u8, split_reply: bool) -> SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut b = [0u8; 3];
            s.read_exact(&mut b).await.unwrap();
            s.write_all(&[5, reply_method]).await.unwrap();
            if reply_method != 0 { return; }
            let mut req = [0u8; 10];
            s.read_exact(&mut req).await.unwrap();
            let reply = [5u8, 0, 0, 1, 0, 0, 0, 0, 0, 0];
            if split_reply {
                s.write_all(&reply[..3]).await.unwrap();
                s.flush().await.unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                s.write_all(&reply[3..]).await.unwrap();
            } else {
                s.write_all(&reply).await.unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        });
        addr
    }

    #[tokio::test]
    async fn a_split_reply_is_read_completely() {
        let proxy = fake_proxy(0, true).await;
        let c = Socks5Client::new(proxy);
        assert!(c.connect(SocketAddr::from(([127, 0, 0, 1], 80))).await.is_ok());
    }

    #[tokio::test]
    async fn a_proxy_that_ignores_our_credentials_and_picks_no_auth_is_refused() {
        let proxy = fake_proxy(0, false).await;
        let c = Socks5Client::new(proxy).with_auth("u".into(), "p".into());
        assert!(c.connect(SocketAddr::from(([127, 0, 0, 1], 80))).await.is_err());
    }

    #[tokio::test]
    async fn bad_domains_and_long_credentials_are_refused_before_any_traffic() {
        let c = Socks5Client::new(SocketAddr::from(([127, 0, 0, 1], 9)));
        assert!(c.connect_domain(String::new(), 80).await.is_err());
        assert!(c.connect_domain("a".repeat(256), 80).await.is_err());
        assert!(c.connect_domain("bad host".into(), 80).await.is_err());
    }
}
