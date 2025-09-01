#![no_std]
#![no_main]

extern crate alloc;

use alloc::sync::Arc;
use bootloader::{BootInfo, entry_point};
use core::panic::PanicInfo;

use core::ptr::addr_of_mut;
use sos::new::{CPUS, MAX_CPUS, start_one_ap};
use sos::println;
use sos::processor::Processor;
use sos::rr::RRScheduler; // or your actual Scheduler implementation
use sos::task::{Task, executor::Executor};
use sos::thread_pool::ThreadPool;
use sos::vga_buffer::{Color, set_colors};

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    set_colors(Color::Green, Color::Black);
    println!("Welcome to sOS!");

    // ---------------- System init ----------------
    sos::init();

    // ---------------- Memory + Heap ----------------
    use sos::allocator;
    use sos::memory::{self, BootInfoFrameAllocator};
    use x86_64::VirtAddr;

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe { BootInfoFrameAllocator::init(&boot_info.memory_map) };
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("Heap initialization failed");

    // ---------------- CPUs ----------------
    CPUS.init();
    println!("CPUs initialized");

    // ---------------- Processors ----------------
    static mut PROCESSORS: [Processor; MAX_CPUS] = [const { Processor::new() }; MAX_CPUS];

    // get a raw pointer safely without creating a reference
    let processors_ptr: *mut Processor = unsafe { addr_of_mut!(PROCESSORS[0]) as *mut Processor };

    // ---------------- ThreadPool ----------------
    let scheduler = RRScheduler::new(20);
    let pool = Arc::new(ThreadPool::new(scheduler, MAX_CPUS));

    // ---------------- Start AP #1 ----------------
    start_one_ap(1, 1, pool.clone(), processors_ptr);

    // ---------------- Test task ----------------
    let mut executor = Executor::new();
    executor.spawn(Task::new(async {
        println!("Hello from the BSP task!");
        for i in 0..5 {
            println!("BSP iteration {}", i);
            sos::hlt_loop();
        }
    }));

    // ---------------- Scheduler loop ----------------
    executor.run();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    sos::hlt_loop();
}
