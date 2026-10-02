//! Helpers for endpoints talking to chat bots and SMS services over HTTP.

use std::collections::HashMap;
use std::time::Duration;

use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::sign::Signer;
use serde_json::Value;

use proxmox_http::client::sync::Client;
use proxmox_http::{HttpClient, HttpOptions, ProxyConfig};

use crate::context::context;
use crate::renderer::{self, TemplateType};
use crate::{Content, Error, Notification};

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// Render the title and the plain text body of a notification.
pub(crate) fn render(notification: &Notification) -> Result<(String, String), Error> {
    match &notification.content {
        Content::Template {
            template_name,
            data,
        } => Ok((
            renderer::render_template(TemplateType::Subject, template_name, data)?,
            renderer::render_template(TemplateType::PlaintextBody, template_name, data)?,
        )),
        #[cfg(feature = "mail-forwarder")]
        Content::ForwardedMail { title, body, .. } => Ok((title.clone(), body.clone())),
    }
}

pub(crate) fn failed(endpoint: &str, msg: impl Into<String>) -> Error {
    Error::NotifyFailed(endpoint.to_string(), msg.into().into())
}

/// Send a POST request and return the response parsed as JSON.
pub(crate) fn post(
    endpoint: &str,
    url: &str,
    body: String,
    content_type: &str,
    headers: &HashMap<String, String>,
) -> Result<Value, Error> {
    let proxy_config = context()
        .http_proxy_config()
        .map(|url| ProxyConfig::parse_proxy_url(&url))
        .transpose()
        .map_err(|err| Error::NotifyFailed(endpoint.to_string(), err.into()))?;

    let options = HttpOptions {
        proxy_config,
        ..Default::default()
    };

    let response = Client::new_with_timeout(options, HTTP_TIMEOUT)
        .post(url, Some(body), Some(content_type), Some(headers))
        .map_err(|err| Error::NotifyFailed(endpoint.to_string(), err.into()))?;

    serde_json::from_str(response.body())
        .map_err(|err| failed(endpoint, format!("invalid response: {err}")))
}

pub(crate) fn hmac(digest: MessageDigest, key: &[u8], data: &[u8]) -> Result<Vec<u8>, Error> {
    let sign = || -> Result<Vec<u8>, openssl::error::ErrorStack> {
        let key = PKey::hmac(key)?;
        let mut signer = Signer::new(digest, &key)?;
        signer.update(data)?;
        signer.sign_to_vec()
    };
    sign().map_err(|err| Error::Generic(format!("could not compute signature: {err}")))
}

pub(crate) fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<Vec<u8>, Error> {
    hmac(MessageDigest::sha256(), key, data)
}

pub(crate) fn base64(data: &[u8]) -> String {
    openssl::base64::encode_block(data)
}

pub(crate) fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// Percent-encode everything except the unreserved characters of RFC 3986.
pub(crate) fn percent_encode(value: &str) -> String {
    let mut res = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                res.push(byte as char)
            }
            _ => res.push_str(&format!("%{byte:02X}")),
        }
    }
    res
}

/// Encode parameters as `application/x-www-form-urlencoded` body.
pub(crate) fn form_encode<'a>(params: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    params
        .into_iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Random hex string, used as nonce.
pub(crate) fn nonce() -> Result<String, Error> {
    let mut buf = [0u8; 16];
    openssl::rand::rand_bytes(&mut buf)
        .map_err(|err| Error::Generic(format!("could not generate nonce: {err}")))?;
    Ok(hex(&buf))
}

/// Truncate to at most `max` bytes at a character boundary.
pub(crate) fn truncate_bytes(value: &str, max: usize) -> &str {
    if value.len() <= max {
        return value;
    }
    let mut end = max;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

/// Truncate to at most `max` characters.
pub(crate) fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_percent_encode() {
        assert_eq!(percent_encode("aZ09-_.~"), "aZ09-_.~");
        assert_eq!(percent_encode("a b/c*"), "a%20b%2Fc%2A");
        assert_eq!(percent_encode("测"), "%E6%B5%8B");
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate_bytes("测试", 4), "测");
        assert_eq!(truncate_bytes("abc", 4), "abc");
        assert_eq!(truncate_chars("测试abc", 3), "测试a");
    }

    #[test]
    fn test_hmac_sha256() {
        // RFC 4231, test case 2
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?").unwrap();
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }
}
