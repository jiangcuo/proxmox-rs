//! SMS notification endpoints. Messages are sent with a template registered at the SMS
//! provider, the template parameters are filled with information about the notification.

use serde::{Deserialize, Serialize};

use proxmox_schema::api_types::{COMMENT_SCHEMA, SINGLE_LINE_COMMENT_FORMAT};
use proxmox_schema::{api, const_regex, ApiStringFormat, Schema, StringSchema, Updater};

use crate::endpoints::common::http;
use crate::schema::ENTITY_NAME_SCHEMA;
use crate::{Endpoint, Error, Notification, Origin};

mod aliyun;
mod huawei;
mod tencent;

pub(crate) const SMS_TYPENAME: &str = "sms";

/// Maximum length of a template parameter, longer values are truncated. Providers reject
/// messages with too long parameters.
const MAX_PARAM_LENGTH: usize = 35;

const_regex! {
    PHONE_REGEX = r"^\+?[0-9]{5,20}$";
}

pub const PHONE_SCHEMA: Schema =
    StringSchema::new("Phone number, with country code for numbers outside of China.")
        .format(&ApiStringFormat::Pattern(&PHONE_REGEX))
        .schema();

pub const SMS_TEXT_SCHEMA: Schema = StringSchema::new("SMS provider setting.")
    .format(&SINGLE_LINE_COMMENT_FORMAT)
    .min_length(1)
    .max_length(128)
    .schema();

#[api]
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "kebab-case")]
/// SMS provider.
pub enum SmsProvider {
    /// Alibaba Cloud SMS
    #[default]
    Aliyun,
    /// Tencent Cloud SMS
    Tencent,
    /// Huawei Cloud Message & SMS
    Huawei,
}

#[api]
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "kebab-case")]
/// Information passed as SMS template parameter.
pub enum SmsTemplateParam {
    /// Title of the notification
    Title,
    /// Severity of the notification
    Severity,
    /// Host name of the node
    Hostname,
    /// Type of the notification, e.g. vzdump
    Type,
    /// Time of the notification
    Time,
}

impl SmsTemplateParam {
    fn name(&self) -> &'static str {
        match self {
            SmsTemplateParam::Title => "title",
            SmsTemplateParam::Severity => "severity",
            SmsTemplateParam::Hostname => "hostname",
            SmsTemplateParam::Type => "type",
            SmsTemplateParam::Time => "time",
        }
    }
}

#[api(
    properties: {
        name: {
            schema: ENTITY_NAME_SCHEMA,
        },
        phone: {
            type: Array,
            items: {
                schema: PHONE_SCHEMA,
            },
        },
        "sign-name": {
            schema: SMS_TEXT_SCHEMA,
        },
        "template-id": {
            schema: SMS_TEXT_SCHEMA,
        },
        "template-param": {
            type: Array,
            items: {
                type: SmsTemplateParam,
            },
            optional: true,
        },
        "app-id": {
            schema: SMS_TEXT_SCHEMA,
            optional: true,
        },
        region: {
            schema: SMS_TEXT_SCHEMA,
            optional: true,
        },
        comment: {
            optional: true,
            schema: COMMENT_SCHEMA,
        },
    }
)]
#[derive(Serialize, Deserialize, Updater, Default)]
#[serde(rename_all = "kebab-case")]
/// Config for SMS notification endpoints
pub struct SmsConfig {
    /// Name of the endpoint.
    #[updater(skip)]
    pub name: String,
    /// SMS provider.
    pub provider: SmsProvider,
    /// Phone numbers to send the message to.
    #[serde(default)]
    #[updater(serde(skip_serializing_if = "Option::is_none"))]
    pub phone: Vec<String>,
    /// Signature name registered at the provider.
    pub sign_name: String,
    /// ID of the template registered at the provider.
    pub template_id: String,
    /// Template parameters, by name for Alibaba Cloud, in order for Tencent and Huawei Cloud.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[updater(serde(skip_serializing_if = "Option::is_none"))]
    pub template_param: Vec<SmsTemplateParam>,
    /// SMS application ID (Tencent Cloud SdkAppId) or sender (Huawei Cloud channel number).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// Region of the provider, e.g. cn-hangzhou (Alibaba), ap-guangzhou (Tencent) or
    /// cn-north-4 (Huawei).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
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
        "access-key": {
            schema: SMS_TEXT_SCHEMA,
        },
        "secret-key": {
            schema: SMS_TEXT_SCHEMA,
        },
    }
)]
#[derive(Serialize, Deserialize, Clone, Updater)]
#[serde(rename_all = "kebab-case")]
/// Private configuration for SMS notification endpoints.
/// This config will be saved to a separate configuration file with stricter
/// permissions (root:root 0600)
pub struct SmsPrivateConfig {
    /// Name of the endpoint
    #[updater(skip)]
    pub name: String,
    /// Access key ID (Alibaba), SecretId (Tencent) or application key (Huawei).
    pub access_key: String,
    /// Access key secret (Alibaba), SecretKey (Tencent) or application secret (Huawei).
    pub secret_key: String,
}

