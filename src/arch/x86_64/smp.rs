#![allow(dead_code)]

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::mem::{self, MaybeUninit};
use core::sync::atomic::{AtomicU8, Ordering};

use crate::processor::Processor;
use crate::thread_pool::{self, ThreadPool};

const APIC_BASE: usize = 0xFEE0_0000;
const APIC_ICR_LOW: usize = 0x300;
const APIC_ICR_HIGH: usize = 0x310;
const DELIVERY_MODE_INIT: u32 = 0x5 << 8;
const DELIVERY_MODE_STARTUP: u32 = 0x6 << 8;
const LEVEL_ASSERT: u32 = 1 << 14;
const TRIGGER_MODE_LEVEL: u32 = 1 << 15;

const TRAMPOLINE_PADDR: usize = 0x7000;
const TRAMPOLINE_VECTOR: u8 = (TRAMPOLINE_PADDR >> 12) as u8;

pub const MAX_CPUS: usize = 8;

#[repr(C, align(64))]
pub struct CpuInfo {
    pub id: usize,
    pub apic_id: u32,
    pub online: AtomicU8,
    _pad: [u8; 64 - mem::size_of::<usize>() - 4 - 1],
}

pub struct CpuStorage {
    inner: UnsafeCell<[MaybeUninit<CpuInfo>; MAX_CPUS]>,
}

unsafe impl Sync for CpuStorage {}

impl CpuStorage {
    pub const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(unsafe {
                MaybeUninit::<[MaybeUninit<CpuInfo>; MAX_CPUS]>::uninit().assume_init()
            }),
        }
    }

    pub fn init(&self) {
        unsafe {
            let ptr = (*self.inner.get()).as_mut_ptr();
            for i in 0..MAX_CPUS {
                let entry = CpuInfo {
                    id: i,
                    apic_id: 0,
                    online: AtomicU8::new(0),
                    _pad: [0; 64 - mem::size_of::<usize>() - 4 - 1],
                };
                ptr.add(i).write(MaybeUninit::new(entry));
            }
        }
    }

    pub fn get(&self, idx: usize) -> &CpuInfo {
        unsafe { &*((*self.inner.get())[idx].as_ptr()) }
    }

    pub fn get_mut(&self, idx: usize) -> &mut CpuInfo {
        unsafe { &mut *((*self.inner.get())[idx].as_mut_ptr()) }
    }
}

pub static CPUS: CpuStorage = CpuStorage::new();

#[repr(C)]
pub struct ApStartupData {
    pub stack_top: u64,
    pub pml4_phys: u64,
    pub cpu_id: u32,
    pub apic_id: u32,
    pub _reserved: u32,
}

#[unsafe(no_mangle)]
pub static mut AP_STARTUP: ApStartupData = ApStartupData {
    stack_top: 0,
    pml4_phys: 0,
    cpu_id: 0,
    apic_id: 0,
    _reserved: 0,
};

#[repr(align(16))]
#[derive(Clone, Copy)]
pub struct AlignedStack(pub [u8; 16 * 1024]);

pub static mut AP_STACKS: [AlignedStack; MAX_CPUS] = [AlignedStack([0u8; 16 * 1024]); MAX_CPUS];

core::arch::global_asm!(
    "
    .section .boot, \"ax\"
    .org 0x7000
    .global ap_trampoline_phys
ap_trampoline_phys:
    nop
    jmp ap_trampoline_entry
"
);

#[unsafe(no_mangle)]
pub static mut GLOBAL_THREAD_POOL_PTR: *const () = core::ptr::null();

#[unsafe(no_mangle)]
pub static mut PROCESSORS_PTR: *mut Processor = core::ptr::null_mut();

unsafe fn apic_base() -> *mut u32 {
    APIC_BASE as *mut u32
}

fn apic_write(offset: usize, value: u32) {
    unsafe {
        core::ptr::write_volatile(apic_base().add(offset / 4), value);
    }
}

pub fn nop(max: usize) {
    unsafe {
        for _ in 0..max {
            core::arch::asm!("nop", options(nomem, nostack, preserves_flags));
        }
    }
}
fn send_init_sipi(apic_id: u8, vector: u8) {
    apic_write(APIC_ICR_HIGH, (apic_id as u32) << 24);
    apic_write(
        APIC_ICR_LOW,
        DELIVERY_MODE_INIT | LEVEL_ASSERT | TRIGGER_MODE_LEVEL,
    );
    nop(10_000);
    apic_write(APIC_ICR_LOW, DELIVERY_MODE_INIT);
    nop(20_000);

    apic_write(APIC_ICR_HIGH, (apic_id as u32) << 24);
    apic_write(APIC_ICR_LOW, DELIVERY_MODE_STARTUP | (vector as u32));
    nop(20_000);
    apic_write(APIC_ICR_HIGH, (apic_id as u32) << 24);
    apic_write(APIC_ICR_LOW, DELIVERY_MODE_STARTUP | (vector as u32));
}

#[unsafe(no_mangle)]
pub extern "C" fn ap_trampoline_entry() -> ! {
    unsafe {
        let data = &raw const AP_STARTUP as *const ApStartupData;
        let stack_top = (*data).stack_top as usize;
        let cpu_id = (*data).cpu_id as usize;
        let apic_id = (*data).apic_id;

        core::arch::asm!("mov rsp, {}", in(reg) stack_top, options(nostack, preserves_flags));

        let cpu = CPUS.get_mut(cpu_id);
        cpu.apic_id = apic_id;
        cpu.online.store(1, Ordering::SeqCst);

        let cpu_ptr = cpu as *mut CpuInfo as u64;
        let low = cpu_ptr as u32;
        let high = (cpu_ptr >> 32) as u32;
        core::arch::asm!(
            "wrmsr",
            in("ecx") 0xC0000101u32,
            in("eax") low,
            in("edx") high,
        );

        if GLOBAL_THREAD_POOL_PTR.is_null() {
            loop {
                core::arch::asm!("hlt");
            }
        }
        let pool_arc = Arc::from_raw(GLOBAL_THREAD_POOL_PTR as *const ThreadPool);
        core::mem::forget(pool_arc.clone());

        if PROCESSORS_PTR.is_null() {
            loop {
                core::arch::asm!("hlt");
            }
        }
        let procs = &*PROCESSORS_PTR.add(cpu_id);

        let loop_ctx_raw = crate::context::create_loop_context_for_thread_pool();
        if loop_ctx_raw.is_null() {
            loop {
                core::arch::asm!("hlt");
            }
        }
        let loop_ctx_box: Box<dyn thread_pool::Context> = Box::from_raw(loop_ctx_raw);

        procs.init(cpu_id, loop_ctx_box, pool_arc.clone());

        loop {
            procs.run_next(cpu_id);
            core::arch::asm!("hlt");
        }
    }
}

pub fn start_one_ap(
    ap_index: usize,
    apic_id: u32,
    pool: Arc<ThreadPool>,
    procs_ptr: *mut Processor,
) {
    unsafe {
        GLOBAL_THREAD_POOL_PTR = Arc::into_raw(pool.clone()) as *const ();
        PROCESSORS_PTR = procs_ptr;

        let stack_top = (&AP_STACKS[ap_index].0 as *const _ as usize) + AP_STACKS[ap_index].0.len();
        AP_STARTUP.stack_top = stack_top as u64;
        AP_STARTUP.pml4_phys = 0;
        AP_STARTUP.cpu_id = ap_index as u32;
        AP_STARTUP.apic_id = apic_id;

        send_init_sipi(apic_id as u8, TRAMPOLINE_VECTOR);
    }
}
