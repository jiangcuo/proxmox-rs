use failure::*;

use std::collections::HashMap;
use std::sync::Arc;

use futures::*;
use hyper::{Body, Request, Response, StatusCode};

use crate::tools;
use crate::api_schema::router::*;
use crate::server::formatter::*;
use crate::server::WorkerTask;

/// Hyper Service implementation to handle stateful H2 connections.
///
/// We use this kind of service to handle backup protocol
/// connections. State is stored inside the generic ``rpcenv``. Logs
/// goes into the ``WorkerTask`` log.
pub struct H2Service<E> {
    router: &'static Router,
    rpcenv: E,
    worker: Arc<WorkerTask>,
    debug: bool,
}

impl <E: RpcEnvironment + Clone> H2Service<E> {

    pub fn new(rpcenv: E, worker: Arc<WorkerTask>, router: &'static Router, debug: bool) -> Self {
        Self { rpcenv, worker, router, debug }
    }

    pub fn debug<S: AsRef<str>>(&self, msg: S) {
        if self.debug { self.worker.log(msg); }
    }

    fn handle_request(&self, req: Request<Body>) -> BoxFut {

        let (parts, body) = req.into_parts();

        let method = parts.method.clone();

        let (path, components) = match tools::normalize_uri_path(parts.uri.path()) {
            Ok((p,c)) => (p, c),
            Err(err) => return Box::new(future::err(http_err!(BAD_REQUEST, err.to_string()))),
        };

        self.debug(format!("{} {}", method, path));

        let mut uri_param = HashMap::new();

        let formatter = &JSON_FORMATTER;

        match self.router.find_method(&components, method, &mut uri_param) {
            MethodDefinition::None => {
                let err = http_err!(NOT_FOUND, "Path not found.".to_string());
                return Box::new(future::ok((formatter.format_error)(err)));
            }
            MethodDefinition::Simple(api_method) => {
                return crate::server::rest::handle_sync_api_request(
                    self.rpcenv.clone(), api_method, formatter, parts, body, uri_param);
            }
            MethodDefinition::Async(async_method) => {
                return crate::server::rest::handle_async_api_request(
                    self.rpcenv.clone(), async_method, formatter, parts, body, uri_param);
            }
        }
    }

    fn log_response(worker: Arc<WorkerTask>, method: hyper::Method, path: &str, resp: &Response<Body>) {

        let status = resp.status();

        if !status.is_success() {
            let reason = status.canonical_reason().unwrap_or("unknown reason");

            let mut message = "request failed";
            if let Some(data) = resp.extensions().get::<ErrorMessageExtension>() {
                message = &data.0;
            }

            worker.log(format!("{} {}: {} {}: {}", method.as_str(), path, status.as_str(), reason, message));
        }
    }
}

impl <E: RpcEnvironment + Clone> hyper::service::Service for H2Service<E> {
    type ReqBody = Body;
    type ResBody = Body;
    type Error = Error;
    type Future = Box<dyn Future<Item = Response<Body>, Error = Self::Error> + Send>;

    fn call(&mut self, req: Request<Self::ReqBody>) -> Self::Future {
        let path = req.uri().path().to_owned();
        let method = req.method().clone();
        let worker = self.worker.clone();

        Box::new(self.handle_request(req).then(move |result| {
            match result {
                Ok(res) => {
                    Self::log_response(worker, method, &path, &res);
                    Ok::<_, Error>(res)
                }
                Err(err) => {
                     if let Some(apierr) = err.downcast_ref::<HttpError>() {
                        let mut resp = Response::new(Body::from(apierr.message.clone()));
                        resp.extensions_mut().insert(ErrorMessageExtension(apierr.message.clone()));
                        *resp.status_mut() = apierr.code;
                        Self::log_response(worker, method, &path, &resp);
                        Ok(resp)
                    } else {
                        let mut resp = Response::new(Body::from(err.to_string()));
                        resp.extensions_mut().insert(ErrorMessageExtension(err.to_string()));
                        *resp.status_mut() = StatusCode::BAD_REQUEST;
                        Self::log_response(worker, method, &path, &resp);
                        Ok(resp)
                    }
                }
            }
        }))
    }
}
