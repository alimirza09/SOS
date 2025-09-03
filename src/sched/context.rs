use alloc::boxed::Box;
use alloc::vec::Vec;
use core::arch::global_asm;
use core::mem::size_of;

#[repr(C)]
pub struct RawContext {
    pub r15: usize,
    pub r14: usize,
    pub r13: usize,
    pub r12: usize,
    pub rbx: usize,
    pub rbp: usize,
    pub rsp: usize,
}

const _: () = assert!(size_of::<RawContext>() == 7 * size_of::<usize>());

unsafe extern "C" {
    fn ctx_switch(old: *mut RawContext, new: *const RawContext);
}

global_asm!(
    r#"
    .text
    .global ctx_switch
    .type ctx_switch, @function
ctx_switch:
    mov [rdi + 0], r15
    mov [rdi + 8], r14
    mov [rdi + 16], r13
    mov [rdi + 24], r12
    mov [rdi + 32], rbx
    mov [rdi + 40], rbp
    mov [rdi + 48], rsp

    mov r15, [rsi + 0]
    mov r14, [rsi + 8]
    mov r13, [rsi + 16]
    mov r12, [rsi + 24]
    mov rbx, [rsi + 32]
    mov rbp, [rsi + 40]
    mov rsp, [rsi + 48]

    ret
"#
);

pub trait LocalContext {
    unsafe fn switch_to(&mut self, next: &mut dyn LocalContext);
    fn raw_mut_ptr(&mut self) -> *mut RawContext;
    fn raw_ptr(&self) -> *const RawContext;
}

pub struct ContextImpl {
    raw: RawContext,
    _stack: Box<[u8]>,
}

impl ContextImpl {
    pub fn new_with_entry(stack_size: usize, entry_fn: extern "C" fn() -> !) -> Self {
        let mut v = Vec::with_capacity(stack_size);
        unsafe {
            v.set_len(stack_size);
        }
        let boxed = v.into_boxed_slice();
        let top = boxed.as_ptr() as usize + boxed.len();

        let new_rsp = top - core::mem::size_of::<usize>();
        unsafe {
            let ptr = new_rsp as *mut usize;
            ptr.write(entry_fn as usize);
        }

        let raw = RawContext {
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            rbx: 0,
            rbp: 0,
            rsp: new_rsp,
        };

        ContextImpl { raw, _stack: boxed }
    }
}

impl LocalContext for ContextImpl {
    unsafe fn switch_to(&mut self, next: &mut dyn LocalContext) {
        unsafe {
            ctx_switch(self.raw_mut_ptr(), next.raw_ptr());
        }
    }
    fn raw_mut_ptr(&mut self) -> *mut RawContext {
        &mut self.raw as *mut RawContext
    }
    fn raw_ptr(&self) -> *const RawContext {
        &self.raw as *const RawContext
    }
}

extern "C" fn context_loop() -> ! {
    loop {
        unsafe {
            core::arch::asm!("sti; hlt", options(nomem, preserves_flags));
        }
    }
}

use crate::thread_pool as tp_mod;
impl tp_mod::Context for ContextImpl {
    unsafe fn switch_to(&mut self, next: &mut dyn tp_mod::Context) {
        unsafe {
            let next_raw = next as *mut dyn tp_mod::Context;
            let data_ptr = next_raw as *mut ContextImpl;
            ctx_switch(self.raw_mut_ptr(), (&mut (*data_ptr)).raw_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub extern "Rust" fn create_loop_context_for_thread_pool() -> *mut dyn tp_mod::Context {
    const STACK_SIZE: usize = 16 * 1024;
    let ctx_impl = ContextImpl::new_with_entry(STACK_SIZE, context_loop);
    let boxed: Box<dyn tp_mod::Context> = Box::new(ctx_impl);
    Box::into_raw(boxed)
}
