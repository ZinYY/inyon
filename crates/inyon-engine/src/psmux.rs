//! A persistent, authenticated loopback connection to PSmux's control protocol.
//! Like `psmux -CC`, but without spawning a console or its debug log of typed input.
use std::path::Path;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::UnboundedReceiver;

pub fn valid_session(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.chars().any(|c| c.is_control() || "/\\:".contains(c))
}

pub async fn relay(session: &str, keys: UnboundedReceiver<Vec<u8>>, id: &Value, send: &crate::ops::Sender) -> Result<(), String> {
    let dir = std::env::var_os("PSMUX_DATA_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| crate::paths::home().join(".psmux"));
    relay_in(&dir, session, keys, id, send).await
}

async fn relay_in(dir: &Path, session: &str, mut keys: UnboundedReceiver<Vec<u8>>, id: &Value, send: &crate::ops::Sender) -> Result<(), String> {
    if !valid_session(session) { return Err("Invalid session name".into()); }
    let port: u16 = std::fs::read_to_string(dir.join(format!("{session}.port"))).map_err(|_| "Session port is unavailable")?.trim().parse().map_err(|_| "Invalid session port")?;
    let secret = std::fs::read_to_string(dir.join(format!("{session}.key"))).map_err(|_| "Session key is unavailable")?;
    let secret = secret.trim();
    if secret.is_empty() || secret.contains(['\r', '\n']) { return Err("Invalid session key".into()); }
    let mut stream = tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)))
        .await.map_err(|_| "Session connection timed out")?.map_err(|e| e.to_string())?;
    stream.set_nodelay(true).map_err(|e| e.to_string())?;
    stream.write_all(format!("AUTH {secret}\n").as_bytes()).await.map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut reply)).await.map_err(|_| "Session authentication timed out")?.map_err(|e| e.to_string())?;
    if reply.trim() != "OK" { return Err("Session authentication failed".into()); }
    reader.get_mut().write_all(b"CONTROL_NOECHO\n").await.map_err(|e| e.to_string())?;
    let mut buf = vec![0; 16384];
    loop {
        tokio::select! {
            data = keys.recv() => {
                let Some(data) = data else { return Ok(()) };
                reader.get_mut().write_all(&data).await.map_err(|e| e.to_string())?;
            }
            n = reader.read(&mut buf) => {
                let n = n.map_err(|e| e.to_string())?;
                if n == 0 { return Ok(()); }
                send(json!({"id":id,"type":"data","data":B64.encode(&buf[..n])}));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_names_cannot_escape_the_registry_directory() {
        for s in ["", ".", "..", "../other", "C:\\other", "x\ny"] { assert!(!valid_session(s)); }
        for s in ["work", "随便聊聊", "dev__my-session", "work space"] { assert!(valid_session(s)); }
    }

    #[tokio::test]
    async fn control_connection_authenticates_and_preserves_bidirectional_bytes() {
        let dir = std::env::temp_dir().join(format!("inyon-psmux-{}-{}", std::process::id(), crate::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        std::fs::write(dir.join("test.port"), listener.local_addr().unwrap().port().to_string()).unwrap();
        std::fs::write(dir.join("test.key"), "test-secret").unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = BufReader::new(socket);
            let mut line = String::new();
            socket.read_line(&mut line).await.unwrap();
            assert_eq!(line, "AUTH test-secret\n");
            socket.get_mut().write_all(b"OK\n").await.unwrap();
            line.clear(); socket.read_line(&mut line).await.unwrap();
            assert_eq!(line, "CONTROL_NOECHO\n");
            line.clear(); socket.read_line(&mut line).await.unwrap();
            assert_eq!(line, "send-keys -t %2 -H e4 bd a0\n");
            socket.get_mut().write_all(b"%output %2 \\344\\275\\240\n").await.unwrap();
        });
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(b"send-keys -t %2 -H e4 bd a0\n".to_vec()).unwrap();
        let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let out = output.clone();
        let send: crate::ops::Sender = std::sync::Arc::new(move |v| out.lock().unwrap().extend(B64.decode(v["data"].as_str().unwrap()).unwrap()));
        tokio::time::timeout(Duration::from_secs(5), relay_in(&dir, "test", rx, &json!("t"), &send)).await.unwrap().unwrap();
        server.await.unwrap();
        assert_eq!(*output.lock().unwrap(), b"%output %2 \\344\\275\\240\n");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
