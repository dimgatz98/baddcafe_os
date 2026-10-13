use core::future::poll_fn;
use core::task::{Context, Poll, Waker};

use spin::Mutex;

use crate::{idt, keyboard::Keyboard, print, serial_println};

const QUEUE_CAPACITY: usize = 128;
const INPUT_BUDGET: usize = 32;

struct State {
    bytes: [u8; QUEUE_CAPACITY],
    head: usize,
    len: usize,
    overflow: bool,
    claimed: bool,
    waker: Option<Waker>,
}

struct ScancodeInput {
    state: Mutex<State>,
}

impl ScancodeInput {
    const fn new() -> Self {
        ScancodeInput {
            state: Mutex::new(State {
                bytes: [0; QUEUE_CAPACITY],
                head: 0,
                len: 0,
                overflow: false,
                claimed: false,
                waker: None,
            }),
        }
    }

    fn take(&self) -> Option<Scancodes<'_>> {
        idt::without_interrupts(|| {
            let mut state = self.state.lock();
            if state.claimed {
                return None;
            }
            state.claimed = true;
            Some(Scancodes { input: self })
        })
    }

    fn push(&self, scancode: u8) {
        let waker = idt::without_interrupts(|| {
            let mut state = self.state.lock();
            if state.len == QUEUE_CAPACITY {
                // A lost release/prefix invalidates decoder state. Discard the
                // old batch and report the gap before delivering newer bytes.
                state.head = 0;
                state.len = 0;
                state.overflow = true;
            } else {
                let tail = (state.head + state.len) % QUEUE_CAPACITY;
                state.bytes[tail] = scancode;
                state.len += 1;
            }
            state.waker.take()
        });
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn poll_next(&self, context: &mut Context<'_>) -> Poll<Result<u8, InputOverflow>> {
        // Waker callbacks, including clone and drop, run outside the queue lock.
        let mut next_waker = Some(context.waker().clone());
        let (result, old_waker) = idt::without_interrupts(|| {
            let mut state = self.state.lock();
            let result = if state.overflow {
                state.overflow = false;
                Poll::Ready(Err(InputOverflow))
            } else if state.len != 0 {
                let byte = state.bytes[state.head];
                state.head = (state.head + 1) % QUEUE_CAPACITY;
                state.len -= 1;
                Poll::Ready(Ok(byte))
            } else {
                Poll::Pending
            };
            // Checking for input and registering its waiter are one critical
            // section: an interrupt cannot arrive between them.
            let old_waker = core::mem::replace(
                &mut state.waker,
                if result.is_pending() {
                    next_waker.take()
                } else {
                    None
                },
            );
            (result, old_waker)
        });
        drop(old_waker);
        result
    }
}

static INPUT: ScancodeInput = ScancodeInput::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputOverflow;

/// The sole consumer of a scancode queue. Borrowing it mutably for `next` prevents
/// two outstanding reads from overwriting each other's wake registration.
pub struct Scancodes<'a> {
    input: &'a ScancodeInput,
}

impl Scancodes<'_> {
    pub async fn next(&mut self) -> Result<u8, InputOverflow> {
        poll_fn(|context| self.input.poll_next(context)).await
    }
}

impl Drop for Scancodes<'_> {
    fn drop(&mut self) {
        let waker = idt::without_interrupts(|| {
            let mut state = self.input.state.lock();
            state.claimed = false;
            state.waker.take()
        });
        drop(waker);
    }
}

pub fn take_scancodes() -> Option<Scancodes<'static>> {
    INPUT.take()
}

pub(crate) fn add_scancode(scancode: u8) {
    INPUT.push(scancode);
}

