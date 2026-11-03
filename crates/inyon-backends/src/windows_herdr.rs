//! A native Windows SSH destination needs a different shell transport. Keep commands and
//! pasted text off the command line, and deliver the versioned Herdr bridge over stdin.
use inyon_protocol::{HostConfig, HostKind};
use serde_json::json;
use inyon_core::hosts::host::{ExecResult, UTF8_TMUX};
use inyon_core::hosts::http_error::{HubError, HubResult};
use std::{rc::Rc, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

const SOURCE: &str = include_str!("windows_herdr.py");
const BOOTSTRAP: &str = "import os,json\nh=b''\nwhile not h.endswith(b'\\n'):\n c=os.read(0,1)\n if not c or len(h)>10: raise RuntimeError('Invalid Inyon frame')\n h+=c\nn=int(h)\nif n<1 or n>16777216: raise RuntimeError('Invalid Inyon frame size')\nb=b''\nwhile len(b)<n:\n c=os.read(0,n-len(b))\n if not c: raise RuntimeError('Incomplete Inyon frame')\n b+=c\nr=json.loads(b)\nexec(compile(r['source'],'<inyon-windows-herdr>','exec'),{'__name__':'__main__','INYON_REQUEST':r})\n";

pub(crate) fn enabled(c: &HostConfig) -> bool {
    c.kind == HostKind::Ssh && c.ssh_mode.as_deref() == Some("windows-herdr")
}

pub(crate) fn command() -> String {
    let hex: String = BOOTSTRAP.bytes().map(|b| format!("{b:02x}")).collect();
    format!("python -u -c \"exec(bytes.fromhex('{hex}'))\"")
}

pub(crate) fn input(cmd: &str, input: Option<Vec<u8>>) -> Vec<u8> {
    let mode = if cmd.ends_with(inyon_core::tmux::tmux::PROBE) {
        "probe"
    } else if cmd.contains("exec tmux") && cmd.contains("-C attach-session") {
        "control"
    } else {
        "shell"
    };
    frame(mode, cmd, input)
}

fn frame(mode: &str, cmd: &str, input: Option<Vec<u8>>) -> Vec<u8> {
    let body = serde_json::to_vec(&json!({ "mode": mode, "command": cmd, "source": SOURCE })).expect("bridge JSON");
    let mut out = format!("{}\n", body.len()).into_bytes();
    out.extend(body);
    out.extend(input.unwrap_or_default());
    out
}

pub(crate) fn request(cmd: &str) -> Option<serde_json::Value> {
    let cmd = cmd.strip_prefix(UTF8_TMUX).unwrap_or(cmd);
    serde_json::from_str(cmd.strip_prefix(inyon_core::tmux::herdr::PREFIX)?).ok()
}

pub(crate) type SharedChannel = Rc<Mutex<Option<Channel>>>;

/// One SSH process per destination, shared by screen reads and input. A failed request is
/// never replayed: the peer may already have applied it before its reply was lost.
pub(crate) struct Channel {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    sequence: u64,
}

impl Channel {
    async fn start(mut command: Command) -> HubResult<Self> {
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true)
            .spawn().map_err(|e| HubError::plain(e.to_string()))?;
        let mut stdin = child.stdin.take().expect("bridge stdin");
        let stdout = BufReader::new(child.stdout.take().expect("bridge stdout"));
        stdin.write_all(&frame("rpc", "", None)).await.map_err(|e| HubError::plain(e.to_string()))?;
        Ok(Self { _child: child, stdin, stdout, sequence: 0 })
    }

    async fn exchange(&mut self, request: serde_json::Value) -> HubResult<ExecResult> {
        self.sequence += 1;
        let mut bytes = serde_json::to_vec(&json!({"id":self.sequence, "request":request})).expect("bridge request");
        bytes.push(b'\n');
        self.stdin.write_all(&bytes).await.map_err(|e| HubError::plain(e.to_string()))?;
        let mut line = String::new();
        self.stdout.read_line(&mut line).await.map_err(|e| HubError::plain(e.to_string()))?;
        let reply: serde_json::Value = serde_json::from_str(&line).map_err(|_| HubError::plain("Windows input connection closed. Check the terminal before retrying."))?;
        if reply["id"].as_u64() != Some(self.sequence) {
            return Err(HubError::plain("Windows bridge response did not match this request"));
        }
        Ok(ExecResult { code: if reply["error"].is_string() { 1 } else { 0 },
            stdout: reply["stdout"].as_str().unwrap_or("").to_owned(), stderr: reply["error"].as_str().unwrap_or("").to_owned() })
    }
}

