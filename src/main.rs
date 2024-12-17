use clap::{self, Args, Parser, Subcommand};
use codec::{HttpTunnelCodec, HttpTunnelCodecBuilder};
use conf::ProxyConfiguration;
use derive_builder::Builder;
use log::*;
use rand::{thread_rng, Rng};
use serde::{self, Deserialize, Serialize};
use tokio::{
    io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf},
    net::TcpListener,
    time::Duration,
};
use tunnel::{
    ConnectionTunnel, EstablishTunnelResult, HttpTunnelTarget, SimpleTcpConnector, TunnelCtxBuilder,
};

mod codec;
mod conf;
mod tunnel;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proxy_configuration = ProxyConfiguration {
        bind_address: String::from("127.0.0.1:10086"),
        tunnel_config: conf::TunnelConfig {
            client_connection: conf::ClientConnectionConfig {
                connect_timeout: Duration::from_secs(60),
            },
            target_connection: conf::TargetConnectionConfig {
                connect_timeout: Duration::from_secs(60),
            },
        },
    };

    let tcp_listener = match TcpListener::bind(&proxy_configuration.bind_address).await {
        Ok(s) => {
            info!("Serve request on: {}", &proxy_configuration.bind_address);
            s
        }
        Err(e) => {
            panic!(
                "Error binding address {}: {}",
                &proxy_configuration.bind_address, e
            );
        }
    };

    loop {
        // Asynchronously wait for an inbound socket.
        let socket = tcp_listener.accept().await;

        match socket {
            Ok((stream, _)) => {
                let config = proxy_configuration.clone();
                // handle accepted connections asynchronously
                tokio::spawn(async move { tunnel_stream(&config, stream).await });
            }
            Err(e) => error!("Failed TCP handshake {}", e),
        }
    }
}

/// Tunnel via a client connection.
async fn tunnel_stream<C: AsyncRead + AsyncWrite + Send + Unpin + 'static>(
    config: &ProxyConfiguration,
    client_connection: C,
) -> io::Result<()> {
    let ctx = TunnelCtxBuilder::default()
        .id(thread_rng().gen::<u128>())
        .build()
        .expect("TunnelCtxBuilder failed");

    // here it can be any codec.
    let codec: HttpTunnelCodec = HttpTunnelCodecBuilder::default()
        .tunnel_ctx(ctx)
        .build()
        .expect("HttpTunnelCodecBuilder failed");

    // any `TargetConnector` would do.
    let connector: SimpleTcpConnector<HttpTunnelTarget> =
        SimpleTcpConnector::new(config.tunnel_config.target_connection.connect_timeout);

    let stats = ConnectionTunnel::new(
        codec,
        connector,
        client_connection,
        config.tunnel_config.clone(),
    )
    .start()
    .await;
    return Ok(());
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
struct Cli {
    /// Configuration file.
    #[clap(long)]
    config: Option<String>,
    /// Bind address, e.g. 0.0.0.0:8443.
    #[clap(long)]
    bind: String,
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Args, Debug)]
#[clap(about = "Run the tunnel in HTTP mode", long_about = None)]
#[clap(author, version, long_about = None)]
#[clap(propagate_version = true)]
struct HttpOptions {}

#[derive(Args, Debug)]
#[clap(about = "Run the tunnel in HTTPS mode", long_about = None)]
#[clap(author, version, long_about = None)]
#[clap(propagate_version = true)]
struct HttpsOptions {}

#[derive(Args, Debug)]
#[clap(about = "Run the tunnel in TCP proxy mode", long_about = None)]
#[clap(author, version, long_about = None)]
#[clap(propagate_version = true)]
struct TcpOptions {}

#[derive(Subcommand, Debug)]
enum Commands {
    Http(HttpOptions),
    Https(HttpsOptions),
    Tcp(TcpOptions),
}

#[derive(Builder, Deserialize, Clone)]
pub struct RelayPolicy {
    #[serde(with = "humantime_serde")]
    pub idle_timeout: Duration,
    /// Min bytes-per-minute (bpm)
    pub min_rate_bpm: u64,
    // Max bytes-per-second (bps)
    pub max_rate_bps: u64,
}

// #[tokio::test]
// async fn test_timed_operation_timeout() {
//     let time_duration = 1;
//     let data = b"data on the wire";
//     let mut mock_connection: Mock = Builder::new()
//         .wait(Duration::from_secs(time_duration * 2))
//         .read(data)
//         .build();

//     let relay_policy: RelayPolicy = RelayPolicyBuilder::default()
//         .min_rate_bpm(1000)
//         .max_rate_bps(100_000)
//         .idle_timeout(Duration::from_secs(time_duration))
//         .build()
//         .unwrap();

//     let mut buf = [0; 1024];
//     let timed_future = relay_policy
//         .timed_operation(mock_connection.read(&mut buf))
//         .await;
//     assert!(timed_future.is_err());
// }

// #[tokio::test]
// async fn test_timed_operation_failed_io() {
//     let mut mock_connection: Mock = Builder::new()
//         .read_error(Error::from(ErrorKind::BrokenPipe))
//         .build();

//     let relay_policy: RelayPolicy = RelayPolicyBuilder::default()
//         .min_rate_bpm(1000)
//         .max_rate_bps(100_000)
//         .idle_timeout(Duration::from_secs(5))
//         .build()
//         .unwrap();

//     let mut buf = [0; 1024];
//     let timed_future = relay_policy
//         .timed_operation(mock_connection.read(&mut buf))
//         .await;
//     assert!(timed_future.is_ok()); // no timeout
//     assert!(timed_future.unwrap().is_err()); // but io-error
// }