/// An SMS notification endpoint.
pub struct SmsEndpoint {
    pub config: SmsConfig,
    pub private_config: SmsPrivateConfig,
}

#[api]
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// The set of properties that can be deleted from an SMS endpoint configuration.
pub enum DeleteableSmsProperty {
    /// Delete `comment`
    Comment,
    /// Delete `disable`
    Disable,
    /// Delete `template-param`
    TemplateParam,
    /// Delete `app-id`
    AppId,
    /// Delete `region`
    Region,
}

impl SmsConfig {
    /// Check the provider specific settings.
    pub fn verify(&self) -> Result<(), String> {
        if self.phone.is_empty() {
            return Err("at least one phone number is required".into());
        }
        match self.provider {
            SmsProvider::Tencent if self.app_id.is_none() => {
                Err("Tencent Cloud requires the SMS application ID (app-id)".into())
            }
            SmsProvider::Huawei if self.app_id.is_none() => {
                Err("Huawei Cloud requires the sender channel number (app-id)".into())
            }
            _ => Ok(()),
        }
    }
}

/// Add the country code of China to numbers without one, Tencent and Huawei Cloud require
/// numbers in E.164 format.
fn e164(phone: &str) -> String {
    if phone.starts_with('+') {
        phone.to_string()
    } else {
        format!("+86{phone}")
    }
}

impl SmsEndpoint {
    /// Values of the configured template parameters.
    fn template_params(
        &self,
        notification: &Notification,
    ) -> Result<Vec<(&'static str, String)>, Error> {
        let params = &self.config.template_param;
        let fields = &notification.metadata.additional_fields;

        let title = if params.contains(&SmsTemplateParam::Title) {
            http::render(notification)?.0
        } else {
            String::new()
        };

        Ok(params
            .iter()
            .map(|param| {
                let value = match param {
                    SmsTemplateParam::Title => title.clone(),
                    SmsTemplateParam::Severity => notification.metadata.severity.to_string(),
                    SmsTemplateParam::Hostname => {
                        fields.get("hostname").cloned().unwrap_or_default()
                    }
                    SmsTemplateParam::Type => fields.get("type").cloned().unwrap_or_default(),
                    SmsTemplateParam::Time => proxmox_time::strftime_local(
                        "%Y-%m-%d %H:%M",
                        notification.metadata.timestamp,
                    )
                    .unwrap_or_default(),
                };
                (param.name(), http::truncate_chars(&value, MAX_PARAM_LENGTH))
            })
            .collect())
    }
}

impl Endpoint for SmsEndpoint {
    fn send(&self, notification: &Notification) -> Result<(), Error> {
        self.config
            .verify()
            .map_err(|err| http::failed(self.name(), err))?;

        let params = self.template_params(notification)?;

        match self.config.provider {
            SmsProvider::Aliyun => aliyun::send(self, &params),
            SmsProvider::Tencent => tencent::send(self, &params),
            SmsProvider::Huawei => huawei::send(self, &params),
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
    fn test_e164() {
        assert_eq!(e164("13800000000"), "+8613800000000");
        assert_eq!(e164("+85212345678"), "+85212345678");
    }

    #[test]
    fn test_verify() {
        let mut config = SmsConfig {
            provider: SmsProvider::Tencent,
            phone: vec!["13800000000".into()],
            ..Default::default()
        };
        assert!(config.verify().is_err());
        config.app_id = Some("1400000000".into());
        assert!(config.verify().is_ok());
        config.phone.clear();
        assert!(config.verify().is_err());
    }
}
