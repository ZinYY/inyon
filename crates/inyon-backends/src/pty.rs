//! Terminals: a command in a pseudo-terminal (the phone's terminal view), the `pty` half of
//! hosts/process-backend.ts. The terminal's reads, its writes and the wait for its child all block,
//! so each runs on a thread of its own: a terminal that stops reading (ssh on a dead network, a
//! stopped program) holds up only its own writer, never the core's thread.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::{mpsc as std_mpsc, Arc, Mutex};

use inyon_core::hosts::host::{OnData, PtyHandle};
use inyon_core::hosts::http_error::{HubError, HubResult};
use inyon_core::platform::rt::Deferred;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tokio::sync::{mpsc, oneshot};

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }
}

/// Where keys for a terminal go. On Unix not portable-pty's own writer: when that one is dropped it
/// types a newline and Ctrl-D, which a tmux client or `ssh -tt` still on its way out passes to the
/// pane as Enter (the engine's `terminal_input`).
fn terminal_input(master: &dyn MasterPty) -> std::io::Result<Box<dyn Write + Send>> {
    #[cfg(unix)]
    {
        use std::os::fd::BorrowedFd;
        let fd = master.as_raw_fd().ok_or_else(|| std::io::Error::other("no terminal"))?;
        // SAFETY: the master is open for this whole call, and the copy is a descriptor of its own
        let own = unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()?;
        Ok(Box::new(std::fs::File::from(own)))
    }
    #[cfg(not(unix))]
    master.take_writer().map_err(std::io::Error::other)
}

/// Starts `argv` in a pseudo-terminal of `cols`×`rows`, with TERM=xterm-256color and a UTF-8 LANG
/// unless one is set. `on_data` gets its output on the core's thread.
pub fn open(argv: &[String], env: &[(String, String)], cols: u16, rows: u16, mut on_data: OnData) -> HubResult<PtyHandle> {
    let err = |e: &dyn std::fmt::Display| HubError::plain(e.to_string());
    let pair = native_pty_system().openpty(size(cols, rows)).map_err(|e| err(&e))?;
    // portable-pty has no pre_exec hook, so the open-file limit goes back through sh
    #[cfg(unix)]
    let argv = &crate::limits::with_limit_restored(argv);
    let mut cmd = CommandBuilder::new(&argv[0]);
    cmd.args(&argv[1..]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }
    for k in crate::process::DROPPED {
        cmd.env_remove(k);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("LANG", std::env::var("LANG").ok().filter(|l| !l.is_empty()).unwrap_or_else(|| "C.UTF-8".into()));
    let mut child = pair.slave.spawn_command(cmd).map_err(|e| err(&e))?;
    drop(pair.slave);
    let master: Box<dyn MasterPty + Send> = pair.master;
    let mut reader = master.try_clone_reader().map_err(|e| err(&e))?;
    let mut writer = terminal_input(&*master).map_err(|e| err(&e))?;
    let killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>> = Arc::new(Mutex::new(child.clone_killer()));

    // keys, written in order on a thread of their own; it ends when the terminal closes or is killed
    let (keys_tx, keys_rx) = std_mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        for data in keys_rx {
            if writer.write_all(&data).and_then(|()| writer.flush()).is_err() {
                break;
            }
        }
    });

    // output, read on a thread of its own and drawn on the core's
    let (data_tx, mut data_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if data_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    tokio::task::spawn_local(async move {
        while let Some(d) = data_rx.recv().await {
            on_data(d);
        }
    });

    let exited = Deferred::new();
    let (code_tx, code_rx) = oneshot::channel::<i32>();
    std::thread::spawn(move || {
        let code = child.wait().map(|s| s.exit_code() as i32).unwrap_or(1);
        let _ = code_tx.send(code);
    });
    let done = exited.clone();
    tokio::task::spawn_local(async move {
        done.resolve(code_rx.await.unwrap_or(1));
    });

    let master = Rc::new(RefCell::new(Some(master)));
    let (rm, km) = (master.clone(), master);
    let keys = Rc::new(RefCell::new(Some(keys_tx)));
    let kk = keys.clone();
    Ok(PtyHandle::new(
        move |d: &str| {
            if let Some(k) = keys.borrow().as_ref() {
                let _ = k.send(d.as_bytes().to_vec());
            }
        },
        move |c, r| {
            if let Some(m) = rm.borrow().as_ref() {
                let _ = m.resize(size(c, r));
            }
        },
        move || {
            if let Ok(mut k) = killer.lock() {
                let _ = k.kill();
            }
            // closes the terminal; the writer ends with it, whatever it was in the middle of
            kk.borrow_mut().take();
            km.borrow_mut().take();
        },
        exited,
    ))
}
