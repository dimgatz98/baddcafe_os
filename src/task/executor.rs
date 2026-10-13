use alloc::sync::Arc;
use alloc::task::Wake;
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Waker};

use super::Task;
use crate::idt;

struct TaskWake {
    ready: AtomicBool,
}

impl Wake for TaskWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.ready.store(true, Ordering::Release);
    }
}

struct TaskEntry {
    task: Task,
    wake: Arc<TaskWake>,
    waker: Waker,
}

/// A single-CPU executor. Wakeups must come from this CPU's tasks or interrupts;
/// another CPU would also need to send an interrupt to wake a halted processor.
pub struct Executor {
    tasks: Vec<TaskEntry>,
}

impl Executor {
    pub const fn new() -> Self {
        Executor { tasks: Vec::new() }
    }

    pub fn spawn(&mut self, task: Task) {
        let wake = Arc::new(TaskWake {
            ready: AtomicBool::new(true),
        });
        let waker = Waker::from(wake.clone());
        self.tasks.push(TaskEntry { task, wake, waker });
    }

    /// Poll each ready task at most once, returning the number polled. A task that
    /// wakes itself gets another turn on the next pass, after its peers.
    pub fn run_ready_tasks(&mut self) -> usize {
        let mut polled = 0;
        self.tasks.retain_mut(|entry| {
            // Clear before polling: a wake during poll must survive for next time.
            if !entry.wake.ready.swap(false, Ordering::AcqRel) {
                return true;
            }
            polled += 1;
            let mut context = Context::from_waker(&entry.waker);
            entry.task.poll(&mut context).is_pending()
        });
        polled
    }

    pub fn run(&mut self) -> ! {
        assert!(idt::are_enabled(), "executor needs interrupts enabled");
        loop {
            self.run_ready_tasks();
            self.sleep_if_idle();
        }
    }

    fn sleep_if_idle(&self) {
        idt::disable();
        if self
            .tasks
            .iter()
            .any(|entry| entry.wake.ready.load(Ordering::Acquire))
        {
            idt::enable();
        } else {
            // STI defers interrupts until after HLT. Keep both instructions in one
            // asm block, with a memory barrier, so no wake can slip before sleep.
            unsafe { asm!("sti", "hlt", options(nostack)) };
        }
    }
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use alloc::vec;
    use core::cell::{Cell, RefCell};
    use core::future::poll_fn;
    use core::task::Poll;

    #[test_case]
    fn pending_tasks_wait_for_a_wake_and_wakes_coalesce() {
        let saved = Rc::new(RefCell::new(None::<Waker>));
        let slot = saved.clone();
        let mut executor = Executor::new();
        executor.spawn(Task::new(poll_fn(move |context| {
            *slot.borrow_mut() = Some(context.waker().clone());
            Poll::Pending
        })));

        assert_eq!(executor.run_ready_tasks(), 1);
        assert_eq!(executor.run_ready_tasks(), 0);
        for _ in 0..1000 {
            saved.borrow().as_ref().unwrap().wake_by_ref();
        }
        assert_eq!(executor.run_ready_tasks(), 1);
        assert_eq!(executor.run_ready_tasks(), 0);
        saved.borrow().as_ref().unwrap().clone().wake();
        assert_eq!(executor.run_ready_tasks(), 1);
    }

    #[test_case]
    fn self_wakes_survive_poll_and_give_peers_a_turn() {
        let order = Rc::new(RefCell::new(Vec::new()));
        let mut executor = Executor::new();
        for id in 0..2 {
            let order = order.clone();
            executor.spawn(Task::new(async move {
                order.borrow_mut().push(id);
                crate::task::yield_now().await;
                order.borrow_mut().push(id);
            }));
        }

        assert_eq!(executor.run_ready_tasks(), 2);
        assert_eq!(*order.borrow(), vec![0, 1]);
        assert_eq!(executor.run_ready_tasks(), 2);
        assert_eq!(*order.borrow(), vec![0, 1, 0, 1]);
        assert!(executor.tasks.is_empty());
    }

    #[test_case]
    fn completed_tasks_drop_and_stale_wakers_cannot_wake_new_tasks() {
        struct DropProbe(Rc<Cell<bool>>);
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }

        let dropped = Rc::new(Cell::new(false));
        let probe = DropProbe(dropped.clone());
        let saved = Rc::new(RefCell::new(None::<Waker>));
        let slot = saved.clone();
        let mut executor = Executor::new();
        executor.spawn(Task::new(poll_fn(move |context| {
            let _ = &probe;
            *slot.borrow_mut() = Some(context.waker().clone());
            context.waker().wake_by_ref();
            Poll::Ready(())
        })));
        assert_eq!(executor.run_ready_tasks(), 1);
        assert!(dropped.get());
        assert!(executor.tasks.is_empty());

        executor.spawn(Task::new(core::future::pending()));
        assert_eq!(executor.run_ready_tasks(), 1);
        saved.borrow().as_ref().unwrap().wake_by_ref();
        assert_eq!(executor.run_ready_tasks(), 0);
        drop(executor);
        saved.borrow_mut().take().unwrap().wake();
    }

    #[test_case]
    fn moving_task_storage_preserves_a_borrow_across_await() {
        let done = Rc::new(Cell::new(false));
        let result = done.clone();
        let mut executor = Executor::new();
        executor.spawn(Task::new(async move {
            let value = 42u64;
            let reference = &value;
            let address = reference as *const u64 as usize;
            crate::task::yield_now().await;
            assert_eq!(*reference, 42);
            assert_eq!(reference as *const u64 as usize, address);
            result.set(true);
        }));
        executor.run_ready_tasks();
        for _ in 0..32 {
            executor.spawn(Task::new(async {}));
        }
        executor.run_ready_tasks();
        assert!(done.get());
        assert!(executor.tasks.is_empty());
    }

    #[test_case]
    fn a_wake_before_sleep_keeps_the_executor_running() {
        let mut executor = Executor::new();
        executor.spawn(Task::new(core::future::pending()));
        assert_eq!(executor.run_ready_tasks(), 1);
        executor.tasks[0].waker.wake_by_ref();
        // With the timer masked, an accidental HLT would make this test time out.
        let mask = idt::without_interrupts(|| unsafe {
            let mask = crate::port::inb(0x21);
            crate::port::outb(0x21, 0xff);
            mask
        });
        executor.sleep_if_idle();
        assert!(idt::are_enabled());
        idt::without_interrupts(|| unsafe { crate::port::outb(0x21, mask) });
        assert_eq!(executor.run_ready_tasks(), 1);
    }
}
