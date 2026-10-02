use proxmox_http_error::HttpError;

use crate::api::{http_bail, http_err};
use crate::endpoints::sms::{
    DeleteableSmsProperty, SmsConfig, SmsConfigUpdater, SmsPrivateConfig, SmsPrivateConfigUpdater,
    SMS_TYPENAME,
};
use crate::Config;

/// Get a list of all SMS endpoints.
///
/// The caller is responsible for any needed permission checks.
/// Returns a list of all SMS endpoints or a `HttpError` if the config is
/// erroneous (`500 Internal server error`).
pub fn get_endpoints(config: &Config) -> Result<Vec<SmsConfig>, HttpError> {
    config
        .config
        .convert_to_typed_array(SMS_TYPENAME)
        .map_err(|e| http_err!(NOT_FOUND, "Could not fetch endpoints: {e}"))
}

/// Get SMS endpoint with given `name`
///
/// The caller is responsible for any needed permission checks.
/// Returns the endpoint or a `HttpError` if the endpoint was not found (`404 Not found`).
pub fn get_endpoint(config: &Config, name: &str) -> Result<SmsConfig, HttpError> {
    config
        .config
        .lookup(SMS_TYPENAME, name)
        .map_err(|_| http_err!(NOT_FOUND, "endpoint '{name}' not found"))
}

/// Add a new SMS endpoint.
///
/// The caller is responsible for any needed permission checks.
/// The caller also responsible for locking the configuration files.
/// Returns a `HttpError` if:
///   - an entity with the same name already exists (`400 Bad request`)
///   - the provider specific settings are incomplete (`400 Bad request`)
///   - the configuration could not be saved (`500 Internal server error`)
///
/// Panics if the names of the private config and the public config do not match.
pub fn add_endpoint(
    config: &mut Config,
    endpoint_config: SmsConfig,
    private_endpoint_config: SmsPrivateConfig,
) -> Result<(), HttpError> {
    if endpoint_config.name != private_endpoint_config.name {
        // Programming error by the user of the crate, thus we panic
        panic!("name for endpoint config and private config must be identical");
    }

    super::ensure_unique(config, &endpoint_config.name)?;

    if let Err(err) = endpoint_config.verify() {
        http_bail!(BAD_REQUEST, "{err}");
    }

    set_private_config_entry(config, &private_endpoint_config)?;

    config
        .config
        .set_data(&endpoint_config.name, SMS_TYPENAME, &endpoint_config)
        .map_err(|e| {
            http_err!(
                INTERNAL_SERVER_ERROR,
                "could not save endpoint '{}': {e}",
                endpoint_config.name
            )
        })
}

/// Update existing SMS endpoint
///
/// The caller is responsible for any needed permission checks.
/// The caller also responsible for locking the configuration files.
/// Returns a `HttpError` if:
///   - the endpoint does not exist (`404 Not found`)
///   - the provider specific settings are incomplete (`400 Bad request`)
///   - the configuration could not be saved (`500 Internal server error`)
pub fn update_endpoint(
    config: &mut Config,
    name: &str,
    endpoint_config_updater: SmsConfigUpdater,
    private_endpoint_config_updater: SmsPrivateConfigUpdater,
    delete: Option<&[DeleteableSmsProperty]>,
    digest: Option<&[u8]>,
) -> Result<(), HttpError> {
    super::verify_digest(config, digest)?;

    let mut endpoint = get_endpoint(config, name)?;
    let mut private_endpoint = get_private_config(config, name)?;

    if let Some(delete) = delete {
        for deletable_property in delete {
            match deletable_property {
                DeleteableSmsProperty::Comment => endpoint.comment = None,
                DeleteableSmsProperty::Disable => endpoint.disable = None,
                DeleteableSmsProperty::TemplateParam => endpoint.template_param.clear(),
                DeleteableSmsProperty::AppId => endpoint.app_id = None,
                DeleteableSmsProperty::Region => endpoint.region = None,
            }
        }
    }

    let SmsConfigUpdater {
        provider,
        phone,
        sign_name,
        template_id,
        template_param,
        app_id,
        region,
        comment,
        disable,
    } = endpoint_config_updater;

    if let Some(provider) = provider {
        endpoint.provider = provider;
    }
    if let Some(phone) = phone {
        endpoint.phone = phone;
    }
    if let Some(sign_name) = sign_name {
        endpoint.sign_name = sign_name;
    }
    if let Some(template_id) = template_id {
        endpoint.template_id = template_id;
    }
    if let Some(template_param) = template_param {
        endpoint.template_param = template_param;
    }
    if let Some(app_id) = app_id {
        endpoint.app_id = Some(app_id);
    }
    if let Some(region) = region {
        endpoint.region = Some(region);
    }
    if let Some(comment) = comment {
        endpoint.comment = Some(comment);
    }
    if let Some(disable) = disable {
        endpoint.disable = Some(disable);
    }

    if let Some(access_key) = private_endpoint_config_updater.access_key {
        private_endpoint.access_key = access_key;
    }
    if let Some(secret_key) = private_endpoint_config_updater.secret_key {
        private_endpoint.secret_key = secret_key;
    }

    if let Err(err) = endpoint.verify() {
        http_bail!(BAD_REQUEST, "{err}");
    }

    set_private_config_entry(config, &private_endpoint)?;

    config
        .config
        .set_data(name, SMS_TYPENAME, &endpoint)
        .map_err(|e| {
            http_err!(
                INTERNAL_SERVER_ERROR,
                "could not save endpoint '{name}': {e}"
            )
        })
}

