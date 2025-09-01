use alloc::boxed::Box;
use alloc::vec::Vec;
use core::arch::global_asm;
use core::mem::size_of;

/// Low-level saved register state for a context.
/// Layout MUST match the assembly `ctx_switch` implementation below.
#[repr(C)]
pub struct RawContext {
    pub r15: usize,
    pub r14: usize,
    pub r13: usize,
    pub r12: usize,
    pub rbx: usize,
    pub rbp: usize,
    pub rsp: usize, // stored stack pointer (points to where `ret` will pop RIP)
}

const _: () = assert!(size_of::<RawContext>() == 7 * size_of::<usize>());

// Low-level assembly context switch:
// Saves callee-saved registers into `old` and restores them from `new`,
// including swapping `rsp`. Implemented in assembly below.
unsafe extern "C" {
    fn ctx_switch(old: *mut RawContext, new: *const RawContext);
}

global_asm!(
    r#"
    .text
    .global ctx_switch
    .type ctx_switch, @function
// ctx_switch(old: *mut RawContext, new: *const RawContext)
// rdi = old, rsi = new
ctx_switch:
    // Save callee-saved registers into *rdi (old)
    mov [rdi + 0], r15
    mov [rdi + 8], r14
    mov [rdi + 16], r13
    mov [rdi + 24], r12
    mov [rdi + 32], rbx
    mov [rdi + 40], rbp
    mov [rdi + 48], rsp

    // Load callee-saved registers from *rsi (new)
    mov r15, [rsi + 0]
    mov r14, [rsi + 8]
    mov r13, [rsi + 16]
    mov r12, [rsi + 24]
    mov rbx, [rsi + 32]
    mov rbp, [rsi + 40]
    mov rsp, [rsi + 48]

    // Return; execution continues with registers/state of `new`
    ret
"#
);

/// A local trait for contexts used inside this module.
pub trait LocalContext {
    /// Switch from this context (`self`) to `next`.
    /// Safety: this performs an architecture-level register/stack swap.
    unsafe fn switch_to(&mut self, next: &mut dyn LocalContext);
    fn raw_mut_ptr(&mut self) -> *mut RawContext;
    fn raw_ptr(&self) -> *const RawContext;
}

/// Concrete context implementation that owns a stack and a RawContext.
pub struct ContextImpl {
    raw: RawContext,
    // Owned stack memory (grows downward). Requires allocator.
    _stack: Box<[u8]>,
}

impl ContextImpl {
    /// Create a new ContextImpl with `stack_size` bytes and entry function `entry_fn`.
    /// When this context is switched-to the first time, it will start executing `entry_fn`.
    pub fn new_with_entry(stack_size: usize, entry_fn: extern "C" fn() -> !) -> Self {
        // allocate stack on heap
        let mut v = Vec::with_capacity(stack_size);
        // initialize to zeros and set length
        unsafe {
            v.set_len(stack_size);
        }
        let boxed = v.into_boxed_slice();
        let top = boxed.as_ptr() as usize + boxed.len();

        // place entry_fn address as the return RIP on the new stack:
        // new_rsp = top - 8; write entry_fn as usize at new_rsp
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
        // when ctx_switch returns, this context was resumed
    }
    fn raw_mut_ptr(&mut self) -> *mut RawContext {
        &mut self.raw as *mut RawContext
    }
    fn raw_ptr(&self) -> *const RawContext {
        &self.raw as *const RawContext
    }
}

// Idle function the context will run when first started.
extern "C" fn context_loop() -> ! {
    loop {
        // enable interrupts and halt — this is the idle loop
        unsafe {
            core::arch::asm!("sti; hlt", options(nomem, preserves_flags));
        }
    }
}

/// ---- Integration with user's thread_pool::Context trait ----
///
/// We attempt to implement `crate::thread_pool::Context` for `ContextImpl`.
/// This requires the `thread_pool::Context` trait to have a compatible method
/// signature. Based on your earlier Processor code that used `Box<dyn Context>`
/// with `switch_to(&mut self, next: &mut dyn Context)`, we assume the trait
/// is similar. If your actual trait differs, paste it here and I will adapt.
///
/// The `cfg` below ensures we refer to the trait in your crate root.
use crate::thread_pool as tp_mod;

/// If `tp_mod::Context` exists with the assumed signature, implement it for ContextImpl.
/// This block will fail to compile if the trait signature doesn't match; if so, paste
/// the exact trait and I'll adjust.
impl tp_mod::Context for ContextImpl {
    unsafe fn switch_to(&mut self, next: &mut dyn tp_mod::Context) {
        unsafe {
            // We need to coerce `next` (a dyn tp_mod::Context) to our LocalContext representation.
            // Here we rely on the fact that the concrete type is `ContextImpl` (the one we create).
            // So we try to obtain a raw pointer to the underlying ContextImpl's RawContext.
            // This is safe if `next` is actually a `ContextImpl` trait object (which create_loop_context_for_thread_pool guarantees).
            //
            // Trick: transmute the trait object to raw pointers: (data_ptr, vtable_ptr).
            let next_raw = next as *mut dyn tp_mod::Context;
            // SAFETY: we expect `next` to be a `ContextImpl`. Convert trait-object pointer to a raw pointer to ContextImpl:
            let data_ptr = next_raw as *mut ContextImpl;
            // call ctx_switch between self.raw and data_ptr.raw
            ctx_switch(self.raw_mut_ptr(), (&mut (*data_ptr)).raw_ptr());
            // when resumed, returns here
        }
    }
}

/// Create a boxed `ContextImpl` and return it as `*mut dyn thread_pool::Context`.
/// Caller (AP startup) will do `Box::from_raw(...)` and pass it to `Processor::init`.
#[unsafe(no_mangle)]
pub extern "Rust" fn create_loop_context_for_thread_pool() -> *mut dyn tp_mod::Context {
    const STACK_SIZE: usize = 16 * 1024;
    let ctx_impl = ContextImpl::new_with_entry(STACK_SIZE, context_loop);
    // Box as trait object of thread_pool::Context
    let boxed: Box<dyn tp_mod::Context> = Box::new(ctx_impl);
    Box::into_raw(boxed)
}
