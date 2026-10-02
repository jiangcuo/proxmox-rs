//! Huawei Cloud Message & SMS (batchSendSms v1) with WSSE authentication.

use std::collections::HashMap;

use serde_json::Value;

use crate::endpoints::common::http;
use crate::Error;

use super::{e164, SmsEndpoint};

const DEFAULT_REGION: &str = "cn-north-4";

fn url(region: Option<&str>) -> String {
    let region = region.unwrap_or(DEFAULT_REGION);
    format!("https://smsapi.{region}.myhuaweicloud.com:443/sms/batchSendSms/v1")
}

/// The X-WSSE header, the password digest is base64(sha256(nonce + created + secret)).
fn wsse_header(app_key: &str, app_secret: &str, nonce: &str, created: &str) -> String {
    let digest = openssl::sha::sha256(format!("{nonce}{created}{app_secret}").as_bytes());
    format!(
        "UsernameToken Username=\"{app_key}\",PasswordDigest=\"{}\",Nonce=\"{nonce}\",Created=\"{created}\"",
        http::base64(&digest)
    )
}

fn build_body(endpoint: &SmsEndpoint, params: &[(&str, String)]) -> String {
    let config = &endpoint.config;
    let phones: Vec<String> = config.phone.iter().map(|p| e164(p)).collect();
    let phones = phones.join(",");
    let values: Vec<&str> = params.iter().map(|(_, value)| value.as_str()).collect();
    let template_params = Value::from(values).to_string();

    let mut fields = vec![
        ("from", config.app_id.as_deref().unwrap_or_default()),
        ("to", phones.as_str()),
        ("templateId", config.template_id.as_str()),
        ("signature", config.sign_name.as_str()),
    ];
    if !params.is_empty() {
        fields.push(("templateParas", template_params.as_str()));
    }
    http::form_encode(fields)
}

pub(super) fn send(endpoint: &SmsEndpoint, params: &[(&str, String)]) -> Result<(), Error> {
    let name = &endpoint.config.name;

    let created = proxmox_time::epoch_to_rfc3339_utc(proxmox_time::epoch_i64())
        .map_err(|err| http::failed(name, err.to_string()))?;
    let headers = HashMap::from([
        (
            "Authorization".to_string(),
            r#"WSSE realm="SDP",profile="UsernameToken",type="Appkey""#.to_string(),
        ),
        (
            "X-WSSE".to_string(),
            wsse_header(
                &endpoint.private_config.access_key,
                &endpoint.private_config.secret_key,
                &http::nonce()?,
                &created,
            ),
        ),
    ]);

    let response = http::post(
        name,
        &url(endpoint.config.region.as_deref()),
        build_body(endpoint, params),
        "application/x-www-form-urlencoded",
        &headers,
    )?;

    match response["code"].as_str() {
        Some("000000") => Ok(()),
        code => Err(http::failed(
            name,
            format!(
                "Huawei Cloud SMS error {}: {}",
                code.unwrap_or("unknown"),
                response["description"].as_str().unwrap_or_default()
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoints::sms::{SmsConfig, SmsPrivateConfig, SmsProvider};

    #[test]
    fn test_request() {
        assert_eq!(
            url(None),
            "https://smsapi.cn-north-4.myhuaweicloud.com:443/sms/batchSendSms/v1"
        );

        let endpoint = SmsEndpoint {
            config: SmsConfig {
                name: "sms".into(),
                provider: SmsProvider::Huawei,
                phone: vec!["13800000000".into(), "+85212345678".into()],
                sign_name: "PXVirt".into(),
                template_id: "tpl1".into(),
                app_id: Some("8820000001".into()),
                ..Default::default()
            },
            private_config: SmsPrivateConfig {
                name: "sms".into(),
                access_key: "key".into(),
                secret_key: "secret".into(),
            },
        };
        assert_eq!(
            build_body(&endpoint, &[("hostname", "pve1".into())]),
            "from=8820000001&to=%2B8613800000000%2C%2B85212345678&templateId=tpl1\
             &signature=PXVirt&templateParas=%5B%22pve1%22%5D"
        );
    }

    #[test]
    fn test_wsse_header() {
        // computed with an independent implementation of the algorithm
        assert_eq!(
            wsse_header(
                "appkey",
                "appsecret",
                "0123456789abcdef",
                "2024-01-01T00:00:00Z"
            ),
            "UsernameToken Username=\"appkey\",\
             PasswordDigest=\"14z1UhQGIu/Wm/y/PezOKYU/8RUCfltkEO/l0WGOOq0=\",\
             Nonce=\"0123456789abcdef\",Created=\"2024-01-01T00:00:00Z\""
        );
    }
}
