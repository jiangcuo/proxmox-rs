use proxmox_http_error::HttpError;

use crate::api::http_err;
use crate::endpoints::dingtalk::{
    DeleteableDingtalkProperty, DingtalkConfig, DingtalkConfigUpdater, DingtalkPrivateConfig,
    DingtalkPrivateConfigUpdater, DINGTALK_TYPENAME,
};
use crate::Config;

/// Get a list of all DingTalk endpoints.
///
/// The caller is responsible for any needed permission checks.
/// Returns a list of all DingTalk endpoints or a `HttpError` if the config is
/// erroneous (`500 Internal server error`).
pub fn get_endpoints(config: &Config) -> Result<Vec<DingtalkConfig>, HttpError> {
    config
        .config
        .convert_to_typed_array(DINGTALK_TYPENAME)
        .map_err(|e| http_err!(NOT_FOUND, "Could not fetch endpoints: {e}"))
}

/// Get DingTalk endpoint with given `name`
///
/// The caller is responsible for any needed permission checks.
/// Returns the endpoint or a `HttpError` if the endpoint was not found (`404 Not found`).
pub fn get_endpoint(config: &Config, name: &str) -> Result<DingtalkConfig, HttpError> {
    config
        .config
        .lookup(DINGTALK_TYPENAME, name)
        .map_err(|_| http_err!(NOT_FOUND, "endpoint '{name}' not found"))
}

/// Add a new DingTalk endpoint.
///
/// The caller is responsible for any needed permission checks.
/// The caller also responsible for locking the configuration files.
/// Returns a `HttpError` if:
///   - an entity with the same name already exists (`400 Bad request`)
///   - the configuration could not be saved (`500 Internal server error`)
///
/// Panics if the names of the private config and the public config do not match.
pub fn add_endpoint(
    config: &mut Config,
    endpoint_config: DingtalkConfig,
    private_endpoint_config: DingtalkPrivateConfig,
) -> Result<(), HttpError> {
    if endpoint_config.name != private_endpoint_config.name {
        // Programming error by the user of the crate, thus we panic
        panic!("name for endpoint config and private config must be identical");
    }

    super::ensure_unique(config, &endpoint_config.name)?;

    set_private_config_entry(config, &private_endpoint_config)?;

    config
        .config
        .set_data(&endpoint_config.name, DINGTALK_TYPENAME, &endpoint_config)
        .map_err(|e| {
            http_err!(
                INTERNAL_SERVER_ERROR,
                "could not save endpoint '{}': {e}",
                endpoint_config.name
            )
        })
}

/// Update existing DingTalk endpoint
///
/// The caller is responsible for any needed permission checks.
/// The caller also responsible for locking the configuration files.
/// Returns a `HttpError` if:
///   - the endpoint does not exist (`404 Not found`)
///   - the configuration could not be saved (`500 Internal server error`)
pub fn update_endpoint(
    config: &mut Config,
    name: &str,
    endpoint_config_updater: DingtalkConfigUpdater,
    private_endpoint_config_updater: DingtalkPrivateConfigUpdater,
    delete: Option<&[DeleteableDingtalkProperty]>,
    digest: Option<&[u8]>,
) -> Result<(), HttpError> {
    super::verify_digest(config, digest)?;

    let mut endpoint = get_endpoint(config, name)?;
    let mut private_endpoint = get_private_config(config, name)?;

    if let Some(delete) = delete {
        for deletable_property in delete {
            match deletable_property {
                DeleteableDingtalkProperty::Comment => endpoint.comment = None,
                DeleteableDingtalkProperty::Disable => endpoint.disable = None,
                DeleteableDingtalkProperty::Secret => private_endpoint.secret = None,
            }
        }
    }

    if let Some(url) = private_endpoint_config_updater.url {
        private_endpoint.url = url;
    }

    if let Some(secret) = private_endpoint_config_updater.secret {
        private_endpoint.secret = Some(secret);
    }

    if let Some(comment) = endpoint_config_updater.comment {
        endpoint.comment = Some(comment)
    }

    if let Some(disable) = endpoint_config_updater.disable {
        endpoint.disable = Some(disable);
    }

    set_private_config_entry(config, &private_endpoint)?;

    config
        .config
        .set_data(name, DINGTALK_TYPENAME, &endpoint)
        .map_err(|e| {
            http_err!(
                INTERNAL_SERVER_ERROR,
                "could not save endpoint '{name}': {e}"
            )
        })
}

