#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use std::path::Path;

use anyhow::{format_err, Error};
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod http_client;
pub use http_client::http_client;

mod auth_state;
pub use auth_state::*;

use openidconnect::{
    //curl::http_client,
    core::{
        CoreAuthDisplay, CoreAuthPrompt, CoreAuthenticationFlow, CoreErrorResponseType,
        CoreGenderClaim, CoreIdTokenVerifier, CoreJsonWebKey, CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm, CoreProviderMetadata, CoreRevocableToken,
        CoreRevocationErrorResponse, CoreTokenIntrospectionResponse, CoreTokenType,
    },
    AdditionalClaims,
    AuthenticationContextClass,
    AuthorizationCode,
    ClientId,
    ClientSecret,
    CsrfToken,
    EmptyExtraTokenFields,
    EndpointMaybeSet,
    EndpointNotSet,
    EndpointSet,
    IdTokenClaims,
    IdTokenFields,
    IssuerUrl,
    Nonce,
    OAuth2TokenResponse,
    PkceCodeChallenge,
    PkceCodeVerifier,
    RedirectUrl,
    Scope,
    StandardClaims,
    StandardErrorResponse,
    StandardTokenResponse,
    UserInfoClaims,
};

/// Stores Additional Claims into a serde_json::Value;
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct GenericClaims(Value);
impl AdditionalClaims for GenericClaims {}

pub type GenericUserInfoClaims = UserInfoClaims<GenericClaims, CoreGenderClaim>;
pub type GenericIdTokenClaims = IdTokenClaims<GenericClaims, CoreGenderClaim>;

pub type GenericIdTokenFields = IdTokenFields<
    GenericClaims,
    EmptyExtraTokenFields,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;

pub type GenericTokenResponse = StandardTokenResponse<GenericIdTokenFields, CoreTokenType>;

pub type GenericClient = openidconnect::Client<
    GenericClaims,
    CoreAuthDisplay,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJsonWebKey,
    CoreAuthPrompt,
    StandardErrorResponse<CoreErrorResponseType>,
    GenericTokenResponse,
    CoreTokenIntrospectionResponse,
    CoreRevocableToken,
    CoreRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OpenIdConfig {
    pub issuer_url: String,
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acr_values: Option<Vec<String>>,
}

