//! Shared native HTTPS client configuration; proxy settings never contain secrets.
use reqwest::{Client, Proxy, Url};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    #[default]
    Environment,
    Direct,
    Http,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProxySettings {
    pub mode: ProxyMode,
    pub server: String,
}
impl ProxySettings {
    pub fn validated(&self) -> Result<Self, String> {
        let server = self.server.trim();
        if server.is_empty() {
            if self.mode == ProxyMode::Http {
                return Err("Enter an HTTP proxy URL, for example http://127.0.0.1:7890.".into());
            }
            return Ok(Self {
                mode: self.mode,
                server: String::new(),
            });
        }
        if server.len() > 2048 || server.chars().any(char::is_whitespace) || server.contains('\\') {
            return Err("Enter a valid HTTP proxy URL.".into());
        }
        let url = Url::parse(server).map_err(|_| "Enter a valid HTTP proxy URL.".to_string())?;
        if url.scheme() != "http"
            || url.host_str().is_none()
            || url.port_or_known_default() == Some(0)
        {
            return Err("Use an http:// proxy server with a valid host and port.".into());
        }
        // URLs are persisted in preferences and exposed to Settings, so never accept userinfo.
        if !url.username().is_empty() || url.password().is_some() || server.contains('@') {
            return Err("Proxy URLs with usernames or passwords are not supported.".into());
        }
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err(
                "A proxy URL must contain only the scheme, host, and optional port.".into(),
            );
        }
        Ok(Self {
            mode: self.mode,
            server: url.to_string(),
        })
    }
}

pub fn build_client(proxy: &ProxySettings) -> Result<Client, String> {
    let proxy = proxy.validated()?;
    let mut builder = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .user_agent("Luma/0.1.0");
    match proxy.mode {
        ProxyMode::Environment => {} // Preserve reqwest's existing environment proxy behavior.
        ProxyMode::Direct => {
            builder = builder.no_proxy();
        }
        ProxyMode::Http => {
            let configured = Proxy::all(&proxy.server)
                .map_err(|_| "Unable to configure HTTP proxy.".to_string())?;
            builder = builder.no_proxy().proxy(configured);
        }
    }
    builder
        .build()
        .map_err(|_| "Unable to initialize the network connection.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_http_hosts_and_rejects_credential_or_nonproxy_urls() {
        for server in [
            "http://127.0.0.1:7890",
            "http://localhost:8080",
            "http://[::1]:3128",
            "http://proxy.example",
        ] {
            assert!(ProxySettings {
                mode: ProxyMode::Http,
                server: server.into()
            }
            .validated()
            .is_ok());
        }
        for server in [
            "",
            "localhost:7890",
            "https://localhost:7890",
            "socks5://localhost:1080",
            "http://user:secret@localhost:8080",
            "http://localhost:0",
            "http://localhost:65536",
            "http://localhost/path",
            "http://localhost?token=secret",
            "http://localhost/#secret",
            "http://local host:80",
        ] {
            let error = ProxySettings {
                mode: ProxyMode::Http,
                server: server.into(),
            }
            .validated()
            .unwrap_err();
            assert!(!error.contains("secret"));
        }
        assert!(ProxySettings::default().validated().is_ok());
        assert!(build_client(&ProxySettings {
            mode: ProxyMode::Direct,
            server: String::new()
        })
        .is_ok());
    }
    #[tokio::test]
    async fn https_uses_http_connect_without_exposing_origin_authorization() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let proxy = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "HTTPS request did not reach the configured proxy"
                        );
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("Test listener failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
                let mut byte = [0];
                assert_eq!(stream.read(&mut byte).unwrap(), 1);
                request.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            String::from_utf8(request).unwrap()
        });
        let client = build_client(&ProxySettings {
            mode: ProxyMode::Http,
            server: format!("http://{address}"),
        })
        .unwrap();
        assert!(client
            .get("https://api.github.com/user")
            .bearer_auth("test-origin-secret")
            .send()
            .await
            .is_err());
        let request = proxy.join().unwrap();
        assert!(request.starts_with("CONNECT api.github.com:443 HTTP/1.1"));
        assert!(!request.contains("test-origin-secret"));
        assert!(client
            .get("http://api.github.com/user")
            .send()
            .await
            .is_err());
    }
}
