//! Plugin type definitions.

use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, format_err, Error};
use futures::TryFutureExt;
use http::{Request, Response};
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use std::net::{IpAddr, SocketAddr};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::Command;

use proxmox_acme::async_client::AcmeClient;
use proxmox_acme::{Authorization, Challenge};
use proxmox_rest_server::WorkerTask;

use crate::plugin_config::PluginData;
use crate::types::{AcmeDomain, DnsPlugin};

const PROXMOX_ACME_SH_PATH: &str = "/usr/share/proxmox-acme/proxmox-acme";

pub(crate) fn get_acme_plugin(
    plugin_data: &PluginData,
    name: &str,
) -> Result<Option<Box<dyn AcmePlugin + Send + Sync + 'static>>, Error> {
    let (ty, data) = match plugin_data.get(name) {
        Some(plugin) => plugin,
        None => return Ok(None),
    };

    Ok(Some(match ty.as_str() {
        "dns" => {
            let plugin: DnsPlugin = serde::Deserialize::deserialize(data)?;
            Box::new(plugin)
        }
        "standalone" => {
            // this one has no config
            Box::<StandaloneServer>::default()
        }
        other => bail!("missing implementation for plugin type '{}'", other),
    }))
}

pub(crate) trait AcmePlugin {
    /// Setup everything required to trigger the validation and return the corresponding validation
    /// URL.
    fn setup<'fut, 'a: 'fut, 'b: 'fut, 'c: 'fut, 'd: 'fut>(
        &'a mut self,
        client: &'b mut AcmeClient,
        authorization: &'c Authorization,
        domain: &'d AcmeDomain,
        task: Arc<WorkerTask>,
    ) -> Pin<Box<dyn Future<Output = Result<&'c str, Error>> + Send + 'fut>>;

    fn teardown<'fut, 'a: 'fut, 'b: 'fut, 'c: 'fut, 'd: 'fut>(
        &'a mut self,
        client: &'b mut AcmeClient,
        authorization: &'c Authorization,
        domain: &'d AcmeDomain,
        task: Arc<WorkerTask>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'fut>>;
}

fn extract_challenge<'a>(
    authorization: &'a Authorization,
    ty: &str,
) -> Result<&'a Challenge, Error> {
    authorization
        .challenges
        .iter()
        .find(|ch| ch.ty == ty)
        .ok_or_else(|| format_err!("no supported challenge type ({}) found", ty))
}

async fn pipe_to_tasklog<T: AsyncRead + Unpin>(
    pipe: T,
    task: Arc<WorkerTask>,
) -> Result<(), std::io::Error> {
    let mut pipe = BufReader::new(pipe);
    let mut line = String::new();
    loop {
        line.clear();
        match pipe.read_line(&mut line).await {
            Ok(0) => return Ok(()),
            Ok(_) => task.log_message(line.as_str()),
            Err(err) => return Err(err),
        }
    }
}

impl DnsPlugin {
    async fn action<'a>(
        &self,
        client: &mut AcmeClient,
        authorization: &'a Authorization,
        domain: &AcmeDomain,
        task: Arc<WorkerTask>,
        action: &str,
    ) -> Result<&'a str, Error> {
        let challenge = extract_challenge(authorization, "dns-01")?;
        let mut stdin_data = client
            .dns_01_txt_value(
                challenge
                    .token()
                    .ok_or_else(|| format_err!("missing token in challenge"))?,
            )?
            .into_bytes();
        stdin_data.push(b'\n');
        stdin_data.extend(self.data.as_bytes());
        if stdin_data.last() != Some(&b'\n') {
            stdin_data.push(b'\n');
        }

        let mut command = Command::new("/usr/bin/setpriv");

        #[rustfmt::skip]
        command.args([
            "--reuid", "nobody",
            "--regid", "nogroup",
            "--clear-groups",
            "--reset-env",
            "--",
            "/bin/bash",
                PROXMOX_ACME_SH_PATH,
                action,
                &self.core.api,
                domain.alias.as_deref().unwrap_or(&domain.domain),
        ]);

        // We could use 1 socketpair, but tokio wraps them all in `File` internally causing `close`
        // to be called separately on all of them without exception, so we need 3 pipes :-(

        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let mut stdin = child.stdin.take().expect("Stdio::piped()");
        let stdout = child.stdout.take().expect("Stdio::piped() failed?");
        let stdout = pipe_to_tasklog(stdout, Arc::clone(&task));
        let stderr = child.stderr.take().expect("Stdio::piped() failed?");
        let stderr = pipe_to_tasklog(stderr, Arc::clone(&task));
        let stdin = async move {
            stdin.write_all(&stdin_data).await?;
            stdin.flush().await?;
            Ok::<_, std::io::Error>(())
        };
        match futures::try_join!(stdin, stdout, stderr) {
            Ok(((), (), ())) => (),
            Err(err) => {
                if let Err(err) = child.kill().await {
                    task.log_message(format!(
                        "failed to kill '{} {}' command: {}",
                        PROXMOX_ACME_SH_PATH, action, err
                    ));
                }
                bail!("'{}' failed: {}", PROXMOX_ACME_SH_PATH, err);
            }
        }

        let status = child.wait().await?;
        if !status.success() {
            bail!(
                "'{} {}' exited with error ({})",
                PROXMOX_ACME_SH_PATH,
                action,
                status.code().unwrap_or(-1)
            );
        }

        Ok(&challenge.url)
    }
}

