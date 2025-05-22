use std::error::Error as StdError;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use http::request::Request;
use http::uri::PathAndQuery;
use http::Method;
use http::{StatusCode, Uri};
use http_body_util::BodyExt;
use openssl::hash::MessageDigest;
use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};
use openssl::x509::{self, X509};
use proxmox_login::Ticket;
use serde::Serialize;

use proxmox_http::Body;
use proxmox_login::ticket::Validity;
use proxmox_login::{Login, SecondFactorChallenge, TicketResult};

use crate::auth::AuthenticationKind;
use crate::error::ParseFingerprintError;
use crate::{Error, Token};

use super::{HttpApiClient, HttpApiResponse, HttpApiResponseStream};

/// See [`set_verify_callback`](openssl::ssl::SslContextBuilder::set_verify_callback()).
pub type TlsCallback = dyn Fn(bool, &mut x509::X509StoreContextRef) -> bool + Send + Sync + 'static;
#[derive(Default)]
pub enum TlsOptions {
    /// Default TLS verification.
    #[default]
    Verify,

    /// Insecure: ignore invalid certificates.
    Insecure,

    /// Expect a specific certificate fingerprint.
    Fingerprint(Vec<u8>),

    /// Verify with a specific PEM formatted CA.
    CaCert(X509),

    /// Use a callback for certificate verification.
    Callback(Box<TlsCallback>),
}

impl TlsOptions {
    pub fn parse_fingerprint(fp: &str) -> Result<Self, ParseFingerprintError> {
        use hex::FromHex;

        let hex: Vec<u8> = fp
            .as_bytes()
            .iter()
            .copied()
            .filter(|&b| b != b':')
            .collect();

        let fp = <[u8; 32]>::from_hex(hex).map_err(|_| ParseFingerprintError)?;

        Ok(Self::Fingerprint(fp.into()))
    }
}

/// A Proxmox API client base backed by a [`proxmox_http::client::Client`].
pub struct Client {
    api_url: Uri,
    auth: Mutex<Option<Arc<AuthenticationKind>>>,
    client: Arc<proxmox_http::client::Client>,
    pve_compat: bool,
    cookie_name: Option<String>,
}

impl Client {
    /// Create a new client instance which will connect to the provided endpoint.
    pub fn new(api_url: Uri) -> Self {
        Client::with_client(api_url, Arc::new(proxmox_http::client::Client::new()))
    }

    pub fn new_with_cookie(api_url: Uri, cookie_name: &str) -> Self {
        let mut client =
            Client::with_client(api_url, Arc::new(proxmox_http::client::Client::new()));
        client.set_cookie_name(cookie_name);
        client
    }

    /// Instantiate a client for an API with a given HTTP client instance.
    pub fn with_client(api_url: Uri, client: Arc<proxmox_http::client::Client>) -> Self {
        Self {
            api_url,
            auth: Mutex::new(None),
            client,
            pve_compat: false,
            cookie_name: None,
        }
    }

    /// Create a new client instance which will connect to the provided endpoint.
    pub fn with_options(
        api_url: Uri,
        tls_options: TlsOptions,
        http_options: proxmox_http::HttpOptions,
    ) -> Result<Self, Error> {
        let mut connector = SslConnector::builder(SslMethod::tls_client())
            .map_err(|err| Error::internal("failed to create ssl connector builder", err))?;

        match tls_options {
            TlsOptions::Verify => (),
            TlsOptions::Insecure => connector.set_verify(SslVerifyMode::NONE),
            TlsOptions::Fingerprint(expected_fingerprint) => {
                connector.set_verify_callback(SslVerifyMode::PEER, move |valid, chain| {
                    if valid {
                        return true;
                    }
                    verify_fingerprint(chain, &expected_fingerprint)
                });
            }
            TlsOptions::Callback(cb) => {
                connector
                    .set_verify_callback(SslVerifyMode::PEER, move |valid, chain| cb(valid, chain));
            }
            TlsOptions::CaCert(ca) => {
                let mut store = openssl::x509::store::X509StoreBuilder::new().map_err(|err| {
                    Error::internal("failed to create certificate store builder", err)
                })?;
                store
                    .add_cert(ca)
                    .map_err(|err| Error::internal("failed to build certificate store", err))?;
                connector.set_cert_store(store.build());
            }
        }

        let client =
            proxmox_http::client::Client::with_ssl_connector(connector.build(), http_options);

        Ok(Self::with_client(api_url, Arc::new(client)))
    }

    /// Get the underlying client object.
    pub fn http_client(&self) -> &Arc<proxmox_http::client::Client> {
        &self.client
    }

    /// Get a reference to the current authentication information.
    pub fn authentication(&self) -> Option<Arc<AuthenticationKind>> {
        self.auth.lock().unwrap().clone()
    }

