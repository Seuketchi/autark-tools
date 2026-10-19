//! OAuth 2 for an app on your own computer (RFC 8252, 7636): the sign-in page opens in your browser, the
//! provider sends the browser back to a port on 127.0.0.1 with a code, and the code is traded for a refresh
//! token. PKCE keeps a stolen code useless. The client's secret, which Google issues to desktop apps, isn't
//! treated as secret by the provider either, but is kept in `accounts.toml` readable only by you.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::Client;

/// Where a provider's OAuth lives, and what to ask for.
pub struct Provider {
    pub auth_url: &'static str,
    pub token_url: &'static str,
    pub userinfo_url: &'static str,
    pub scopes: &'static [&'static str],
}

pub const GOOGLE: Provider = Provider {
    auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
    token_url: "https://oauth2.googleapis.com/token",
    userinfo_url: "https://openidconnect.googleapis.com/v1/userinfo",
    // Mail over IMAP and SMTP, calendars, contacts, and the address that signed in.
    scopes: &["https://mail.google.com/", "https://www.googleapis.com/auth/calendar", "https://www.googleapis.com/auth/carddav", "openid", "email"],
};

pub fn provider(name: &str) -> Result<&'static Provider, String> {
    match name {
        "google" => Ok(&GOOGLE),
        other => Err(format!("OAuth sign-in isn't set up for “{other}”")),
    }
}

/// A fresh access token and when it stops working.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Access {
    pub token: String,
    pub expires_in: u64,
}

/// Everything but the unreserved characters (RFC 3986).
const RESERVED: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

fn encode(text: &str) -> String {
    utf8_percent_encode(text, RESERVED).to_string()
}

fn form(fields: &[(&str, &str)]) -> String {
    fields.iter().map(|(key, value)| format!("{}={}", encode(key), encode(value))).collect::<Vec<_>>().join("&")
}

fn random_bytes(count: usize) -> Result<Vec<u8>, String> {
    let mut bytes = vec![0u8; count];
    std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes)).map_err(|err| format!("no randomness: {err}"))?;
    Ok(bytes)
}

/// A PKCE verifier and its challenge.
pub fn pkce() -> Result<(String, String), String> {
    let verifier = URL_SAFE_NO_PAD.encode(random_bytes(48)?);
    let challenge = challenge(&verifier);
    Ok((verifier, challenge))
}

pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The page to open in the browser.
pub fn authorize_url(provider: &Provider, client: &Client, redirect: &str, state: &str, challenge: &str) -> String {
    let scope = provider.scopes.join(" ");
    let query = form(&[
        ("client_id", &client.client_id),
        ("redirect_uri", redirect),
        ("response_type", "code"),
        ("scope", &scope),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        // Without these Google sends a refresh token only the first time an app is approved.
        ("access_type", "offline"),
        ("prompt", "consent"),
    ]);
    format!("{}?{query}", provider.auth_url)
}

/// What the browser asked the local port for: `GET /?code=…&state=… HTTP/1.1`.
pub fn parse_redirect(request_line: &str) -> Result<(String, String), String> {
    let target = request_line.split_whitespace().nth(1).ok_or("the browser sent nothing readable")?;
    let query = target.split_once('?').map(|(_, query)| query).unwrap_or_default();
    let mut code = None;
    let mut state = String::new();
    let mut error = None;
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else { continue };
        let value = percent_decode_str(&value.replace('+', " ")).decode_utf8_lossy().into_owned();
        match key {
            "code" => code = Some(value),
            "state" => state = value,
            "error" => error = Some(value),
            _ => {}
        }
    }
    if let Some(error) = error {
        return Err(format!("the sign-in was refused: {error}"));
    }
    Ok((code.ok_or("the browser came back without a code")?, state))
}

/// Waits on `listener` for the redirect, answers the browser with a page saying to close the tab, and returns
/// the code if the state matches. Requests for anything else (a favicon) are answered and ignored.
pub fn wait_for_code(listener: &TcpListener, state: &str, timeout: Duration) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|err| err.to_string())?;
    let end = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(result) = answer(stream, state) {
                    return result;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > end {
                    return Err("the sign-in wasn't finished in time".to_owned());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(err) => return Err(err.to_string()),
        }
    }
}

fn answer(mut stream: TcpStream, state: &str) -> Option<Result<String, String>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = [0u8; 4096];
    let read = stream.read(&mut request).ok()?;
    let first = String::from_utf8_lossy(&request[..read]).lines().next().unwrap_or_default().to_owned();
    let page = |text: &str| format!("<!doctype html><meta charset=utf-8><title>Autark</title><body style=\"font:16px system-ui;margin:3em\"><p>{text}</p>");
    if !first.contains("code=") && !first.contains("error=") {
        let body = page("Waiting for the sign-in.");
        let _ = stream.write_all(format!("HTTP/1.1 404 Not Found\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        return None;
    }
    let result = parse_redirect(&first).and_then(|(code, got)| if got == state { Ok(code) } else { Err("the sign-in came back with the wrong state; try again".to_owned()) });
    let body = page(if result.is_ok() { "Signed in. You can close this tab." } else { "The sign-in didn't work. You can close this tab." });
    let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
    Some(result)
}

fn token_request(url: &str, fields: &[(&str, &str)]) -> Result<Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).http_status_as_error(false).https_only(true).build().into();
    let response = agent.post(url).header("Content-Type", "application/x-www-form-urlencoded").send(form(fields)).map_err(|err| format!("{url}: {err}"))?;
    let status = response.status();
    let text = response.into_body().read_to_string().map_err(|err| format!("{url}: {err}"))?;
    let value: Value = serde_json::from_str(&text).map_err(|_| format!("{url}: the answer isn't JSON ({status})"))?;
    if !status.is_success() {
        let why = value["error_description"].as_str().or(value["error"].as_str()).unwrap_or("refused");
        return Err(format!("sign-in refused: {why}"));
    }
    Ok(value)
}

