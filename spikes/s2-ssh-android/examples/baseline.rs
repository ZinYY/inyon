//! The same runtime with no SSH: the difference in size to `exec` is what russh costs.

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:22".into());
    match tokio::net::TcpStream::connect(&addr).await {
        Ok(_) => println!("connected to {addr}"),
        Err(e) => println!("could not connect to {addr}: {e}"),
    }
}