    /// Get a serialized version of the ticket if one is used.
    ///
    /// This returns `None` when using an API token and `Error::Unauthorized` if not logged in.
    pub fn serialize_ticket(&self) -> Result<Option<Vec<u8>>, Error> {
        let auth = self.authentication().ok_or(Error::Unauthorized)?;
        let auth = match &*auth {
            AuthenticationKind::Token(_) => return Ok(None),
            AuthenticationKind::Ticket(auth) => auth,
        };
        Ok(Some(serde_json::to_vec(auth).map_err(|err| {
            Error::internal("failed to serialize ticket", err)
        })?))
    }

    #[deprecated(note = "use set_authentication instead")]
    /// Replace the authentication information with an API token.
    pub fn use_api_token(&self, token: Token) {
        self.set_authentication(token);
    }

    /// Replace the currently used authentication.
    ///
    /// This can be a `Token` or an [`Authentication`](proxmox_login::Authentication).
    pub fn set_authentication(&self, auth: impl Into<AuthenticationKind>) {
        *self.auth.lock().unwrap() = Some(Arc::new(auth.into()));
    }

    /// Drop the current authentication information.
    pub fn logout(&self) {
        self.auth.lock().unwrap().take();
    }

    /// Enable Proxmox VE login API compatibility. This is required to support TFA authentication
    /// on Proxmox VE APIs which require the `new-format` option.
    pub fn set_pve_compatibility(&mut self, compatibility: bool) {
        self.pve_compat = compatibility;
    }

    pub fn set_cookie_name(&mut self, cookie_name: &str) {
        self.cookie_name = Some(cookie_name.to_string());
    }

    /// Get the currently used API url.
    pub fn api_url(&self) -> &Uri {
        &self.api_url
    }

    /// Build a URI relative to the current API endpoint.
    fn build_uri(&self, path_and_query: &str) -> Result<Uri, Error> {
        let parts = self.api_url.clone().into_parts();
        let mut builder = http::uri::Builder::new();
        if let Some(scheme) = parts.scheme {
            builder = builder.scheme(scheme);
        }
        if let Some(authority) = parts.authority {
            builder = builder.authority(authority)
        }
        builder
            .path_and_query(
                path_and_query
                    .parse::<PathAndQuery>()
                    .map_err(|err| Error::internal("failed to parse uri", err))?,
            )
            .build()
            .map_err(|err| Error::internal("failed to build Uri", err))
    }

    /// Perform an *unauthenticated* HTTP request.
    async fn send_authenticated_request(
        client: Arc<proxmox_http::client::Client>,
        auth: Arc<AuthenticationKind>,
        method: Method,
        uri: Uri,
        json_body: Option<String>,
        // send an `Accept: application/json-seq` header.
        streaming: bool,
        cookie_name: &Option<String>,
    ) -> Result<(http::response::Parts, Body), Error> {
        let mut request = auth.set_auth_headers_with_cookie_name(
            Request::builder().method(method).uri(uri),
            cookie_name,
        );
        if streaming {
            request = request.header(http::header::ACCEPT, "application/json-seq");
        }

        let request = if let Some(body) = json_body {
            request
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(body.into())
        } else {
            request.body(Body::empty())
        }
        .map_err(|err| Error::internal("failed to build request", err))?;

        let response = client
            .request(request)
            .await
            .map_err(|err| Error::Client(err.into()))?;

        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(Error::Unauthorized);
        }

        let (response, body) = response.into_parts();

        if !response.status.is_success() {
            let body = read_body(body).await?;
            // FIXME: Decode json errors...
            //match serde_json::from_slice(&data)
            //    Ok(value) =>
            //        if value["error"]
            let data =
                String::from_utf8(body).map_err(|_| Error::Other("API returned non-utf8 data"))?;

            return Err(Error::api(response.status, data));
        }

