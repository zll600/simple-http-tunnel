use async_trait::async_trait;
use futures::stream::SplitStream;
use futures::{SinkExt, StreamExt};
use log::{debug, error};
use std::{
    fmt::{self, Display},
    io::{Error, ErrorKind},
    marker::PhantomData,
};
use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf},
    net::TcpStream,
    time::Duration,
};

use derive_builder::Builder;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, Framed};

use crate::{codec::Nugget, conf::TunnelConfig};

#[derive(Builder, Clone, Debug)]
pub struct TunnelCtx {
    id: u128,
}

impl fmt::Display for TunnelCtx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id)
    }
}

#[derive(Clone, Debug, Serialize)]
pub enum EstablishTunnelResult {
    /// Successfully connected to target.  
    Ok,
    /// Successfully connected to target but has a nugget to send after connection establishment.
    OkWithNugget,
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

#[async_trait]
pub trait TunnelTarget {
    type Addr;
    fn target_addr(&self) -> Self::Addr;
    fn has_nugget(&self) -> bool;
    fn nugget(&self) -> &Nugget;
}

#[derive(Builder)]
pub struct HttpTunnelTarget {
    target: String,
    pub nugget: Option<Nugget>,
}

impl fmt::Display for HttpTunnelTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.target)
    }
}

#[async_trait]
impl TunnelTarget for HttpTunnelTarget {
    type Addr = String;

    fn target_addr(&self) -> Self::Addr {
        self.target.clone()
    }

    fn has_nugget(&self) -> bool {
        self.nugget.is_some()
    }

    fn nugget(&self) -> &Nugget {
        self.nugget
            .as_ref()
            .expect("Cannot use this method without checking `has_nugget`")
    }
}

#[async_trait]
pub trait TargetConnector {
    type Target: TunnelTarget + Send + Sync + Sized;
    type Stream: AsyncRead + AsyncWrite + Send + Sized + 'static;

    async fn connect(&mut self, target: &Self::Target) -> io::Result<Self::Stream>;
}

pub struct SimpleTcpConnector<D> {
    connect_timeout: Duration,
    // tunnel_ctx: TunnelCtx,
    // dns_resolver: R,
    // #[builder(setter(skip))]
    _phantom_target: PhantomData<D>,
}

impl<D> SimpleTcpConnector<D> {
    pub fn new(connect_timeout: Duration) -> Self {
        Self {
            // dns_resolver,
            connect_timeout,
            // tunnel_ctx,
            _phantom_target: PhantomData,
        }
    }
}

#[async_trait]
impl<D> TargetConnector for SimpleTcpConnector<D>
where
    D: TunnelTarget<Addr = String> + Send + Sync + Sized,
{
    type Target = D;
    type Stream = TcpStream;

    async fn connect(&mut self, target: &Self::Target) -> io::Result<Self::Stream> {
        let target_addr = &target.target_addr();

        // let addr = self.dns_resolver.resolve(target_addr).await?;
        let addr = target_addr;

        match TcpStream::connect(addr).await {
            Ok(tcp_stream) => {
                let mut stream = tcp_stream;
                if target.has_nugget() {
                    if let Ok(written_successfully) =
                        stream.write_all(&target.nugget().data()).await
                    {
                        written_successfully;
                    } else {
                        error!("Timeout sending nugget to {}, {}", addr, target_addr,);
                        return Err(Error::from(ErrorKind::TimedOut));
                    }
                }
                Ok(stream)
            }
            Err(_) => {
                error!("Timeout connecting to {}, {}", addr, target_addr);
                Err(Error::from(ErrorKind::TimedOut))
            }
        }
    }
}

