use std::{
    fmt,
    io::{Error, ErrorKind},
};

use serde::Serialize;

#[derive(Debug)]
pub struct TunnelCtx {
    id: u128,
}

impl fmt::Display for TunnelCtx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id)
    }
}

#[derive(Serialize)]
pub enum EstablishTunnelResult {
    /// Successfully connected to target.  
    Ok,
    /// Malformed request
    BadRequest,
    /// Target is not allowed
    Forbidden,
    /// Unsupported operation, however valid for the protocol.
    OperationNotAllowed,
    /// The client failed to send a tunnel request timely.
    RequestTimeout,
    /// Cannot connect to target.
    BadGateway,
    /// Connection attempt timed out.
    GatewayTimeout,
    /// Busy. Try again later.
    TooManyRequests,
    /// Any other error. E.g. an abrupt I/O error.
    ServerError,
}

impl From<Error> for EstablishTunnelResult {
    fn from(e: Error) -> Self {
        match e.kind() {
            ErrorKind::TimedOut => EstablishTunnelResult::GatewayTimeout,
            _ => EstablishTunnelResult::BadGateway,
        }
    }
}