        Ok((response, body))
    }

    /// Perform an *unauthenticated* HTTP request.
    async fn authenticated_request(
        client: Arc<proxmox_http::client::Client>,
        auth: Arc<AuthenticationKind>,
        method: Method,
        uri: Uri,
        json_body: Option<String>,
        cookie_name: &Option<String>,
    ) -> Result<HttpApiResponse, Error> {
        let (response, body) = Self::send_authenticated_request(
            client,
            auth,
            method,
            uri,
            json_body,
            false,
            cookie_name,
        )
        .await?;
        let body = read_body(body).await?;

        let content_type = match response.headers.get(http::header::CONTENT_TYPE) {
            None => None,
            Some(value) => Some(
                value
                    .to_str()
                    .map_err(|err| Error::internal("bad Content-Type header", err))?
                    .to_owned(),
            ),
        };

        Ok(HttpApiResponse {
            status: response.status.as_u16(),
            content_type,
            body,
        })
    }

    /// Assert that we are authenticated and return the `AuthenticationKind`.
    /// Otherwise returns `Error::Unauthorized`.
    pub fn login_auth(&self) -> Result<Arc<AuthenticationKind>, Error> {
        self.auth
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Error::Unauthorized)
    }

    /// Check to see if we need to refresh the ticket. Note that it is an error to call this when
    /// logged out, which will return `Error::Unauthorized`.
    ///
    /// Tokens are always valid.
    pub fn ticket_validity(&self) -> Result<Validity, Error> {
        match &*self.login_auth()? {
            AuthenticationKind::Token(_) => Ok(Validity::Valid),
            AuthenticationKind::Ticket(auth) => Ok(auth.ticket.validity()),
        }
    }

    /// If the ticket expires soon (has a validity of [`Validity::Refresh`]), this will attempt to
    /// refresh the ticket.
    pub async fn maybe_refresh_ticket(&self) -> Result<(), Error> {
        if let Validity::Refresh = self.ticket_validity()? {
            self.refresh_ticket().await?;
        }

        Ok(())
    }

    async fn do_login_request(
        &self,
        request: proxmox_login::Request,
    ) -> Result<(Option<Ticket>, Vec<u8>), Error> {
        let request = http::Request::builder()
            .method(Method::POST)
            .uri(request.url)
            .header(http::header::CONTENT_TYPE, request.content_type)
            .header(
                http::header::CONTENT_LENGTH,
                request.content_length.to_string(),
            )
            .body(request.body.into())
            .map_err(|err| Error::internal("error building login http request", err))?;

        let api_response = self
            .client
            .request(request)
            .await
            .map_err(|err| Error::Client(err.into()))?;
        if !api_response.status().is_success() {
            return Err(Error::api(api_response.status(), "authentication failed"));
        }

        let (parts, body) = api_response.into_parts();
        let body = read_body(body).await?;

        let ticket: Option<Ticket> = self.cookie_name.as_ref().and_then(|cookie_name| {
            parts
                .headers
                .get_all(http::header::SET_COOKIE)
                .iter()
                .filter_map(|c| c.to_str().ok())
                .filter_map(|c| match (c.find('='), c.find(';')) {
                    (Some(begin), Some(end)) if begin < end && &c[..begin] == cookie_name => {
                        Some(&c[begin + 1..end])
                    }
                    _ => None,
                })
                .filter_map(|t| t.parse().ok())
                .next()
        });

        Ok((ticket, body))
    }

    /// Attempt to refresh the current ticket.
    ///
    /// If not logged in at all yet, `Error::Unauthorized` will be returned.
    pub async fn refresh_ticket(&self) -> Result<(), Error> {
        let auth = self.login_auth()?;
        let auth = match &*auth {
            AuthenticationKind::Token(_) => return Ok(()),
            AuthenticationKind::Ticket(auth) => auth,
        };

        let login = Login::renew(self.api_url.to_string(), auth.ticket.to_string())
            .map_err(Error::Ticket)?;

        let (ticket, api_response) = self.do_login_request(login.request()).await?;

        match login.response_with_cookie_ticket(ticket, &api_response)? {
            TicketResult::Full(auth) | TicketResult::HttpOnly(auth) => {
                *self.auth.lock().unwrap() = Some(Arc::new(auth.into()));
                Ok(())
            }
            TicketResult::TfaRequired(_) => Err(proxmox_login::error::ResponseError::Msg(
                "ticket refresh returned a TFA challenge",
            )
            .into()),
        }
    }

    /// Attempt to login.
    ///
    /// This will propagate the PVE compatibility state and then perform the `Login` request via
    /// the inner http client.
    ///
    /// If the authentication is complete, `None` is returned and the authentication state updated.
    /// If a 2nd factor is required, `Some` is returned.
    pub async fn login(&self, login: Login) -> Result<Option<SecondFactorChallenge>, Error> {
        let login = login.pve_compatibility(self.pve_compat);

        let (ticket, api_response) = self.do_login_request(login.request()).await?;

        Ok(
            match login.response_with_cookie_ticket(ticket, &api_response)? {
                TicketResult::TfaRequired(challenge) => Some(challenge),
                TicketResult::Full(auth) | TicketResult::HttpOnly(auth) => {
                    *self.auth.lock().unwrap() = Some(Arc::new(auth.into()));
                    None
                }
            },
        )
    }

    /// Attempt to finish a 2nd factor login.
    ///
    /// This will propagate the PVE compatibility state and then perform the `Login` request via
    /// the inner http client.
    pub async fn login_tfa(
        &self,
        challenge: SecondFactorChallenge,
        challenge_response: proxmox_login::Request,
    ) -> Result<(), Error> {
        let (ticket, api_response) = self.do_login_request(challenge_response).await?;

        let auth = challenge.response_with_cookie_ticket(ticket, &api_response)?;
        *self.auth.lock().unwrap() = Some(Arc::new(auth.into()));
        Ok(())
    }
}