pub async fn print_keypresses(mut scancodes: Scancodes<'_>) {
    let mut keyboard = Keyboard::new();
    loop {
        for _ in 0..INPUT_BUDGET {
            match scancodes.next().await {
                Ok(scancode) => {
                    if let Some(character) = keyboard.decode(scancode) {
                        print!("{}", character);
                    }
                }
                Err(InputOverflow) => {
                    keyboard = Keyboard::new();
                    serial_println!("Keyboard input overflow; decoder reset");
                }
            }
        }
        super::yield_now().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use alloc::task::Wake;
    use core::future::Future;
    use core::pin::pin;
    use core::sync::atomic::{AtomicUsize, Ordering};

    struct WakeCount(AtomicUsize);

    impl Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn counter() -> (Arc<WakeCount>, Waker) {
        let count = Arc::new(WakeCount(AtomicUsize::new(0)));
        let waker = Waker::from(count.clone());
        (count, waker)
    }

    #[test_case]
    fn buffered_input_and_ring_wrap_preserve_order() {
        let input = ScancodeInput::new();
        input.push(0x1e);
        let mut receiver = input.take().unwrap();
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(
            pin!(receiver.next()).poll(&mut context),
            Poll::Ready(Ok(0x1e))
        );
        for round in 0..4 {
            for byte in 0..100 {
                input.push(byte + round);
            }
            for byte in 0..100 {
                assert_eq!(
                    pin!(receiver.next()).poll(&mut context),
                    Poll::Ready(Ok(byte + round))
                );
            }
        }
        assert!(pin!(receiver.next()).poll(&mut context).is_pending());
    }

    #[test_case]
    fn input_after_pending_wakes_the_latest_waiter() {
        let input = ScancodeInput::new();
        let mut receiver = input.take().unwrap();
        let (first, first_waker) = counter();
        let (second, second_waker) = counter();
        let mut next = pin!(receiver.next());
        assert!(
            next.as_mut()
                .poll(&mut Context::from_waker(&first_waker))
                .is_pending()
        );
        assert!(
            next.as_mut()
                .poll(&mut Context::from_waker(&second_waker))
                .is_pending()
        );
        input.push(0x30);
        assert_eq!(first.0.load(Ordering::Relaxed), 0);
        assert_eq!(second.0.load(Ordering::Relaxed), 1);
        assert_eq!(
            next.as_mut().poll(&mut Context::from_waker(&second_waker)),
            Poll::Ready(Ok(0x30))
        );
        input.push(0xb0);
        assert_eq!(second.0.load(Ordering::Relaxed), 1);
    }

    #[test_case]
    fn overflow_reports_a_gap_then_accepts_new_input() {
        let input = ScancodeInput::new();
        let mut receiver = input.take().unwrap();
        let mut context = Context::from_waker(Waker::noop());
        for _ in 0..QUEUE_CAPACITY {
            input.push(0x2a);
        }
        input.push(0xaa);
        input.push(0x1e);
        assert_eq!(
            pin!(receiver.next()).poll(&mut context),
            Poll::Ready(Err(InputOverflow))
        );
        assert_eq!(
            pin!(receiver.next()).poll(&mut context),
            Poll::Ready(Ok(0x1e))
        );
        assert!(pin!(receiver.next()).poll(&mut context).is_pending());
    }

    #[test_case]
    fn dropping_the_consumer_releases_its_waker_and_claim() {
        let input = ScancodeInput::new();
        let (count, waker) = counter();
        {
            let mut receiver = input.take().unwrap();
            assert!(input.take().is_none());
            assert!(
                pin!(receiver.next())
                    .poll(&mut Context::from_waker(&waker))
                    .is_pending()
            );
            assert_eq!(Arc::strong_count(&count), 3);
        }
        assert_eq!(Arc::strong_count(&count), 2);
        input.push(0x20);
        assert_eq!(count.0.load(Ordering::Relaxed), 0);
        let mut receiver = input.take().unwrap();
        assert_eq!(
            pin!(receiver.next()).poll(&mut Context::from_waker(&waker)),
            Poll::Ready(Ok(0x20))
        );
    }

    #[test_case]
    fn cancelling_a_read_keeps_input_for_the_next_read() {
        let input = ScancodeInput::new();
        let mut receiver = input.take().unwrap();
        let (_, waker) = counter();
        let mut context = Context::from_waker(&waker);
        {
            let mut read = pin!(receiver.next());
            assert!(read.as_mut().poll(&mut context).is_pending());
        }
        input.push(0x12);
        assert_eq!(
            pin!(receiver.next()).poll(&mut context),
            Poll::Ready(Ok(0x12))
        );
    }

    #[test_case]
    fn queue_operations_preserve_disabled_interrupts() {
        let input = ScancodeInput::new();
        idt::without_interrupts(|| {
            let mut receiver = input.take().unwrap();
            input.push(0x1e);
            assert_eq!(
                pin!(receiver.next()).poll(&mut Context::from_waker(Waker::noop())),
                Poll::Ready(Ok(0x1e))
            );
            drop(receiver);
            assert!(!idt::are_enabled());
        });
        assert!(idt::are_enabled());
    }

    #[test_case]
    fn a_busy_keyboard_yields_after_each_input_budget() {
        let input = ScancodeInput::new();
        for _ in 0..INPUT_BUDGET * 2 {
            input.push(0x9e);
        }
        let receiver = input.take().unwrap();
        let (count, waker) = counter();
        let mut context = Context::from_waker(&waker);
        let mut task = pin!(print_keypresses(receiver));
        assert!(task.as_mut().poll(&mut context).is_pending());
        assert_eq!(count.0.load(Ordering::Relaxed), 1);
        assert!(task.as_mut().poll(&mut context).is_pending());
        assert_eq!(count.0.load(Ordering::Relaxed), 2);
        assert!(task.as_mut().poll(&mut context).is_pending());
        assert_eq!(count.0.load(Ordering::Relaxed), 2);
        input.push(0x9e);
        assert_eq!(count.0.load(Ordering::Relaxed), 3);
    }

    #[test_case]
    fn overflow_resets_a_shift_whose_release_was_lost() {
        let input = ScancodeInput::new();
        let receiver = input.take().unwrap();
        let mut task = pin!(print_keypresses(receiver));
        let mut context = Context::from_waker(Waker::noop());
        input.push(0x2a);
        assert!(task.as_mut().poll(&mut context).is_pending());
        for _ in 0..QUEUE_CAPACITY {
            input.push(0x9e);
        }
        input.push(0xaa);
        input.push(0x1e);
        crate::vga_buffer::WRITER.lock().clear_screen();
        assert!(task.as_mut().poll(&mut context).is_pending());
        // The first cell of the last VGA text row contains the decoded letter.
        let cell = (0xb8000 + 24 * 160) as *const u8;
        assert_eq!(unsafe { cell.read_volatile() }, b'a');
    }
}
