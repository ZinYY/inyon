//! hosts/tidy.test.ts

use super::*;
use crate::test_support::fake::Fake;
use crate::test_support::rt::run;
use crate::test_support::{host, sh, TempDir};

const PHONE: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPhone inyon-phone";

fn home(keys: &str) -> TempDir {
    let h = TempDir::new("inyon-keys-");
    h.write(".ssh/authorized_keys", keys);
    sh(&format!("chmod 700 '{0}/.ssh' && chmod 600 '{0}/.ssh/authorized_keys'", h.s()), None, &[]);
    h
}

fn drop_key(h: &TempDir) -> i32 {
    sh(&drop_authorized_key(PHONE), None, &[("HOME", &h.s())]).0
}

#[test]
fn removing_a_server_takes_exactly_the_phones_line_out_of_authorized_keys_and_keeps_its_permissions() {
    let h = home(&format!("ssh-ed25519 AAAAmine me@laptop\n{PHONE}\nssh-rsa AAAAwork work@ci\n"));
    assert_eq!(drop_key(&h), 0);
    assert_eq!(h.read(".ssh/authorized_keys"), "ssh-ed25519 AAAAmine me@laptop\nssh-rsa AAAAwork work@ci\n");
    assert_eq!(h.mode(".ssh/authorized_keys"), 0o600);
    assert!(!h.exists(".ssh/authorized_keys.inyon-tmp"));
}

#[test]
fn a_file_without_the_phones_line_is_left_exactly_as_it_is_and_so_is_a_line_that_only_looks_like_it() {
    let near = format!("{PHONE} extra\nfrom=\"10.0.0.1\" {PHONE}\n");
    let h = home(&near);
    drop_key(&h);
    assert_eq!(h.read(".ssh/authorized_keys"), near);
}

#[test]
fn the_only_key_in_the_file_can_come_out_too() {
    let h = home(&format!("{PHONE}\n"));
    drop_key(&h);
    assert_eq!(h.read(".ssh/authorized_keys"), "");
}

#[test]
fn a_file_whose_last_line_has_no_newline_after_it_loses_the_line_all_the_same() {
    // the phone's line last, unterminated: how `echo >>` used to leave it after another such line
    let h = home(&format!("ssh-ed25519 AAAAmine me@laptop\n{PHONE}"));
    assert_eq!(drop_key(&h), 0);
    assert_eq!(h.read(".ssh/authorized_keys"), "ssh-ed25519 AAAAmine me@laptop\n");
    // and another key's line last, unterminated, which keeps its place
    let h2 = home(&format!("{PHONE}\nssh-rsa AAAAwork work@ci"));
    drop_key(&h2);
    assert_eq!(h2.read(".ssh/authorized_keys"), "ssh-rsa AAAAwork work@ci\n");
}

#[test]
fn a_machine_that_answers_loses_its_uploads_and_prompt_log_and_the_connection_to_it_is_closed() {
    run(|| async {
        let f = Fake::install(|_, _, _| crate::test_support::fake::ok("{}"));
        let state = TidyState { status: HostStatus::Online, integration: Some(IntegrationState::Missing), agy_integration: None };
        tidy_machine(&host("srv", "ssh"), Some(state), "echo extra;").await;
        let cmds = f.cmds();
        let rm = cmds.iter().find(|c| c.contains("rm -rf")).unwrap();
        assert!(rm.contains(".inyon/uploads"));
        assert!(rm.contains(".inyon/events.jsonl"));
        assert!(rm.contains("echo extra;"));
        assert_eq!(f.closed.get(), 1);
    });
}

#[test]
fn a_machine_that_is_away_isnt_asked_anything_but_its_connection_still_closes() {
    run(|| async {
        let f = Fake::install(|_, _, _| crate::test_support::fake::ok("{}"));
        let state = TidyState { status: HostStatus::Error, integration: Some(IntegrationState::Installed), agy_integration: None };
        tidy_machine(&host("srv", "ssh"), Some(state), "").await;
        assert!(f.cmds().is_empty());
        assert_eq!(f.closed.get(), 1);
    });
}

#[test]
fn a_machine_that_never_answers_is_given_up_on_after_20_seconds() {
    run(|| async {
        crate::hosts::host::set_host_backends(|_| std::rc::Rc::new(Silent) as std::rc::Rc<dyn crate::hosts::host::HostBackend>);
        let t0 = crate::clock::now_ms();
        let state = TidyState { status: HostStatus::Online, integration: Some(IntegrationState::Installed), agy_integration: None };
        tidy_machine(&host("srv", "ssh"), Some(state), "").await;
        assert_eq!(crate::clock::now_ms() - t0, 20000);
    });
}

/// A machine whose commands never come back.
struct Silent;
impl crate::hosts::host::HostBackend for Silent {
    fn exec(
        &self,
        _: &inyon_protocol::HostConfig,
        _: &str,
        _: ExecOptions,
    ) -> futures_util::future::LocalBoxFuture<'static, crate::hosts::http_error::HubResult<crate::hosts::host::ExecResult>> {
        Box::pin(futures_util::future::pending())
    }
    fn stream(&self, _: &inyon_protocol::HostConfig, _: &str, _: crate::hosts::host::OnLine, _: bool) -> crate::hosts::host::StreamHandle {
        unreachable!()
    }
    fn pty(&self, _: &inyon_protocol::HostConfig, _: &str, _: u16, _: u16, _: crate::hosts::host::OnData) -> crate::hosts::http_error::HubResult<crate::hosts::host::PtyHandle> {
        unreachable!()
    }
}
