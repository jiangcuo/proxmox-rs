use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use proxmox_schema::api_types::{COMMENT_SCHEMA, HTTP_URL_SCHEMA};
use proxmox_schema::{api, Updater};

use crate::endpoints::common::http;
use crate::schema::ENTITY_NAME_SCHEMA;
use crate::{Endpoint, Error, Notification, Origin};

pub(crate) const FEISHU_TYPENAME: &str = "feishu";

/// Maximum size of the message body
const MAX_MESSAGE_SIZE: usize = 20000;

#[api(
    properties: {
        name: {
            schema: ENTITY_NAME_SCHEMA,
        },
        comment: {
            optional: true,
            schema: COMMENT_SCHEMA,
        },
    }
)]
#[derive(Serialize, Deserialize, Updater, Default)]
#[serde(rename_all = "kebab-case")]
/// Config for Feishu (Lark) bot notification endpoints
pub struct FeishuConfig {
    /// Name of the endpoint.
    #[updater(skip)]
    pub name: String,
    /// Comment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Disable this target.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable: Option<bool>,
    /// Origin of this config entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[updater(skip)]
    pub origin: Option<Origin>,
}

#[api(
    properties: {
        url: {
            schema: HTTP_URL_SCHEMA,
        },
    }
)]
#[derive(Serialize, Deserialize, Clone, Updater)]
#[serde(rename_all = "kebab-case")]
/// Private configuration for Feishu bot notification endpoints.
/// This config will be saved to a separate configuration file with stricter
/// permissions (root:root 0600)
pub struct FeishuPrivateConfig {
    /// Name of the endpoint
    #[updater(skip)]
    pub name: String,
    /// Webhook URL of the bot.
    pub url: String,
    /// Secret for signing requests, if the bot requires signed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

/// A Feishu bot notification endpoint.
pub struct FeishuEndpoint {
    pub config: FeishuConfig,
    pub private_config: FeishuPrivateConfig,
}

#[api]
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// The set of properties that can be deleted from a Feishu endpoint configuration.
pub enum DeleteableFeishuProperty {
    /// Delete `comment`
    Comment,
    /// Delete `disable`
    Disable,
    /// Delete `secret`
    Secret,
}

/// Feishu uses "timestamp\nsecret" as key to sign empty data.
fn sign(timestamp: i64, secret: &str) -> Result<String, Error> {
    let key = format!("{timestamp}\n{secret}");
    Ok(http::base64(&http::hmac_sha256(key.as_bytes(), b"")?))
}

fn build_request(title: &str, body: &str, signature: Option<(i64, String)>) -> Value {
    let mut request = json!({
        "msg_type": "post",
        "content": {
            "post": {
                "zh_cn": {
                    "title": title,
                    "content": [[{
                        "tag": "text",
                        "text": http::truncate_bytes(body, MAX_MESSAGE_SIZE),
                    }]],
                },
            },
        },
    });

    if let Some((timestamp, sign)) = signature {
        // the timestamp must be passed as string
        request["timestamp"] = timestamp.to_string().into();
        request["sign"] = sign.into();
    }

    request
}

impl Endpoint for FeishuEndpoint {
    fn send(&self, notification: &Notification) -> Result<(), Error> {
        let (title, body) = http::render(notification)?;

        let signature = match self.private_config.secret.as_deref() {
            Some(secret) => {
                let timestamp = proxmox_time::epoch_i64();
                Some((timestamp, sign(timestamp, secret)?))
            }
            None => None,
        };
        let request = build_request(&title, &body, signature);

        let response = http::post(
            self.name(),
            &self.private_config.url,
            request.to_string(),
            "application/json",
            &HashMap::new(),
        )?;

        // older API versions report 'StatusCode' instead of 'code'
        let code = response["code"]
            .as_i64()
            .or_else(|| response["StatusCode"].as_i64());
        match code {
            Some(0) => Ok(()),
            _ => Err(http::failed(
                self.name(),
                format!(
                    "Feishu error {}: {}",
                    code.unwrap_or(-1),
                    response["msg"].as_str().unwrap_or_default()
                ),
            )),
        }
    }

    fn name(&self) -> &str {
        &self.config.name
    }

    /// Check if the endpoint is disabled
    fn disabled(&self) -> bool {
        self.config.disable.unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request() {
        let request = build_request("title", "body", Some((1700000000, "sig".into())));
        assert_eq!(request["timestamp"], "1700000000");
        assert_eq!(request["sign"], "sig");
        assert_eq!(request["content"]["post"]["zh_cn"]["title"], "title");

        let request = build_request("title", "body", None);
        assert!(request.get("sign").is_none());
    }
}