pub(crate) async fn exchange(shared: SharedChannel, command: Command, request: serde_json::Value, timeout: u64) -> HubResult<ExecResult> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout);
    let mut guard = tokio::time::timeout_at(deadline, shared.lock()).await
        .map_err(|_| HubError::plain("Windows input is busy. This request was not sent."))?;
    // Take ownership until completion, so cancellation also closes a half-read channel.
    let previous = guard.take();
    let result = tokio::time::timeout_at(deadline, async move {
        let mut channel = match previous { Some(c) => c, None => Channel::start(command).await? };
        let reply = channel.exchange(request).await?;
        Ok::<_, HubError>((channel, reply))
    }).await;
    match result {
        Ok(Ok((channel, reply))) => { *guard = Some(channel); Ok(reply) }
        Ok(Err(e)) => Err(e),
        Err(_) => Err(HubError::plain("Windows bridge timed out. Check the terminal before retrying; input was not replayed.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    fn fake_peer() -> tokio::process::Command {
        let mut c = tokio::process::Command::new("python3");
        c.args(["-u", "-c", r#"import sys,json,os,time
n=int(sys.stdin.buffer.readline()); sys.stdin.buffer.read(n)
for line in sys.stdin.buffer:
 r=json.loads(line); q=r['request']; time.sleep(q.get('delay',0))
 print(json.dumps({'id':r['id'],'stdout':str(os.getpid())+':'+str(r['id'])+':'+q.get('text','')}),flush=True)
"#]);
        c
    }

    #[test]
    fn requests_reuse_one_process_and_timeouts_discard_it_without_replaying_input() {
        crate::runtime::run_core(|| async {
            let shared = Rc::new(Mutex::new(None));
            let first = exchange(shared.clone(), fake_peer(), json!({"text":"中文\n'quoted'"}), 3000).await.unwrap();
            let second = exchange(shared.clone(), fake_peer(), json!({"text":"next"}), 3000).await.unwrap();
            let pid = first.stdout.split(':').next().unwrap();
            assert_eq!(second.stdout, format!("{pid}:2:next"));
            assert!(first.stdout.ends_with("中文\n'quoted'"));
            assert!(exchange(shared.clone(), fake_peer(), json!({"delay":2}), 100).await.is_err());
            assert!(shared.lock().await.is_none());
            let next = exchange(shared.clone(), fake_peer(), json!({"text":"explicit next request"}), 3000).await.unwrap();
            assert_ne!(next.stdout.split(':').next().unwrap(), pid);
            assert!(next.stdout.ends_with(":1:explicit next request"));
        });
    }

    #[test]
    fn only_structured_herdr_commands_take_the_persistent_transport() {
        let cmd = inyon_core::tmux::herdr::command(json!({"op":"capture", "panes":["%1"]}));
        assert_eq!(request(&format!("{UTF8_TMUX}{cmd}")), Some(json!({"op":"capture", "panes":["%1"]})));
        assert_eq!(request("echo '# inyon-herdr-rpc-v1'"), None);
    }

    #[test]
    fn bootstrap_consumes_only_the_frame_leaving_unicode_and_binary_input_untouched() {
        let source = "import os,sys\nsys.stdout.buffer.write(os.read(0,100000))";
        let body = serde_json::to_vec(&json!({"source":source})).unwrap();
        let data = "中文\r\nquotes ' \" $()".as_bytes();
        let mut child = Command::new("python3").args(["-u", "-c", BOOTSTRAP]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        write!(stdin, "{}\n", body.len()).unwrap();
        stdin.write_all(&body).unwrap();
        stdin.write_all(data).unwrap();
        drop(stdin);
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, data);
        assert!(command().len() < 3000);
    }
}
