//! hosts/coalesce.test.ts

use std::cell::Cell;
use std::rc::Rc;

use super::coalesce;
use crate::test_support::rt::{run, wait};

fn counter(ms: u64) -> (Rc<Cell<u32>>, Rc<dyn Fn()>) {
    let n = Rc::new(Cell::new(0));
    let c = n.clone();
    (n, coalesce(move || c.set(c.get() + 1), ms))
}

#[test]
fn the_first_call_goes_out_at_once() {
    run(|| async {
        let (n, go) = counter(50);
        go();
        assert_eq!(n.get(), 1);
    });
}

#[test]
fn a_burst_becomes_one_more_call_at_the_end_of_the_window() {
    run(|| async {
        let (n, go) = counter(30);
        go();
        go();
        go();
        go();
        assert_eq!(n.get(), 1);
        wait(45).await;
        assert_eq!(n.get(), 2);
        // quiet since: nothing more
        wait(45).await;
        assert_eq!(n.get(), 2);
    });
}

#[test]
fn after_a_quiet_window_the_next_call_is_immediate_again() {
    run(|| async {
        let (n, go) = counter(20);
        go();
        wait(35).await;
        go();
        assert_eq!(n.get(), 2);
    });
}
