//! Tencent Cloud SMS (API 2021-01-11) with TC3-HMAC-SHA256 signatures.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::endpoints::common::http;
use crate::Error;

use super::{e164, SmsEndpoint};

const HOST: &str = "sms.tencentcloudapi.com";
const SERVICE: &str = "sms";
const CONTENT_TYPE: &str = "application/json; charset=utf-8";
const DEFAULT_REGION: &str = "ap-guangzhou";

fn build_payload(endpoint: &SmsEndpoint, params: &[(&str, String)]) -> Value {
    let config = &endpoint.config;
    let phones: Vec<String> = config.phone.iter().map(|p| e164(p)).collect();

    let mut payload = json!({
        "PhoneNumberSet": phones,
        "SmsSdkAppId": config.app_id.as_deref().unwrap_or_default(),
        "SignName": config.sign_name,
        "TemplateId": config.template_id,
    });
    if !params.is_empty() {
        let values: Vec<&str> = params.iter().map(|(_, value)| value.as_str()).collect();
        payload["TemplateParamSet"] = json!(values);
    }
    payload
}

/// The Authorization header for a request with the given payload.
fn authorization(
    secret_id: &str,
    secret_key: &str,
    timestamp: i64,
    payload: &str,
) -> Result<String, Error> {
    let date = proxmox_time::strftime_utc("%Y-%m-%d", timestamp)
        .map_err(|err| Error::Generic(err.to_string()))?;

    let canonical_request = format!(
        "POST\n/\n\ncontent-type:{CONTENT_TYPE}\nhost:{HOST}\n\ncontent-type;host\n{}",
        http::hex(&openssl::sha::sha256(payload.as_bytes()))
    );
    let scope = format!("{date}/{SERVICE}/tc3_request");
    let string_to_sign = format!(
        "TC3-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        http::hex(&openssl::sha::sha256(canonical_request.as_bytes()))
    );

    let secret_date = http::hmac_sha256(format!("TC3{secret_key}").as_bytes(), date.as_bytes())?;
    let secret_service = http::hmac_sha256(&secret_date, SERVICE.as_bytes())?;
    let secret_signing = http::hmac_sha256(&secret_service, b"tc3_request")?;
    let signature = http::hex(&http::hmac_sha256(
        &secret_signing,
        string_to_sign.as_bytes(),
    )?);

    Ok(format!(
        "TC3-HMAC-SHA256 Credential={secret_id}/{scope}, SignedHeaders=content-type;host, Signature={signature}"
    ))
}

pub(super) fn send(endpoint: &SmsEndpoint, params: &[(&str, String)]) -> Result<(), Error> {
    let name = &endpoint.config.name;
    let private_config = &endpoint.private_config;

    let payload = build_payload(endpoint, params).to_string();
    let timestamp = proxmox_time::epoch_i64();

    let headers = HashMap::from([
        (
            "Authorization".to_string(),
            authorization(
                &private_config.access_key,
                &private_config.secret_key,
                timestamp,
                &payload,
            )?,
        ),
        ("X-TC-Action".to_string(), "SendSms".to_string()),
        ("X-TC-Version".to_string(), "2021-01-11".to_string()),
        ("X-TC-Timestamp".to_string(), timestamp.to_string()),
        (
            "X-TC-Region".to_string(),
            endpoint
                .config
                .region
                .clone()
                .unwrap_or_else(|| DEFAULT_REGION.to_string()),
        ),
    ]);

    let response = http::post(
        name,
        &format!("https://{HOST}/"),
        payload,
        CONTENT_TYPE,
        &headers,
    )?;
    let response = &response["Response"];

    if let Some(error) = response.get("Error") {
        return Err(http::failed(
            name,
            format!(
                "Tencent Cloud SMS error {}: {}",
                error["Code"].as_str().unwrap_or("unknown"),
                error["Message"].as_str().unwrap_or_default()
            ),
        ));
    }

    // the status is reported per phone number
    let failed: Vec<String> = response["SendStatusSet"]
        .as_array()
        .map(|set| set.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|status| status["Code"].as_str() != Some("Ok"))
        .map(|status| {
            format!(
                "{}: {} {}",
                status["PhoneNumber"].as_str().unwrap_or_default(),
                status["Code"].as_str().unwrap_or("unknown"),
                status["Message"].as_str().unwrap_or_default()
            )
        })
        .collect();

    if failed.is_empty() {
        Ok(())
    } else {
        Err(http::failed(
            name,
            format!("Tencent Cloud SMS error: {}", failed.join(", ")),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoints::sms::{SmsConfig, SmsPrivateConfig, SmsProvider};

    #[test]
    fn test_payload() {
        let endpoint = SmsEndpoint {
            config: SmsConfig {
                name: "sms".into(),
                provider: SmsProvider::Tencent,
                phone: vec!["13800000000".into()],
                sign_name: "PXVirt".into(),
                template_id: "1234567".into(),
                app_id: Some("1400000000".into()),
                ..Default::default()
            },
            private_config: SmsPrivateConfig {
                name: "sms".into(),
                access_key: "id".into(),
                secret_key: "key".into(),
            },
        };
        let payload = build_payload(&endpoint, &[("hostname", "pve1".into())]);
        assert_eq!(payload["PhoneNumberSet"], json!(["+8613800000000"]));
        assert_eq!(payload["TemplateParamSet"], json!(["pve1"]));
        assert_eq!(payload["SmsSdkAppId"], "1400000000");
    }

    #[test]
    fn test_authorization() {
        // computed with an independent implementation of the algorithm
        assert_eq!(
            authorization("AKIDtest", "secretkey", 1551113065, r#"{"Limit":1}"#).unwrap(),
            "TC3-HMAC-SHA256 Credential=AKIDtest/2019-02-25/sms/tc3_request, \
             SignedHeaders=content-type;host, \
             Signature=7b219d495b4cc0acea419fecb24505f339134ec9a8bf69ab21209dcb78fdb260"
        );
    }
}
