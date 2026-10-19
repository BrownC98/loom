mod daemon;
mod node_host;
mod profile;
mod registry;

use std::net::SocketAddr;

use clap::Parser;

#[derive(Parser)]
#[command(name = "loom", version)]
enum Cli {
    /// 데몬과 화면을 띄운다.
    Serve {
        /// 데몬이 듣는 주소. 사용자는 한 명이므로 기본값은 루프백이다.
        #[arg(long, default_value = "127.0.0.1:7420")]
        listen: SocketAddr,
    },
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    match Cli::parse() {
        Cli::Serve { listen } => daemon::serve(listen).await,
    }
}