impl AcmePlugin for DnsPlugin {
    fn setup<'fut, 'a: 'fut, 'b: 'fut, 'c: 'fut, 'd: 'fut>(
        &'a mut self,
        client: &'b mut AcmeClient,
        authorization: &'c Authorization,
        domain: &'d AcmeDomain,
        task: Arc<WorkerTask>,
    ) -> Pin<Box<dyn Future<Output = Result<&'c str, Error>> + Send + 'fut>> {
        Box::pin(async move {
            let result = self
                .action(client, authorization, domain, task.clone(), "setup")
                .await;

            let validation_delay = self.core.validation_delay.unwrap_or(30) as u64;
            if validation_delay > 0 {
                task.log_message(format!(
                    "Sleeping {} seconds to wait for TXT record propagation",
                    validation_delay
                ));
                tokio::time::sleep(Duration::from_secs(validation_delay)).await;
            }
            result
        })
    }

    fn teardown<'fut, 'a: 'fut, 'b: 'fut, 'c: 'fut, 'd: 'fut>(
        &'a mut self,
        client: &'b mut AcmeClient,
        authorization: &'c Authorization,
        domain: &'d AcmeDomain,
        task: Arc<WorkerTask>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'fut>> {
        Box::pin(async move {
            self.action(client, authorization, domain, task, "teardown")
                .await
                .map(drop)
        })
    }
}

#[derive(Default)]
struct StandaloneServer {
    abort_handle: Option<futures::future::AbortHandle>,
}

// In case the "order_certificates" future gets dropped between setup & teardown, let's also cancel
// the HTTP listener on Drop:
impl Drop for StandaloneServer {
    fn drop(&mut self) {
        self.stop();
    }
}

impl StandaloneServer {
    fn stop(&mut self) {
        if let Some(abort) = self.abort_handle.take() {
            abort.abort();
        }
    }
}

async fn standalone_respond(
    req: Request<Incoming>,
    path: Arc<String>,
    key_auth: Arc<String>,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    if req.method() == hyper::Method::GET && req.uri().path() == path.as_str() {
        Ok(Response::builder()
            .status(http::StatusCode::OK)
            .body(Full::from(Bytes::from(key_auth.as_bytes().to_owned())))
            .unwrap())
    } else {
        Ok(Response::builder()
            .status(http::StatusCode::NOT_FOUND)
            .body("Not found.".into())
            .unwrap())
    }
}

impl AcmePlugin for StandaloneServer {
    fn setup<'fut, 'a: 'fut, 'b: 'fut, 'c: 'fut, 'd: 'fut>(
        &'a mut self,
        client: &'b mut AcmeClient,
        authorization: &'c Authorization,
        _domain: &'d AcmeDomain,
        _task: Arc<WorkerTask>,
    ) -> Pin<Box<dyn Future<Output = Result<&'c str, Error>> + Send + 'fut>> {
        use hyper::service::service_fn;

        Box::pin(async move {
            self.stop();

            let challenge = extract_challenge(authorization, "http-01")?;
            let token = challenge
                .token()
                .ok_or_else(|| format_err!("missing token in challenge"))?;
            let key_auth = Arc::new(client.key_authorization(token)?);
            let path = Arc::new(format!("/.well-known/acme-challenge/{}", token));

            // `[::]:80` first, then `*:80`
            let dual = SocketAddr::new(IpAddr::from([0u16; 8]), 80);
            let ipv4 = SocketAddr::new(IpAddr::from([0u8; 4]), 80);
            let incoming = TcpListener::bind(dual)
                .or_else(|_| TcpListener::bind(ipv4))
                .await?;

            let server = async move {
                loop {
                    let key_auth = Arc::clone(&key_auth);
                    let path = Arc::clone(&path);
                    match incoming.accept().await {
                        Ok((tcp, _)) => {
                            let io = TokioIo::new(tcp);
                            let service = service_fn(move |request| {
                                standalone_respond(
                                    request,
                                    Arc::clone(&path),
                                    Arc::clone(&key_auth),
                                )
                            });

                            tokio::task::spawn(async move {
                                if let Err(err) =
                                    http1::Builder::new().serve_connection(io, service).await
                                {
                                    println!("Error serving connection: {err:?}");
                                }
                            });
                        }
                        Err(err) => println!("Error accepting connection: {err:?}"),
                    }
                }
            };

            let (future, abort) = futures::future::abortable(server);
            self.abort_handle = Some(abort);
            tokio::spawn(future);

            Ok(challenge.url.as_str())
        })
    }

    fn teardown<'fut, 'a: 'fut, 'b: 'fut, 'c: 'fut, 'd: 'fut>(
        &'a mut self,
        _client: &'b mut AcmeClient,
        _authorization: &'c Authorization,
        _domain: &'d AcmeDomain,
        _task: Arc<WorkerTask>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'fut>> {
        Box::pin(async move {
            if let Some(abort) = self.abort_handle.take() {
                abort.abort();
            }
            Ok(())
        })
    }
}