/// Delete existing SMS endpoint
///
/// The caller is responsible for any needed permission checks.
/// The caller also responsible for locking the configuration files.
/// Returns a `HttpError` if:
///   - the entity does not exist (`404 Not found`)
///   - the endpoint is still referenced by another entity (`400 Bad request`)
pub fn delete_sms_endpoint(config: &mut Config, name: &str) -> Result<(), HttpError> {
    // Check if the endpoint exists
    let _ = get_endpoint(config, name)?;
    super::ensure_safe_to_delete(config, name)?;

    config.private_config.sections.remove(name);
    config.config.sections.remove(name);

    Ok(())
}

fn get_private_config(config: &Config, name: &str) -> Result<SmsPrivateConfig, HttpError> {
    config
        .private_config
        .lookup(SMS_TYPENAME, name)
        .map_err(|_| http_err!(NOT_FOUND, "private config for endpoint '{name}' not found"))
}

fn set_private_config_entry(
    config: &mut Config,
    private_config: &SmsPrivateConfig,
) -> Result<(), HttpError> {
    config
        .private_config
        .set_data(&private_config.name, SMS_TYPENAME, private_config)
        .map_err(|e| {
            http_err!(
                INTERNAL_SERVER_ERROR,
                "could not save private config for endpoint '{}': {e}",
                private_config.name
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::empty_config;
    use crate::endpoints::sms::{SmsProvider, SmsTemplateParam};

    fn add_default_endpoint(config: &mut Config) -> Result<(), HttpError> {
        add_endpoint(
            config,
            SmsConfig {
                name: "sms-endpoint".into(),
                provider: SmsProvider::Aliyun,
                phone: vec!["13800000000".into()],
                sign_name: "PXVirt".into(),
                template_id: "SMS_123".into(),
                template_param: vec![SmsTemplateParam::Hostname, SmsTemplateParam::Title],
                ..Default::default()
            },
            SmsPrivateConfig {
                name: "sms-endpoint".into(),
                access_key: "key".into(),
                secret_key: "secret".into(),
            },
        )
    }

    #[test]
    fn test_add_requires_provider_settings() {
        let mut config = empty_config();

        let result = add_endpoint(
            &mut config,
            SmsConfig {
                name: "sms".into(),
                provider: SmsProvider::Tencent,
                phone: vec!["13800000000".into()],
                ..Default::default()
            },
            SmsPrivateConfig {
                name: "sms".into(),
                access_key: "key".into(),
                secret_key: "secret".into(),
            },
        );
        assert!(result.is_err());
        assert!(get_endpoint(&config, "sms").is_err());
    }

    #[test]
    fn test_sms_update() -> Result<(), HttpError> {
        let mut config = empty_config();
        add_default_endpoint(&mut config)?;

        // switching to Tencent Cloud requires the application ID
        assert!(update_endpoint(
            &mut config,
            "sms-endpoint",
            SmsConfigUpdater {
                provider: Some(SmsProvider::Tencent),
                ..Default::default()
            },
            Default::default(),
            None,
            None,
        )
        .is_err());

        update_endpoint(
            &mut config,
            "sms-endpoint",
            SmsConfigUpdater {
                provider: Some(SmsProvider::Tencent),
                app_id: Some("1400000000".into()),
                phone: Some(vec!["13800000000".into(), "13900000000".into()]),
                ..Default::default()
            },
            SmsPrivateConfigUpdater {
                secret_key: Some("newsecret".into()),
                ..Default::default()
            },
            Some(&[DeleteableSmsProperty::TemplateParam]),
            None,
        )?;

        let endpoint = get_endpoint(&config, "sms-endpoint")?;
        assert_eq!(endpoint.provider, SmsProvider::Tencent);
        assert_eq!(endpoint.phone.len(), 2);
        assert!(endpoint.template_param.is_empty());

        let private_config = get_private_config(&config, "sms-endpoint")?;
        assert_eq!(private_config.access_key, "key");
        assert_eq!(private_config.secret_key, "newsecret");

        Ok(())
    }

    #[test]
    fn test_sms_config_roundtrip() -> Result<(), HttpError> {
        let mut config = empty_config();
        add_default_endpoint(&mut config)?;

        let (public, private) = config.write().unwrap();
        assert!(public.contains("sms: sms-endpoint"));
        assert!(public.contains("\ttemplate-param hostname\n\ttemplate-param title\n"));
        assert!(!public.contains("secret"));
        assert!(private.contains("secret-key secret"));

        let config = Config::new(&public, &private).unwrap();
        let endpoint = get_endpoint(&config, "sms-endpoint")?;
        assert_eq!(
            endpoint.template_param,
            vec![SmsTemplateParam::Hostname, SmsTemplateParam::Title]
        );

        Ok(())
    }

    #[test]
    fn test_sms_endpoint_delete() -> Result<(), HttpError> {
        let mut config = empty_config();
        add_default_endpoint(&mut config)?;

        delete_sms_endpoint(&mut config, "sms-endpoint")?;
        assert!(delete_sms_endpoint(&mut config, "sms-endpoint").is_err());
        assert_eq!(get_endpoints(&config)?.len(), 0);
        assert!(get_private_config(&config, "sms-endpoint").is_err());

        Ok(())
    }
}