/// Delete existing DingTalk endpoint
///
/// The caller is responsible for any needed permission checks.
/// The caller also responsible for locking the configuration files.
/// Returns a `HttpError` if:
///   - the entity does not exist (`404 Not found`)
///   - the endpoint is still referenced by another entity (`400 Bad request`)
pub fn delete_dingtalk_endpoint(config: &mut Config, name: &str) -> Result<(), HttpError> {
    // Check if the endpoint exists
    let _ = get_endpoint(config, name)?;
    super::ensure_safe_to_delete(config, name)?;

    remove_private_config_entry(config, name)?;
    config.config.sections.remove(name);

    Ok(())
}

fn get_private_config(config: &Config, name: &str) -> Result<DingtalkPrivateConfig, HttpError> {
    config
        .private_config
        .lookup(DINGTALK_TYPENAME, name)
        .map_err(|_| http_err!(NOT_FOUND, "private config for endpoint '{name}' not found"))
}

fn set_private_config_entry(
    config: &mut Config,
    private_config: &DingtalkPrivateConfig,
) -> Result<(), HttpError> {
    config
        .private_config
        .set_data(&private_config.name, DINGTALK_TYPENAME, private_config)
        .map_err(|e| {
            http_err!(
                INTERNAL_SERVER_ERROR,
                "could not save private config for endpoint '{}': {e}",
                private_config.name
            )
        })
}

fn remove_private_config_entry(config: &mut Config, name: &str) -> Result<(), HttpError> {
    config.private_config.sections.remove(name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::empty_config;

    fn add_default_endpoint(config: &mut Config) -> Result<(), HttpError> {
        add_endpoint(
            config,
            DingtalkConfig {
                name: "dingtalk-endpoint".into(),
                comment: Some("comment".into()),
                ..Default::default()
            },
            DingtalkPrivateConfig {
                name: "dingtalk-endpoint".into(),
                url: "https://example.com/hook".into(),
                secret: Some("secret".into()),
            },
        )?;

        assert!(get_endpoint(config, "dingtalk-endpoint").is_ok());
        Ok(())
    }

    #[test]
    fn test_update_not_existing_returns_error() {
        let mut config = empty_config();

        assert!(update_endpoint(
            &mut config,
            "test",
            Default::default(),
            Default::default(),
            None,
            None
        )
        .is_err());
    }

    #[test]
    fn test_dingtalk_update() -> Result<(), HttpError> {
        let mut config = empty_config();
        add_default_endpoint(&mut config)?;

        let digest = config.digest;

        update_endpoint(
            &mut config,
            "dingtalk-endpoint",
            DingtalkConfigUpdater {
                comment: Some("newcomment".into()),
                ..Default::default()
            },
            DingtalkPrivateConfigUpdater {
                url: Some("https://example.com/new".into()),
                ..Default::default()
            },
            None,
            Some(&digest),
        )?;

        let endpoint = get_endpoint(&config, "dingtalk-endpoint")?;
        assert_eq!(endpoint.comment, Some("newcomment".to_string()));

        let private_config = get_private_config(&config, "dingtalk-endpoint")?;
        assert_eq!(private_config.url, "https://example.com/new");
        assert_eq!(private_config.secret.as_deref(), Some("secret"));

        update_endpoint(
            &mut config,
            "dingtalk-endpoint",
            Default::default(),
            Default::default(),
            Some(&[
                DeleteableDingtalkProperty::Comment,
                DeleteableDingtalkProperty::Secret,
            ]),
            None,
        )?;

        let endpoint = get_endpoint(&config, "dingtalk-endpoint")?;
        assert_eq!(endpoint.comment, None);
        assert_eq!(
            get_private_config(&config, "dingtalk-endpoint")?.secret,
            None
        );

        Ok(())
    }

    #[test]
    fn test_dingtalk_endpoint_delete() -> Result<(), HttpError> {
        let mut config = empty_config();
        add_default_endpoint(&mut config)?;

        delete_dingtalk_endpoint(&mut config, "dingtalk-endpoint")?;
        assert!(delete_dingtalk_endpoint(&mut config, "dingtalk-endpoint").is_err());
        assert_eq!(get_endpoints(&config)?.len(), 0);
        assert!(get_private_config(&config, "dingtalk-endpoint").is_err());

        Ok(())
    }
}