pub struct OpenIdAuthenticator {
    client: GenericClient,
    config: OpenIdConfig,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PublicAuthState {
    pub csrf_token: CsrfToken,
    pub realm: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PrivateAuthState {
    pub csrf_token: CsrfToken,
    pub nonce: Nonce,
    pub pkce_verifier: PkceCodeVerifier,
    pub ctime: i64,
}

impl Default for PrivateAuthState {
    fn default() -> Self {
        Self::new()
    }
}

impl PrivateAuthState {
    pub fn new() -> Self {
        let nonce = Nonce::new_random();
        let csrf_token = CsrfToken::new_random();
        let (_pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        PrivateAuthState {
            csrf_token,
            nonce,
            pkce_verifier,
            ctime: proxmox_time::epoch_i64(),
        }
    }

    pub fn pkce_verifier(&self) -> PkceCodeVerifier {
        // Note: PkceCodeVerifier does not impl. clone()
        PkceCodeVerifier::new(self.pkce_verifier.secret().to_string())
    }

    pub fn pkce_challenge(&self) -> PkceCodeChallenge {
        PkceCodeChallenge::from_code_verifier_sha256(&self.pkce_verifier)
    }

    pub fn public_state_string(&self, realm: String) -> Result<String, Error> {
        let pub_state = PublicAuthState {
            csrf_token: self.csrf_token.clone(),
            realm,
        };
        Ok(serde_json::to_string(&pub_state)?)
    }
}

impl OpenIdAuthenticator {
    pub fn discover(config: &OpenIdConfig, redirect_url: &str) -> Result<Self, Error> {
        let client_id = ClientId::new(config.client_id.clone());
        let client_key = config.client_key.clone().map(ClientSecret::new);
        let issuer_url = IssuerUrl::new(config.issuer_url.clone())?;

        let provider_metadata = CoreProviderMetadata::discover(&issuer_url, &http_client)?;

        let client =
            GenericClient::from_provider_metadata(provider_metadata, client_id, client_key)
                .set_redirect_uri(RedirectUrl::new(String::from(redirect_url))?);

        Ok(Self {
            client,
            config: config.clone(),
        })
    }

    pub fn authorize_url(&self, state_dir: &str, realm: &str) -> Result<String, Error> {
        let private_auth_state = PrivateAuthState::new();
        let public_auth_state = private_auth_state.public_state_string(realm.to_string())?;
        let nonce = private_auth_state.nonce.clone();

        store_auth_state(Path::new(state_dir), realm, &private_auth_state)?;

        // Generate the authorization URL to which we'll redirect the user.
        let mut request = self
            .client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                || CsrfToken::new(public_auth_state),
                || nonce,
            )
            .set_pkce_challenge(private_auth_state.pkce_challenge());

        request = request.set_display(CoreAuthDisplay::Page);

        match self.config.prompt.as_deref() {
            None => { /* nothing */ }
            Some("none") => {
                request = request.add_prompt(CoreAuthPrompt::None);
            }
            Some("login") => {
                request = request.add_prompt(CoreAuthPrompt::Login);
            }
            Some("consent") => {
                request = request.add_prompt(CoreAuthPrompt::Consent);
            }
            Some("select_account") => {
                request = request.add_prompt(CoreAuthPrompt::SelectAccount);
            }
            Some(extension) => {
                request = request.add_prompt(CoreAuthPrompt::Extension(extension.into()));
            }
        }

        if let Some(ref scopes) = self.config.scopes {
            for scope in scopes.clone() {
                request = request.add_scope(Scope::new(scope));
            }
        }

        if let Some(ref acr_values) = self.config.acr_values {
            for acr in acr_values.clone() {
                request = request.add_auth_context_value(AuthenticationContextClass::new(acr));
            }
        }

        let (authorize_url, _csrf_state, _nonce) = request.url();

        Ok(authorize_url.to_string())
    }

    pub fn verify_public_auth_state(
        state_dir: &str,
        state: &str,
    ) -> Result<(String, PrivateAuthState), Error> {
        verify_public_auth_state(Path::new(state_dir), state)
    }

    pub fn verify_authorization_code(
        &self,
        code: &str,
        private_auth_state: &PrivateAuthState,
    ) -> Result<(GenericIdTokenClaims, GenericUserInfoClaims), Error> {
        self.verify_authorization_code_userinfo(code, private_auth_state, true)
    }

    pub fn verify_authorization_code_userinfo(
        &self,
        code: &str,
        private_auth_state: &PrivateAuthState,
        query_userinfo: bool,
    ) -> Result<(GenericIdTokenClaims, GenericUserInfoClaims), Error> {
        let code = AuthorizationCode::new(code.to_string());
        // Exchange the code with a token.
        let token_response = self
            .client
            .exchange_code(code)
            .map_err(|err| format_err!("Configuration error for token endpoint: {}", err))?
            .set_pkce_verifier(private_auth_state.pkce_verifier())
            .request(&http_client)
            .map_err(|err| format_err!("Failed to contact token endpoint: {}", err))?;

        let id_token_verifier: CoreIdTokenVerifier = self.client.id_token_verifier();
        let id_token_claims: &GenericIdTokenClaims = token_response
            .extra_fields()
            .id_token()
            .expect("Server did not return an ID token")
            .claims(&id_token_verifier, &private_auth_state.nonce)
            .map_err(|err| format_err!("Failed to verify ID token: {}", err))?;

        if !query_userinfo {
            let empty_userinfo_claims = UserInfoClaims::new(
                StandardClaims::new(id_token_claims.subject().clone()),
                GenericClaims(Value::Null),
            );
            return Ok((id_token_claims.clone(), empty_userinfo_claims));
        }

        let userinfo_claims: GenericUserInfoClaims = self
            .client
            .user_info(token_response.access_token().to_owned(), None)?
            .request(&http_client)
            .map_err(|err| format_err!("Failed to contact userinfo endpoint: {}", err))?;

        Ok((id_token_claims.clone(), userinfo_claims))
    }

    /// Like verify_authorization_code(), but returns claims as serde_json::Value
    pub fn verify_authorization_code_simple(
        &self,
        code: &str,
        private_auth_state: &PrivateAuthState,
    ) -> Result<Value, Error> {
        self.verify_authorization_code_simple_userinfo(code, private_auth_state, true)
    }

    /// Like verify_authorization_code_simple_userinfo(), but returns claims as serde_json::Value
    pub fn verify_authorization_code_simple_userinfo(
        &self,
        code: &str,
        private_auth_state: &PrivateAuthState,
        query_userinfo: bool,
    ) -> Result<Value, Error> {
        let (id_token_claims, userinfo_claims) =
            self.verify_authorization_code_userinfo(code, private_auth_state, query_userinfo)?;

        let mut data = serde_json::to_value(id_token_claims)?;

        let data2 = serde_json::to_value(userinfo_claims)?;

        if let Some(map) = data2.as_object() {
            for (key, value) in map {
                if data[key] != Value::Null {
                    continue; // already set
                }
                data[key] = value.clone();
            }
        }

        Ok(data)
    }
}
