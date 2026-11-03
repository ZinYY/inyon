use std::cell::RefCell;
use std::rc::Rc;

use inyon_core::clock::set_now;
use inyon_core::hosts::host::{ExecOptions, HostBackend};
use inyon_core::platform::rt::{yield_now, Deferred};
use inyon_protocol::{HostAuth, HostConfig, HostKind};
use serde_json::json;

use super::*;
use crate::testing::{on_core, FakeSsh, FINGERPRINT};

fn server(auth: HostAuth) -> HostConfig {
    HostConfig {
        id: "box".into(),
        name: "Box".into(),
        kind: HostKind::Ssh,
        target: Some("me@10.0.0.5".into()),
        port: Some(2222),
        host_key: Some(FINGERPRINT.into()),
        auth: Some(auth),
        ..inyon_api::core_routes::empty_config()
    }
}

fn backend(fake: &Rc<FakeSsh>) -> SshBackend {
    SshBackend::new(fake.clone(), Rc::new(|_| Box::pin(async { Ok(Credentials { password: Some("hunter22".into()), ..Default::default() }) })))
}

async fn settle() {
    for _ in 0..20 {
        yield_now().await;
    }
}

#[test]
fn a_command_opens_the_connection_once_and_runs_through_sh() {
    on_core(|| async {
        let fake = FakeSsh::new();
        *fake.exec_answer.borrow_mut() = Some(json!({ "code": 0, "stdout": "hi\n", "stderr": "" }));
        let b = backend(&fake);
        let r = b.exec(&server(HostAuth::Password), "echo 'hi'", ExecOptions::input("in")).await.unwrap();
        assert_eq!((r.code, r.stdout.as_str()), (0, "hi\n"));
        assert_eq!(fake.calls_of("connect"), [json!({ "id": "box", "host": "10.0.0.5", "port": 2222, "username": "me", "hostKey": FINGERPRINT, "password": "hunter22" })]);
        assert_eq!(fake.calls_of("exec")[0], json!({ "id": "box", "command": r#"sh -c 'echo '\''hi'\'''"#, "timeoutMs": 20000, "stdin": "aW4=" }));
        b.exec(&server(HostAuth::Password), "true", ExecOptions::timeout(5)).await.unwrap();
        assert_eq!(fake.calls_of("connect").len(), 1);
        assert_eq!(fake.calls_of("exec")[1]["timeoutMs"], 5);
        // the plugin saying the connection closed: the next command opens a new one
        fake.emit("closed", json!({ "id": "box" }));
        b.exec(&server(HostAuth::Password), "true", ExecOptions::default()).await.unwrap();
        assert_eq!(fake.calls_of("connect").len(), 2);
    });
}

#[test]
fn a_server_that_refuses_is_described_and_left_alone_for_a_while() {
    on_core(|| async {
        let fake = FakeSsh::new();
        fake.fail.borrow_mut().insert("connect", PluginError { code: Some("AUTH_FAILED".into()), message: "denied".into() });
        let b = backend(&fake);
        set_now(Some(1_000_000));
        let r = b.exec(&server(HostAuth::Password), "true", ExecOptions::default()).await.unwrap();
        assert_eq!(r.code, 255);
        assert_eq!(r.stderr, "The server didn't accept the sign-in. Check the user name and the password. (denied)");
        // asked again within 8 seconds: the same answer, without dialing
        set_now(Some(1_007_000));
        assert_eq!(b.exec(&server(HostAuth::Password), "true", ExecOptions::default()).await.unwrap().stderr, r.stderr);
        assert_eq!(fake.calls_of("connect").len(), 1);
        set_now(Some(1_008_000));
        b.exec(&server(HostAuth::Password), "true", ExecOptions::default()).await.unwrap();
        assert_eq!(fake.calls_of("connect").len(), 2);
        // forgetting it (its settings changed) tries at once
        b.forget("box").await;
        b.exec(&server(HostAuth::Password), "true", ExecOptions::default()).await.unwrap();
        assert_eq!(fake.calls_of("connect").len(), 3);
        assert_eq!(fake.calls_of("disconnect"), [json!({ "id": "box" })]);
        set_now(None);
    });
}

#[test]
fn a_server_without_a_user_or_a_confirmed_key_is_never_dialed() {
    on_core(|| async {
        let fake = FakeSsh::new();
        let b = backend(&fake);
        let no_user = HostConfig { target: Some("10.0.0.5".into()), ..server(HostAuth::Password) };
        assert_eq!(b.exec(&no_user, "true", ExecOptions::default()).await.unwrap().stderr, "Add the user name, like you@server");
        let no_key = HostConfig { id: "other".into(), host_key: None, ..server(HostAuth::Password) };
        assert!(b.exec(&no_key, "true", ExecOptions::default()).await.unwrap().stderr.contains("hasn't been confirmed"));
        assert!(fake.calls_of("connect").is_empty());
    });
}

#[test]
fn a_stream_hears_its_lines_without_the_empty_ones_and_ends_with_its_code() {
    on_core(|| async {
        let fake = FakeSsh::new();
        let b = backend(&fake);
        let heard = Rc::new(RefCell::new(Vec::new()));
        let h = heard.clone();
        let s = b.stream(&server(HostAuth::Password), "tail -F x", Box::new(move |l| h.borrow_mut().push(l)), true);
        settle().await;
        let call = fake.calls_of("stream")[0].clone();
        assert_eq!(call, json!({ "id": "box", "streamId": "s1", "command": "sh -c 'tail -F x'", "keepStdin": true }));
        fake.emit("streamLines", json!({ "streamId": "s1", "lines": ["a", "", "b"] }));
        fake.emit("streamLines", json!({ "streamId": "s9", "lines": ["not ours"] }));
        assert_eq!(*heard.borrow(), ["a", "b"]);
        fake.emit("streamExit", json!({ "streamId": "s1", "code": 3 }));
        assert_eq!(s.exited.wait().await, 3);
        // gone from the list: later lines go nowhere
        fake.emit("streamLines", json!({ "streamId": "s1", "lines": ["late"] }));
        assert_eq!(heard.borrow().len(), 2);
    });
}

#[test]
fn a_stream_killed_before_the_plugin_knew_it_is_still_stopped() {
    on_core(|| async {
        let fake = FakeSsh::new();
        let b = backend(&fake);
        // killed while the connection is still opening: it never starts
        let dialing = Deferred::new();
        *fake.dialing.borrow_mut() = Some(dialing.clone());
        let early = b.stream(&server(HostAuth::Password), "a", Box::new(|_| {}), false);
        early.kill();
        *fake.dialing.borrow_mut() = None;
        dialing.resolve(());
        assert_eq!(early.exited.wait().await, 143);
        assert!(fake.calls_of("stream").is_empty());
        // started, then killed: the plugin is told, and its exit event ends it
        let s = b.stream(&server(HostAuth::Password), "b", Box::new(|_| {}), false);
        settle().await;
        s.kill();
        settle().await;
        assert_eq!(fake.calls_of("streamKill"), [json!({ "streamId": "s2" })]);
        fake.emit("streamExit", json!({ "streamId": "s2", "code": 143 }));
        assert_eq!(s.exited.wait().await, 143);
        // a plugin that can't kill it: it counts as ended
        fake.fail.borrow_mut().insert("streamKill", PluginError::plain("gone"));
        let t = b.stream(&server(HostAuth::Password), "c", Box::new(|_| {}), false);
        settle().await;
        t.kill();
        assert_eq!(t.exited.wait().await, 143);
    });
}

#[test]
fn a_terminal_gets_its_output_decoded_and_says_why_it_could_not_open() {
    on_core(|| async {
        let fake = FakeSsh::new();
        let b = backend(&fake);
        let out = Rc::new(RefCell::new(Vec::<u8>::new()));
        let o = out.clone();
        let dialing = Deferred::new();
        *fake.dialing.borrow_mut() = Some(dialing.clone());
        let p = b.pty(&server(HostAuth::Password), "tmux attach", 80, 24, Box::new(move |d| o.borrow_mut().extend(d))).unwrap();
        // typed while the connection is still opening: dropped, as the TS does
        p.write("early");
        *fake.dialing.borrow_mut() = None;
        dialing.resolve(());
        settle().await;
        assert_eq!(fake.calls_of("pty"), [json!({ "id": "box", "ptyId": "p1", "command": "sh -c 'tmux attach'", "cols": 80, "rows": 24 })]);
        p.write("ls\r");
        p.resize(100, 30);
        settle().await;
        assert_eq!(fake.calls_of("ptyWrite"), [json!({ "ptyId": "p1", "data": "ls\r" })]);
        assert_eq!(fake.calls_of("ptyResize"), [json!({ "ptyId": "p1", "cols": 100, "rows": 30 })]);
        fake.emit("ptyData", json!({ "ptyId": "p1", "data": "aGk=" }));
        assert_eq!(*out.borrow(), b"hi");
        fake.emit("ptyExit", json!({ "ptyId": "p1", "code": 0 }));
        assert_eq!(p.exited.wait().await, 0);

        fake.fail.borrow_mut().insert("connect", PluginError { code: Some("UNREACHABLE".into()), message: "timeout".into() });
        let shown = Rc::new(RefCell::new(Vec::<u8>::new()));
        let s = shown.clone();
        let other = HostConfig { id: "far".into(), ..server(HostAuth::Password) };
        let q = b.pty(&other, "x", 80, 24, Box::new(move |d| s.borrow_mut().extend(d))).unwrap();
        assert_eq!(q.exited.wait().await, 255);
        let text = String::from_utf8(shown.borrow().clone()).unwrap();
        assert!(text.starts_with("\r\nCan't reach the server.") && text.ends_with("(timeout)\r\n"), "{text}");
    });
}

#[test]
fn each_refusal_says_what_to_check() {
    let e = |code: &str| PluginError { code: Some(code.into()), message: "m".into() };
    assert!(describe(&e("HOST_KEY_MISMATCH"), None).starts_with("The server's key changed."));
    assert!(describe(&e("AUTH_FAILED"), Some(&server(HostAuth::Key))).contains("that its public key is in ~/.ssh/authorized_keys"));
    assert!(describe(&e("AUTH_FAILED"), Some(&server(HostAuth::PhoneKey))).contains("that this phone's key is in"));
    assert_eq!(describe(&PluginError::plain("plain"), None), "plain");
    assert_eq!(split_target("a@b@c"), ("a@b".to_owned(), "c".to_owned()));
    assert_eq!(split_target("@c"), (String::new(), "@c".to_owned()));
}

#[test]
fn a_sign_in_that_fails_after_the_server_was_edited_doesnt_hold_up_the_new_settings() {
    on_core(|| async {
        let fake = FakeSsh::new();
        fake.fail.borrow_mut().insert("connect", PluginError { code: Some("AUTH_FAILED".into()), message: "denied".into() });
        let dialing = Deferred::new();
        *fake.dialing.borrow_mut() = Some(dialing.clone());
        let b = backend(&fake);
        let first = b.exec(&server(HostAuth::Password), "true", ExecOptions::default());
        inyon_core::platform::rt::spawn(async move {
            let _ = first.await;
        });
        settle().await;
        // edited while it was still signing in; the old attempt fails afterwards
        b.forget("box").await;
        fake.fail.borrow_mut().clear();
        *fake.dialing.borrow_mut() = None;
        dialing.resolve(());
        settle().await;
        let r = b.exec(&server(HostAuth::Password), "true", ExecOptions::default()).await.unwrap();
        assert_eq!(fake.calls_of("connect").len(), 2);
        assert_ne!(r.code, 255, "{}", r.stderr);
    });
}
