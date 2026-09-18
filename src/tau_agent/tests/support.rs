//! Test-only helpers shared by the portable agent modules.

use std::future::Future;
use std::task::{Context, Poll, Waker};

/// Drive an immediately-ready future to completion without an async runtime.
///
/// The portable layer deliberately has no runtime dependency (ADR-010), so the
/// unit tests poll their own futures. `Pending` yields the thread, which is
/// fine for the short, ready futures the tests build; do not use this to drive
/// real I/O.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = Box::pin(future);

    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}
