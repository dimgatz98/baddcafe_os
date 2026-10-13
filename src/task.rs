pub mod executor;
pub mod keyboard;

use alloc::boxed::Box;
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Context, Poll};

pub struct Task {
    future: Pin<Box<dyn Future<Output = ()>>>,
}

impl Task {
    pub fn new(future: impl Future<Output = ()> + 'static) -> Self {
        Task {
            future: Box::pin(future),
        }
    }

    fn poll(&mut self, context: &mut Context<'_>) -> Poll<()> {
        self.future.as_mut().poll(context)
    }
}

/// Give the other tasks a turn even when the current task's input is always ready.
pub async fn yield_now() {
    let mut yielded = false;
    poll_fn(|context| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            context.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await
}