async fn read_body(body: Body) -> Result<Vec<u8>, Error> {
    let data = body
        .collect()
        .await
        .map_err(Error::Anyhow)?
        .to_bytes()
        .to_vec();
    Ok(data)
}

impl HttpApiClient for Client {
    type ResponseFuture<'a> =
        Pin<Box<dyn Future<Output = Result<HttpApiResponse, Error>> + Send + 'a>>;

    type ResponseStreamFuture<'a> =
        Pin<Box<dyn Future<Output = Result<HttpApiResponseStream<Self::Body>, Error>> + Send + 'a>>;

    type Body = Body;

    fn request<'a, T>(
        &'a self,
        method: Method,
        path_and_query: &'a str,
        params: Option<T>,
    ) -> Self::ResponseFuture<'a>
    where
        T: Serialize + 'a,
    {
        let params = params
            .map(|params| {
                serde_json::to_string(&params)
                    .map_err(|err| Error::internal("failed to serialize parameters", err))
            })
            .transpose();

        Box::pin(async move {
            let params = params?;
            let auth = self.login_auth()?;
            let uri = self.build_uri(path_and_query)?;
            let client = Arc::clone(&self.client);
            Self::authenticated_request(client, auth, method, uri, params, &self.cookie_name).await
        })
    }

    fn streaming_request<'a, T>(
        &'a self,
        method: Method,
        path_and_query: &'a str,
        params: Option<T>,
    ) -> Self::ResponseStreamFuture<'a>
    where
        T: Serialize + 'a,
    {
        let params = params
            .map(|params| {
                serde_json::to_string(&params)
                    .map_err(|err| Error::internal("failed to serialize parameters", err))
            })
            .transpose();

        Box::pin(async move {
            let params = params?;
            let auth = self.login_auth()?;
            let uri = self.build_uri(path_and_query)?;
            let client = Arc::clone(&self.client);
            let (response, body) = Self::send_authenticated_request(
                client,
                auth,
                method,
                uri,
                params,
                true,
                &self.cookie_name,
            )
            .await?;

            let content_type = match response.headers.get(http::header::CONTENT_TYPE) {
                None => None,
                Some(value) => Some(
                    value
                        .to_str()
                        .map_err(|err| Error::internal("bad Content-Type header", err))?
                        .to_owned(),
                ),
            };

            Ok(HttpApiResponseStream {
                status: response.status.as_u16(),
                content_type,
                body: Some(body),
            })
        })
    }
}

fn verify_fingerprint(chain: &x509::X509StoreContextRef, expected_fingerprint: &[u8]) -> bool {
    let Some(cert) = chain.current_cert() else {
        log::error!("no certificate in chain?");
        return false;
    };

    let fp = match cert.digest(MessageDigest::sha256()) {
        Err(err) => {
            log::error!("error calculating certificate fingerprint: {err}");
            return false;
        }
        Ok(fp) => fp,
    };

    if expected_fingerprint != fp.as_ref() {
        log::error!("bad fingerprint: {}", fp_string(&fp));
        log::error!("expected fingerprint: {}", fp_string(expected_fingerprint));
        return false;
    }

    true
}

fn fp_string(fp: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    for b in fp {
        if !out.is_empty() {
            out.push(':');
        }
        let _ = write!(out, "{b:02x}");
    }
    out
}

impl Error {
    pub(crate) fn internal<E>(context: &'static str, err: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        Self::Internal(context, Box::new(err))
    }
}

impl AuthenticationKind {
    pub fn set_auth_headers(&self, request: http::request::Builder) -> http::request::Builder {
        match self {
            AuthenticationKind::Ticket(auth) => auth.set_auth_headers(request),
            AuthenticationKind::Token(auth) => auth.set_auth_headers(request),
        }
    }

    pub fn set_auth_headers_with_cookie_name(
        &self,
        request: http::request::Builder,
        cookie_name: &Option<String>,
    ) -> http::request::Builder {
        match self {
            AuthenticationKind::Ticket(auth) => {
                if let Some(name) = cookie_name {
                    auth.set_auth_headers_with_cookie_name(request, name)
                } else {
                    auth.set_auth_headers(request)
                }
            }
            AuthenticationKind::Token(auth) => auth.set_auth_headers(request),
        }
    }

    pub fn userid(&self) -> &str {
        match self {
            AuthenticationKind::Ticket(auth) => &auth.userid,
            AuthenticationKind::Token(auth) => &auth.userid,
        }
    }
}
