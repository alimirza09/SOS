use crate::thread_pool::*;
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::cell::UnsafeCell;

#[derive(Default)]
pub struct Processor {
    inner: UnsafeCell<Option<ProcessorInner>>,
}

struct ProcessorInner {
    /// Processor ID
    id: usize,
    /// Current running thread
    thread: Option<(Tid, Box<dyn Context>)>,
    /// The context of
    loop_context: Box<dyn Context>,
    /// Reference to `ThreadPool`
    manager: Arc<ThreadPool>,
}
impl Processor {
    pub const fn new() -> Self {
        Processor {
            inner: UnsafeCell::new(None),
        }
    }
    pub unsafe fn init(&self, id: usize, context: Box<dyn Context>, manager: Arc<ThreadPool>) {
        unsafe {
            *self.inner.get() = Some(ProcessorInner {
                id: id,
                thread: None,
                loop_context: context,
                manager: manager,
            });
        }
    }
    fn inner(&self) -> &mut ProcessorInner {
        unsafe { &mut *self.inner.get() }
            .as_mut()
            .expect("Processor is not initialized")
    }

    pub fn tid(&self) -> Tid {
        self.inner()
            .thread
            .as_ref()
            .map(|(tid, _)| *tid)
            .expect("tid(): no thread is running on this CPU")
    }

    pub fn manager(&self) -> &Arc<ThreadPool> {
        &self.inner().manager
    }

    pub fn yield_now(&self) {
        let inner = self.inner();
        if let Some((tid, mut ctx)) = inner.thread.take() {
            let loop_ctx = &mut inner.loop_context;
            unsafe { ctx.switch_to(&mut **loop_ctx) };
            inner.thread = Some((tid, ctx));
        } else {
            panic!("yield_now() called with no running thread");
        }
    }
    pub fn run_next(&self, cpu_id: usize) {
        let inner = self.inner();
        if let Some((tid, next_ctx)) = inner.manager.run(cpu_id) {
            inner.thread = Some((tid, next_ctx));
            let (_, ctx_ref) = inner.thread.as_mut().unwrap();
            unsafe { inner.loop_context.switch_to(&mut **ctx_ref) };
        }
    }

    pub fn stop_running(&self) {
        let inner = self.inner();
        if let Some((tid, ctx)) = inner.thread.take() {
            inner.manager.stop(tid, ctx);
        }
    }
}
