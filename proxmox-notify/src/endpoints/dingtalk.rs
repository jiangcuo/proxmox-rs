use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use proxmox_schema::api_types::{COMMENT_SCHEMA, HTTP_URL_SCHEMA};
use proxmox_schema::{api, Updater};

use crate::endpoints::common::http;
use crate::schema::ENTITY_NAME_SCHEMA;
use crate::{Endpoint, Error, Notification, Origin};

pub(crate) const DINGTALK_TYPENAME: &str = "dingtalk";

/// Maximum size of a message
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
/// Config for DingTalk robot notification endpoints
pub struct DingtalkConfig {
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
/// Private configuration for DingTalk robot notification endpoints.
/// This config will be saved to a separate configuration file with stricter
/// permissions (root:root 0600)
pub struct DingtalkPrivateConfig {
    /// Name of the endpoint
    #[updater(skip)]
    pub name: String,
    /// Webhook URL of the robot, including the access token.
    pub url: String,
    /// Secret for signing requests, if the robot requires signed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

/// A DingTalk robot notification endpoint.
pub struct DingtalkEndpoint {
    pub config: DingtalkConfig,
    pub private_config: DingtalkPrivateConfig,
}

#[api]
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// The set of properties that can be deleted from a DingTalk endpoint configuration.
pub enum DeleteableDingtalkProperty {
    /// Delete `comment`
    Comment,
    /// Delete `disable`
    Disable,
    /// Delete `secret`
    Secret,
}

/// Sign a request, returns the query parameters to append to the URL.
fn sign(timestamp_ms: i64, secret: &str) -> Result<String, Error> {
    let string_to_sign = format!("{timestamp_ms}\n{secret}");
    let mac = http::hmac_sha256(secret.as_bytes(), string_to_sign.as_bytes())?;
    let sign = http::percent_encode(&http::base64(&mac));
    Ok(format!("timestamp={timestamp_ms}&sign={sign}"))
}

fn timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

impl Endpoint for DingtalkEndpoint {
    fn send(&self, notification: &Notification) -> Result<(), Error> {
        let (title, body) = http::render(notification)?;
        let content = format!("{title}\n\n{body}");

        let request = json!({
            "msgtype": "text",
            "text": {
                "content": http::truncate_bytes(&content, MAX_MESSAGE_SIZE),
            },
        });

        let mut url = self.private_config.url.clone();
        if let Some(secret) = self.private_config.secret.as_deref() {
            let separator = if url.contains('?') { '&' } else { '?' };
            url = format!("{url}{separator}{}", sign(timestamp_ms(), secret)?);
        }

        let response = http::post(
            self.name(),
            &url,
            request.to_string(),
            "application/json",
            &HashMap::new(),
        )?;

        match response["errcode"].as_i64() {
            Some(0) => Ok(()),
            _ => Err(http::failed(
                self.name(),
                format!(
                    "DingTalk error {}: {}",
                    response["errcode"],
                    response["errmsg"].as_str().unwrap_or_default()
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
    fn test_sign() {
        // computed with an independent implementation of the algorithm
        assert_eq!(
            sign(1700000000000, "SECtest").unwrap(),
            "timestamp=1700000000000&sign=aZLLrriXgn05YbwaGR7knYsLeJADjr9NwLaNNKpxh4g%3D"
        );
    }
}