/// A connection tunnel.
///
/// # Parameters
/// * `<H>` - proxy handshake codec for initiating a tunnel.
///    It extracts the request message, which contains the target, and, potentially policies.
///    It also takes care of encoding a response.
/// * `<C>` - a connection from from client.
/// * `<T>` - target connector. It takes result produced by the codec and establishes a connection
///           to a target.
///
/// Once the target connection is established, it relays data until any connection is closed or an
/// error happens.
pub struct ConnectionTunnel<H, C, T> {
    tunnel_request_codec: Option<H>,
    target_connector: T,
    client: Option<C>,
    tunnel_config: TunnelConfig,
}

impl<H, C, T> ConnectionTunnel<H, C, T>
where
    H: Decoder<Error = EstablishTunnelResult> + Encoder<EstablishTunnelResult>,
    H::Item: TunnelTarget + Sized + Display + Send + Sync,
    C: AsyncRead + AsyncWrite + Sized + Send + Unpin + 'static,
    T: TargetConnector<Target = H::Item>,
{
    pub fn new(
        handshake_codec: H,
        target_connector: T,
        client: C,
        tunnel_config: TunnelConfig,
    ) -> Self {
        Self {
            tunnel_request_codec: Some(handshake_codec),
            target_connector,
            client: Some(client),
            tunnel_config,
        }
    }

    /// Once the client connected we wait for a tunnel establishment handshake.
    /// For instance, an `HTTP/1.1 CONNECT` for HTTP tunnels.
    ///
    /// During handshake we obtained the target target, and if we were able to connect to it,
    /// a message indicating success is sent back to client (or an error response otherwise).
    ///
    /// At that point we start relaying data in full-duplex mode.
    ///
    /// # Note
    /// This method consumes `self` and thus can be called only once.
    pub async fn start(mut self) -> io::Result<TunnelStats> {
        let stream = self.client.take().expect("downstream can be taken once");

        let tunnel_result = self
            .establish_tunnel(stream, self.tunnel_config.clone())
            .await;

        if let Err(error) = tunnel_result {
            return Ok(TunnelStats { result: error });
        }

        let (client, target) = tunnel_result.unwrap();
        relay_connections(client, target).await
    }

    async fn establish_tunnel(
        &mut self,
        stream: C,
        configuration: TunnelConfig,
    ) -> Result<(C, T::Stream), EstablishTunnelResult> {
        let (mut write, mut read) = self
            .tunnel_request_codec
            .take()
            .expect("establish_tunnel can be called only once")
            .framed(stream)
            .split();

        let (response, target) = self.process_tunnel_request(&configuration, &mut read).await;

        let response_sent = match response {
            EstablishTunnelResult::OkWithNugget => true,
            _ => write.send(response.clone()).await.is_ok(),
        };

        if response_sent {
            match target {
                None => Err(response),
                Some(u) => {
                    // lets take the original stream to either relay data, or to drop it on error
                    let framed = write.reunite(read).expect("Uniting previously split parts");
                    let original_stream = framed.into_inner();

                    Ok((original_stream, u))
                }
            }
        } else {
            Err(EstablishTunnelResult::RequestTimeout)
        }
    }

    async fn process_tunnel_request(
        &mut self,
        configuration: &TunnelConfig,
        read: &mut SplitStream<Framed<C, H>>,
    ) -> (
        EstablishTunnelResult,
        Option<<T as TargetConnector>::Stream>,
    ) {
        let connect_request = read.next().await;

        let response;
        let mut target = None;

        if let Some(event) = connect_request {
            match event {
                Ok(decoded_target) => {
                    let has_nugget = decoded_target.has_nugget();
                    response = match self
                        .connect_to_target(
                            decoded_target,
                            configuration.target_connection.connect_timeout,
                        )
                        .await
                    {
                        Ok(t) => {
                            target = Some(t);
                            if has_nugget {
                                EstablishTunnelResult::OkWithNugget
                            } else {
                                EstablishTunnelResult::Ok
                            }
                        }
                        Err(e) => e,
                    }
                }
                Err(e) => {
                    response = e;
                }
            }
        } else {
            response = EstablishTunnelResult::BadRequest;
        }

        (response, target)
    }

    async fn connect_to_target(
        &mut self,
        target: T::Target,
        connect_timeout: Duration,
    ) -> Result<T::Stream, EstablishTunnelResult> {
        debug!("Establishing HTTP tunnel target connection: {}", target);

        let tcp_stream = self.target_connector.connect(&target).await;
        match tcp_stream {
            Ok(tcp_stream) => Ok(tcp_stream),
            Err(e) => Err(EstablishTunnelResult::from(e)),
        }
    }
}

