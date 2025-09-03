use crate::interrupt::no_interrupt;
use crate::processor::*;
use crate::thread_pool::*;
use alloc::boxed::Box;
use core::marker::PhantomData;
use core::time::Duration;
use log::*;

#[unsafe(no_mangle)]
fn processor() -> &'static Processor {
    #[cfg(not(target_os = "uefi"))]
    unimplemented!("thread: Please implement and export `processor`")
}

#[unsafe(no_mangle)]
fn new_kernel_context(_entry: extern "C" fn(usize) -> !, _arg: usize) -> Box<dyn Context> {
    #[cfg(not(target_os = "uefi"))]
    unimplemented!("thread: Please implement and export `new_kernel_context`")
}

pub fn current() -> Thread {
    Thread {
        tid: processor().tid(),
    }
}

pub fn sleep(dur: Duration) {
    let time = dur_to_ticks(dur);
    trace!("sleep: {:?} ticks", time);
    processor().manager().sleep(current().id(), time);
    park();

    fn dur_to_ticks(dur: Duration) -> usize {
        return dur.as_secs() as usize * 100 + dur.subsec_nanos() as usize / 10_000_000;
    }
}

pub fn spawn<F, T>(f: F) -> JoinHandle<T>
where
    F: Send + 'static + FnOnce() -> T,
    T: Send + 'static,
{
    trace!("spawn:");

    let f = Box::into_raw(Box::new(f));

    extern "C" fn kernel_thread_entry<F, T>(f: usize) -> !
    where
        F: Send + 'static + FnOnce() -> T,
        T: Send + 'static,
    {
        let f = unsafe { Box::from_raw(f as *mut F) };
        let ret = Box::new(f());
        let exit_code = Box::into_raw(ret) as usize;
        processor().manager().exit(current().id(), exit_code);
        yield_now();
        unreachable!()
    }

    let context = new_kernel_context(kernel_thread_entry::<F, T>, f as usize);
    let tid = processor().manager().add(context);

    return JoinHandle {
        thread: Thread { tid },
        mark: PhantomData,
    };
}

pub fn yield_now() {
    trace!("yield:");
    no_interrupt(|| {
        processor().yield_now();
    });
}

pub fn park() {
    trace!("park:");
    processor().manager().sleep(current().id(), 0);
    yield_now();
}

pub fn park_action(f: impl FnOnce()) {
    trace!("park:");
    processor().manager().sleep(current().id(), 0);
    f();
    yield_now();
}

pub struct Thread {
    tid: usize,
}

impl Thread {
    pub fn unpark(&self) {
        processor().manager().wakeup(self.tid);
    }
    pub fn id(&self) -> usize {
        self.tid
    }
}

pub struct JoinHandle<T> {
    thread: Thread,
    mark: PhantomData<T>,
}

impl<T> JoinHandle<T> {
    pub fn thread(&self) -> &Thread {
        &self.thread
    }
    pub fn join(self) -> Result<T, ()> {
        loop {
            trace!("try to join thread {}", self.thread.tid);
            if let Some(exit_code) = processor().manager().try_remove(self.thread.tid) {
                core::mem::forget(self);
                return Ok(unsafe { *Box::from_raw(exit_code as *mut T) });
            }
            processor().manager().wait(current().id(), self.thread.tid);
            yield_now();
        }
    }
}

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        processor().manager().detach(self.thread.tid);
    }
}