/// Trades the code for tokens: the access token now and the refresh token to keep.
pub fn exchange(provider: &Provider, client: &Client, code: &str, redirect: &str, verifier: &str) -> Result<(Access, String), String> {
    let reply = token_request(
        provider.token_url,
        &[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", redirect), ("client_id", &client.client_id), ("client_secret", &client.client_secret), ("code_verifier", verifier)],
    )?;
    let (access, refresh) = read_tokens(&reply)?;
    let refresh = refresh.ok_or("the provider sent no refresh token; remove the app's access in your account settings and try again")?;
    Ok((access, refresh))
}

/// A new access token from the refresh token.
pub fn refresh(provider: &Provider, client: &Client, refresh_token: &str) -> Result<Access, String> {
    let reply = token_request(provider.token_url, &[("grant_type", "refresh_token"), ("refresh_token", refresh_token), ("client_id", &client.client_id), ("client_secret", &client.client_secret)])?;
    read_tokens(&reply).map(|(access, _)| access)
}

fn read_tokens(reply: &Value) -> Result<(Access, Option<String>), String> {
    let token = reply["access_token"].as_str().ok_or("the provider sent no access token")?.to_owned();
    let expires_in = reply["expires_in"].as_u64().unwrap_or(3600);
    Ok((Access { token, expires_in }, reply["refresh_token"].as_str().map(str::to_owned)))
}

/// The address that signed in.
pub fn email_of(provider: &Provider, access: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).http_status_as_error(false).https_only(true).build().into();
    let response = agent.get(provider.userinfo_url).header("Authorization", format!("Bearer {access}")).call().map_err(|err| format!("{}: {err}", provider.userinfo_url))?;
    let text = response.into_body().read_to_string().map_err(|err| err.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|_| "the provider's answer about you isn't JSON".to_owned())?;
    value["email"].as_str().map(str::to_owned).ok_or_else(|| "the provider didn't say which address signed in".to_owned())
}

/// The whole sign-in: opens the browser, waits, and returns the first access token and the refresh token.
/// `open` shows the URL to the user (the caller decides how).
pub fn sign_in(provider: &Provider, client: &Client, open: impl FnOnce(&str)) -> Result<(Access, String), String> {
    if client.client_id.is_empty() {
        return Err("no OAuth client is set up: add client_id and client_secret to accounts.toml".to_owned());
    }
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|err| format!("no port to listen on: {err}"))?;
    let redirect = format!("http://127.0.0.1:{}", listener.local_addr().map_err(|err| err.to_string())?.port());
    let (verifier, challenge) = pkce()?;
    let state = URL_SAFE_NO_PAD.encode(random_bytes(18)?);
    open(&authorize_url(provider, client, &redirect, &state, &challenge));
    let code = wait_for_code(&listener, &state, Duration::from_secs(300))?;
    exchange(provider, client, &code, &redirect, &verifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pkce_example_from_the_rfc() {
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let (verifier, challenge_text) = pkce().unwrap();
        assert!(verifier.len() >= 43 && challenge(&verifier) == challenge_text);
    }

    #[test]
    fn the_sign_in_url_asks_for_what_it_needs() {
        let client = Client { client_id: "id 1".into(), client_secret: "s".into() };
        let url = authorize_url(&GOOGLE, &client, "http://127.0.0.1:5000", "st", "ch");
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?client_id=id%201&"));
        for needle in ["redirect_uri=http%3A%2F%2F127.0.0.1%3A5000", "code_challenge_method=S256", "access_type=offline", "https%3A%2F%2Fmail.google.com%2F"] {
            assert!(url.contains(needle), "{needle} missing from {url}");
        }
    }

    #[test]
    fn redirects_are_read() {
        assert_eq!(parse_redirect("GET /?code=4%2Fabc&state=xyz&scope=a+b HTTP/1.1").unwrap(), ("4/abc".to_owned(), "xyz".to_owned()));
        assert!(parse_redirect("GET /?error=access_denied&state=x HTTP/1.1").unwrap_err().contains("access_denied"));
        assert!(parse_redirect("GET / HTTP/1.1").is_err());
    }

    #[test]
    fn the_local_port_hands_over_the_code_only_with_the_right_state() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser = std::thread::spawn(move || {
            for path in ["/favicon.ico", "/?code=wrong&state=nope", "/?code=right&state=good"] {
                let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
                stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).unwrap();
                let mut reply = String::new();
                let _ = stream.read_to_string(&mut reply);
            }
        });
        // The wrong state ends the wait with an error, as it must: the code is not trusted.
        assert!(wait_for_code(&listener, "good", Duration::from_secs(5)).unwrap_err().contains("wrong state"));
        assert_eq!(wait_for_code(&listener, "good", Duration::from_secs(5)).unwrap(), "right");
        browser.join().unwrap();
    }

    #[test]
    fn tokens_are_read_from_replies() {
        let reply = serde_json::json!({ "access_token": "at", "expires_in": 3599, "refresh_token": "rt" });
        assert_eq!(read_tokens(&reply).unwrap(), (Access { token: "at".into(), expires_in: 3599 }, Some("rt".to_owned())));
        assert!(read_tokens(&serde_json::json!({})).is_err());
        assert!(sign_in(&GOOGLE, &Client::default(), |_| {}).unwrap_err().contains("client_id"));
    }
}
