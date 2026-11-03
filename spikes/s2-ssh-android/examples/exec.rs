//! `exec <host> <port> <user> <key file> <command>`: probe, log in, run one command. Built for
//! aarch64-linux-android to measure what russh adds to the app, and run on the emulator.

use std::time::{Duration, Instant};

use inyon_ssh_spike::{connect, probe, Credentials};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 6 {
        eprintln!("usage: exec <host> <port> <user> <key file> <command>");
        std::process::exit(2);
    }
    let port: u16 = a[2].parse().expect("port");
    let t = Instant::now();
    let hk = probe(&a[1], port).await.unwrap_or_else(|e| panic!("probe: {e}"));
    println!("host key {} {}", hk.key_type, hk.fingerprint);
    let creds = Credentials { username: a[3].clone(), private_key: Some(std::fs::read_to_string(&a[4]).expect("key file")), ..Default::default() };
    let c = connect(&a[1], port, &hk.fingerprint, &creds).await.unwrap_or_else(|e| panic!("connect: {} {e}", e.code()));
    println!("logged in with {} in {} ms", c.method, t.elapsed().as_millis());
    let chans = c.open_sessions(12).await;
    let ok = chans.iter().filter(|r| r.is_ok()).count();
    println!("{ok} of 12 sessions opened on one connection");
    let r = c.exec(&a[5], Duration::from_secs(20)).await.unwrap_or_else(|e| panic!("exec: {e}"));
    print!("{}", r.stdout);
    println!("exit {}", r.code);
    c.disconnect().await;
}
