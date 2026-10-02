//! Alibaba Cloud SMS (dysmsapi), RPC style API with signature version 1.0 (HMAC-SHA1).

use std::collections::{BTreeMap, HashMap};

use openssl::hash::MessageDigest;
use serde_json::Value;

use crate::endpoints::common::http;
use crate::Error;

use super::SmsEndpoint;

const URL: &str = "https://dysmsapi.aliyuncs.com/";
const DEFAULT_REGION: &str = "cn-hangzhou";

/// Request parameters, without the signature.
fn build_params(
    endpoint: &SmsEndpoint,
    params: &[(&str, String)],
    nonce: &str,
    timestamp: &str,
) -> BTreeMap<String, String> {
    let config = &endpoint.config;

    let mut res = BTreeMap::new();
    let mut set = |key: &str, value: &str| {
        res.insert(key.to_string(), value.to_string());
    };
    set("AccessKeyId", &endpoint.private_config.access_key);
    set("Action", "SendSms");
    set("Format", "JSON");
    set("PhoneNumbers", &config.phone.join(","));
    set(
        "RegionId",
        config.region.as_deref().unwrap_or(DEFAULT_REGION),
    );
    set("SignName", &config.sign_name);
    set("SignatureMethod", "HMAC-SHA1");
    set("SignatureNonce", nonce);
    set("SignatureVersion", "1.0");
    set("TemplateCode", &config.template_id);
    set("Timestamp", timestamp);
    set("Version", "2017-05-25");

    if !params.is_empty() {
        let template_param: serde_json::Map<String, Value> = params
            .iter()
            .map(|(name, value)| (name.to_string(), Value::from(value.as_str())))
            .collect();
        set("TemplateParam", &Value::Object(template_param).to_string());
    }

    res
}

/// Signature over the sorted, percent encoded parameters.
fn signature(params: &BTreeMap<String, String>, secret: &str) -> Result<String, Error> {
    let canonical = http::form_encode(params.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let string_to_sign = format!(
        "POST&{}&{}",
        http::percent_encode("/"),
        http::percent_encode(&canonical)
    );
    let key = format!("{secret}&");
    let mac = http::hmac(
        MessageDigest::sha1(),
        key.as_bytes(),
        string_to_sign.as_bytes(),
    )?;
    Ok(http::base64(&mac))
}

pub(super) fn send(endpoint: &SmsEndpoint, params: &[(&str, String)]) -> Result<(), Error> {
    let name = &endpoint.config.name;

    let timestamp = proxmox_time::epoch_to_rfc3339_utc(proxmox_time::epoch_i64())
        .map_err(|err| http::failed(name, err.to_string()))?;
    let mut request = build_params(endpoint, params, &http::nonce()?, &timestamp);
    let signature = signature(&request, &endpoint.private_config.secret_key)?;
    request.insert("Signature".into(), signature);

    let body = http::form_encode(request.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let response = http::post(
        name,
        URL,
        body,
        "application/x-www-form-urlencoded",
        &HashMap::new(),
    )?;

    match response["Code"].as_str() {
        Some("OK") => Ok(()),
        code => Err(http::failed(
            name,
            format!(
                "Alibaba Cloud SMS error {}: {}",
                code.unwrap_or("unknown"),
                response["Message"].as_str().unwrap_or_default()
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoints::sms::{SmsConfig, SmsPrivateConfig, SmsProvider};

    fn endpoint() -> SmsEndpoint {
        SmsEndpoint {
            config: SmsConfig {
                name: "sms".into(),
                provider: SmsProvider::Aliyun,
                phone: vec!["13800000000".into(), "13900000000".into()],
                sign_name: "PXVirt".into(),
                template_id: "SMS_123456".into(),
                ..Default::default()
            },
            private_config: SmsPrivateConfig {
                name: "sms".into(),
                access_key: "testid".into(),
                secret_key: "testsecret".into(),
            },
        }
    }

    #[test]
    fn test_signature() {
        let params = build_params(
            &endpoint(),
            &[("hostname", "pve1".into()), ("title", "备份失败".into())],
            "45e25e9b-0a6f-4070-8c85-2956eda1b466",
            "2017-07-12T02:42:19Z",
        );
        assert_eq!(params["PhoneNumbers"], "13800000000,13900000000");
        assert_eq!(
            params["TemplateParam"],
            r#"{"hostname":"pve1","title":"备份失败"}"#
        );

        // computed with an independent implementation of the algorithm
        assert_eq!(
            signature(&params, "testsecret").unwrap(),
            "h/pQxHrkzTjNP61rIDySdJLdWO8="
        );
    }
}
