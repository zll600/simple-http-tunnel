use std::fmt::Write;
use std::{str::Split, sync::Arc};

use bytes::BytesMut;
use derive_builder::Builder;
use log::debug;
use tokio_util::codec::{Decoder, Encoder};

use crate::tunnel::{EstablishTunnelResult, HttpTunnelTarget, HttpTunnelTargetBuilder, TunnelCtx};

const REQUEST_END_MARKER: &[u8] = b"\r\n\r\n";
/// A reasonable value to limit possible header size.
const MAX_HTTP_REQUEST_SIZE: usize = 16384; // 1024 * 16

const HOST_HEADER: &str = "host:";

#[derive(Builder, Clone)]
pub struct HttpTunnelCodec {
    // tunnel_ctx: TunnelCtx,
    // enabled_targets: Regex,
}

impl Decoder for HttpTunnelCodec {
    type Item = HttpTunnelTarget;
    type Error = EstablishTunnelResult;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if !got_http_request(&src) {
            return Ok(None);
        }

        match HttpConnectRequest::parse(&src) {
            Ok(parsed_request) => {
                // if !self.enabled_destinations.is_match(&parsed_request.uri) {
                //     debug!(
                //         "Destination `{}` is not allowed. Allowed: `{}`, CTX={}",
                //         parsed_request.uri, self.enabled_destinations, self.tunnel_ctx
                //     );
                //     return Err(EstablishTunnelResult::Forbidden);
                // }
                return Ok(Some(
                    HttpTunnelTargetBuilder::default()
                        .target(parsed_request.uri)
                        .build()
                        .expect("HttpTunnelTargetBuilder failed"),
                ));
            }
            Err(e) => Err(e),
        }
    }
}

impl Encoder<EstablishTunnelResult> for HttpTunnelCodec {
    type Error = std::io::Error;

    fn encode(
        &mut self,
        item: EstablishTunnelResult,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        let (code, message) = match item {
            EstablishTunnelResult::Ok => (200, "OK"),
            EstablishTunnelResult::OkWithNugget => (200, "OK"),
            EstablishTunnelResult::BadRequest => (400, "BAD_REQUEST"),
            EstablishTunnelResult::Forbidden => (403, "FORBIDDEN"),
            EstablishTunnelResult::OperationNotAllowed => (405, "NOT_ALLOWED"),
            EstablishTunnelResult::RequestTimeout => (408, "TIMEOUT"),
            EstablishTunnelResult::TooManyRequests => (429, "TOO_MANY_REQUESTS"),
            EstablishTunnelResult::ServerError => (500, "SERVER_ERROR"),
            EstablishTunnelResult::BadGateway => (502, "BAD_GATEWAY"),
            EstablishTunnelResult::GatewayTimeout => (504, "GATEWAY_TIMEOUT"),
        };

        dst.write_fmt(format_args!("HTTP/1.1 {} {}\r\n\r\n", code as u32, message))
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))
    }
}

fn got_http_request(buffer: &BytesMut) -> bool {
    buffer.len() >= MAX_HTTP_REQUEST_SIZE || buffer.ends_with(REQUEST_END_MARKER)
}

struct HttpConnectRequest {
    uri: String,
    nugget: Option<Nugget>,
}

impl HttpConnectRequest {
    pub fn parse(http_request: &[u8]) -> Result<Self, EstablishTunnelResult> {
        HttpConnectRequest::validate_request_size(http_request)?;
        HttpConnectRequest::validate_legal_characters(http_request)?;

        let http_request_as_string =
            String::from_utf8(http_request.to_vec()).expect("Contains only ASCII");

        let mut lines = http_request_as_string.split("\r\n");

        let request_line = HttpConnectRequest::parse_request_line(
            lines
                .next()
                .expect("At least a single line is present at this point"),
        )?;

        let has_nugget = request_line.3;

        if has_nugget {
            return Ok(Self {
                uri: HttpConnectRequest::extract_destination_host(&mut lines, request_line.1)
                    .unwrap_or_else(|| request_line.1.to_string()),
                nugget: Some(Nugget::new(http_request)),
            });
        }
        return Ok(Self {
            uri: request_line.1.to_string(),
            nugget: None,
        });
    }

    fn extract_destination_host(lines: &mut Split<&str>, endpoint: &str) -> Option<String> {
        lines
            .find(|line| line.to_ascii_lowercase().starts_with(HOST_HEADER))
            .map(|line| line[HOST_HEADER.len()..].trim())
            .map(|host| {
                let mut host = String::from(host);
                if host.rfind(':').is_none() {
                    let default_port = if endpoint.to_ascii_lowercase().starts_with("https://") {
                        ":443"
                    } else {
                        ":80"
                    };
                    host.push_str(default_port);
                }
                host
            })
    }

    fn parse_request_line(
        request_line: &str,
    ) -> Result<(&str, &str, &str, bool), EstablishTunnelResult> {
        let request_line_items = request_line.split(' ').collect::<Vec<&str>>();
        HttpConnectRequest::validate_well_formed(request_line, &request_line_items)?;

        let method = request_line_items[0];
        let uri = request_line_items[1];
        let version = request_line_items[2];

        let has_nugget = HttpConnectRequest::validate_method(method)?;
        HttpConnectRequest::validate_version(version)?;

        Ok((method, uri, version, has_nugget))
    }

    fn validate_version(version: &str) -> Result<(), EstablishTunnelResult> {
        if version != "HTTP/1.1" {
            debug!("Bad version {}", version);
            return Err(EstablishTunnelResult::BadRequest);
        }
        return Ok(());
    }

    fn validate_method(method: &str) -> Result<bool, EstablishTunnelResult> {
        if method != "CONNECT" {
            debug!("Not allowed method {}", method);
            return Err(EstablishTunnelResult::OperationNotAllowed);
        }
        return Ok(false);
    }

    fn validate_well_formed(
        request_line: &str,
        request_line_items: &[&str],
    ) -> Result<(), EstablishTunnelResult> {
        if request_line_items.len() != 3 {
            debug!("Bad request line: `{:?}`", request_line,);
            return Err(EstablishTunnelResult::BadRequest);
        }
        return Ok(());
    }

    fn validate_request_size(http_request: &[u8]) -> Result<(), EstablishTunnelResult> {
        if http_request.len() >= MAX_HTTP_REQUEST_SIZE {
            debug!(
                "Bad request. Size {} exceeds limit {}",
                http_request.len(),
                MAX_HTTP_REQUEST_SIZE
            );
            return Err(EstablishTunnelResult::BadRequest);
        }
        return Ok(());
    }

    fn validate_legal_characters(http_request: &[u8]) -> Result<(), EstablishTunnelResult> {
        for b in http_request {
            match b {
                // accept ascii character only.
                32..=126 | 9 | 10 | 13 => {}
                _ => {
                    debug!("Bad request. Illegal character: {:#04x}", b);
                    return Err(EstablishTunnelResult::BadRequest);
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct Nugget {
    data: Arc<Vec<u8>>,
}

impl Nugget {
    pub fn new<T: Into<Vec<u8>>>(v: T) -> Self {
        Self {
            data: Arc::new(v.into()),
        }
    }

    pub fn data(&self) -> Arc<Vec<u8>> {
        self.data.clone()
    }
}
