use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use proxmox_schema::api_types::{COMMENT_SCHEMA, HTTP_URL_SCHEMA};
use proxmox_schema::{api, Updater};

use crate::endpoints::common::http;
use crate::schema::ENTITY_NAME_SCHEMA;
use crate::{Endpoint, Error, Notification, Origin};

pub(crate) const WECOM_TYPENAME: &str = "wecom";

/// Maximum size of a markdown message
const MAX_MESSAGE_SIZE: usize = 4096;

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
/// Config for WeCom (WeChat Work) robot notification endpoints
pub struct WecomConfig {
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
/// Private configuration for WeCom robot notification endpoints.
/// This config will be saved to a separate configuration file with stricter
/// permissions (root:root 0600)
pub struct WecomPrivateConfig {
    /// Name of the endpoint
    #[updater(skip)]
    pub name: String,
    /// Webhook URL of the robot, including the key.
    pub url: String,
}

/// A WeCom robot notification endpoint.
pub struct WecomEndpoint {
    pub config: WecomConfig,
    pub private_config: WecomPrivateConfig,
}

#[api]
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// The set of properties that can be deleted from a WeCom endpoint configuration.
pub enum DeleteableWecomProperty {
    /// Delete `comment`
    Comment,
    /// Delete `disable`
    Disable,
}

impl Endpoint for WecomEndpoint {
    fn send(&self, notification: &Notification) -> Result<(), Error> {
        let (title, body) = http::render(notification)?;
        let content = format!("**{title}**\n{body}");

        let request = json!({
            "msgtype": "markdown",
            "markdown": {
                "content": http::truncate_bytes(&content, MAX_MESSAGE_SIZE),
            },
        });

        let response = http::post(
            self.name(),
            &self.private_config.url,
            request.to_string(),
            "application/json",
            &HashMap::new(),
        )?;

        match response["errcode"].as_i64() {
            Some(0) => Ok(()),
            _ => Err(http::failed(
                self.name(),
                format!(
                    "WeCom error {}: {}",
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