pub async fn relay_connections<
    D: AsyncRead + AsyncWrite + Sized + Send + Unpin + 'static,
    U: AsyncRead + AsyncWrite + Sized + Send + 'static,
>(
    client: D,
    target: U,
    // ctx: TunnelCtx,
) -> io::Result<TunnelStats> {
    let (client_recv, client_send) = io::split(client);
    let (target_recv, target_send) = io::split(target);

    let downstream_relay: Relay = RelayBuilder::default()
        .name("Downstream")
        // .tunnel_ctx(ctx)
        // .relay_policy(downstream_relay_policy)
        .build()
        .expect("RelayBuilder failed");

    let upstream_relay: Relay = RelayBuilder::default()
        .name("Upstream")
        // .tunnel_ctx(ctx)
        // .relay_policy(upstream_relay_policy)
        .build()
        .expect("RelayBuilder failed");

    let upstream_task =
        tokio::spawn(async move { downstream_relay.relay_data(client_recv, target_send).await });

    let downstream_task =
        tokio::spawn(async move { upstream_relay.relay_data(target_recv, client_send).await });

    let downstream_stats = downstream_task.await??;
    let upstream_stats = upstream_task.await??;

    Ok(TunnelStats {
        // tunnel_ctx: ctx,
        result: EstablishTunnelResult::Ok,
        // upstream_stats: Some(upstream_stats),
        // downstream_stats: Some(downstream_stats),
    })
}

#[derive(Builder)]
pub struct Relay {
    // relay_policy: RelayPolicy,
    name: &'static str,
    // tunnel_ctx: TunnelCtx,
}

impl Relay {
    pub async fn relay_data<R: AsyncReadExt + Sized, W: AsyncWriteExt + Sized>(
        self,
        mut source: ReadHalf<R>,
        mut dest: WriteHalf<W>,
    ) -> io::Result<RelayStats> {
        let mut buffer = [0, 255];

        let mut shutdown_reason = RelayShutdownReasons::None;
        loop {
            let read_result = source.read(&mut buffer).await;

            if read_result.is_err() {
                shutdown_reason = RelayShutdownReasons::ReaderTimeout;
                break;
            }

            let n = match read_result {
                Ok(n) if n == 0 => {
                    shutdown_reason = RelayShutdownReasons::GracefulShutdown;
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    error!("Err: {:?}", e);
                    shutdown_reason = RelayShutdownReasons::ReadError;
                    break;
                }
            };

            let write_result = dest.write_all(&buffer[..n]).await;

            if write_result.is_err() {
                shutdown_reason = RelayShutdownReasons::WriteError;
                break;
            }
        }
        let res = RelayStats { shutdown_reason };
        return Ok(res);
    }
}

/// Stats after the relay is closed. Can be used for telemetry/monitoring.
#[derive(Builder, Clone, Debug, Serialize)]
pub struct RelayStats {
    pub shutdown_reason: RelayShutdownReasons,
    // pub total_bytes: usize,
    // pub event_count: usize,
    // pub duration: Duration,
}

#[derive(Clone, Debug, Serialize)]
pub enum RelayShutdownReasons {
    None,
    GracefulShutdown,
    ReadError,
    WriteError,
    ReaderTimeout,
    WriterTimeout,
}

/// Statistics. No sensitive information.
#[derive(Serialize)]
pub struct TunnelStats {
    // tunnel_ctx: TunnelCtx,
    result: EstablishTunnelResult,
    // upstream_stats: Option<RelayStats>,
    // downstream_stats: Option<RelayStats>,
}
