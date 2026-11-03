//! A native Windows SSH destination needs a different shell transport. Keep commands and
//! pasted text off the command line, and deliver the versioned Herdr bridge over stdin.
use inyon_protocol::{HostConfig, HostKind};
use serde_json::json;

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
    let body = serde_json::to_vec(&json!({ "mode": mode, "command": cmd, "source": SOURCE })).expect("bridge JSON");
    let mut out = format!("{}\n", body.len()).into_bytes();
    out.extend(body);
    out.extend(input.unwrap_or_default());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

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
