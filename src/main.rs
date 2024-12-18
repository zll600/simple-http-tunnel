use codec::{HttpTunnelCodec, HttpTunnelCodecBuilder};
use futures::{SinkExt, StreamExt};
use log::*;
use tokio::{
    io::{self, AsyncRead, AsyncWrite},
    net::TcpListener,
};
use tokio_util::codec::Decoder;
use tunnel::{
    EstablishTunnelResult, HttpTunnelTarget, Relay, RelayBuilder, SimpleTcpConnector,
    TargetConnector, TunnelTarget,
};

mod codec;
mod conf;
mod tunnel;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind_address = "127.0.0.1:10086";

    let tcp_listener = match TcpListener::bind(bind_address).await {
        Ok(s) => {
            info!("Serve request on: {}", bind_address);
            s
        }
        Err(e) => {
            panic!("Error binding address {}: {}", bind_address, e);
        }
    };

    loop {
        // Asynchronously wait for an inbound socket.
        let socket = tcp_listener.accept().await;

        match socket {
            Ok((stream, _)) => {
                // handle accepted connections asynchronously
                tokio::spawn(async move { tunnel_stream(stream).await });
            }
            Err(e) => error!("Failed TCP handshake {}", e),
        }
    }
}

/// Tunnel via a client connection.
async fn tunnel_stream<C: AsyncRead + AsyncWrite + Send + Unpin + 'static>(
    client_connection: C,
) -> io::Result<()> {
    // here it can be any codec.
    let codec: HttpTunnelCodec = HttpTunnelCodecBuilder::default()
        .build()
        .expect("HttpTunnelCodecBuilder failed");

    // any `TargetConnector` would do.
    let mut connector: SimpleTcpConnector<HttpTunnelTarget> = SimpleTcpConnector::new();

    let (mut write, mut read) = codec.framed(client_connection).split();

    let connect_request = read.next().await;

    let response;
    let mut target = None;

    if let Some(event) = connect_request {
        match event {
            Ok(decoded_target) => {
                let has_nugget = decoded_target.has_nugget();
                let tcp_stream = connector.connect(&decoded_target).await;

                response = match connector.connect(&decoded_target).await {
                    Ok(t) => {
                        target = Some(t);
                        if has_nugget {
                            EstablishTunnelResult::OkWithNugget
                        } else {
                            EstablishTunnelResult::Ok
                        }
                    }
                    Err(e) => EstablishTunnelResult::from(e),
                }
            }
            Err(e) => {
                response = e;
            }
        }
    } else {
        response = EstablishTunnelResult::BadRequest;
    }

    let response_sent = match response {
        EstablishTunnelResult::OkWithNugget => true,
        _ => write.send(response.clone()).await.is_ok(),
    };

    if !response_sent {
        panic!("fail to connect target");
    }
    let (client_stream, target_stream) = match target {
        None => panic!("fail to connect target"),
        Some(u) => {
            // lets take the original stream to either relay data, or to drop it on error
            let framed = write.reunite(read).expect("Uniting previously split parts");
            let original_stream = framed.into_inner();
            (original_stream, u)
        }
    };
    let (client_recv, client_send) = io::split(client_stream);
    let (target_recv, target_send) = io::split(target_stream);

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

    Ok(())
}
