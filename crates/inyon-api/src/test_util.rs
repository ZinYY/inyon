//! What the tests share: the core's runtime on the test's thread.

use std::rc::Rc;

/// Makes this thread the core's (the store and registry are per thread), without running anything.
pub fn core() {
    inyon_core::platform::rt::install(Rc::new(inyon_backends::runtime::TokioRuntime));
}

/// Runs `f` on a current-thread runtime with the core installed.
pub fn block<F: std::future::Future>(f: F) -> F::Output {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    tokio::task::LocalSet::new().block_on(&rt, async {
        core();
        f.await
    })
}
