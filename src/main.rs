#![no_std]
#![no_main]

extern crate alloc;

use alloc::sync::Arc;
use bootloader::{BootInfo, entry_point};
use core::panic::PanicInfo;

use core::ptr::addr_of_mut;
use sos::arch::x86_64::smp::{CPUS, MAX_CPUS, start_one_ap};
use sos::drivers::vga_buffer::{Color, set_colors};
use sos::println;
use sos::sched::processor::Processor;
use sos::sched::rr::RRScheduler;
use sos::sched::thread_pool::ThreadPool;
use sos::task::{Task, executor::Executor};

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    set_colors(Color::Green, Color::Black);
    println!("Welcome to sOS!");

    sos::init();

    use sos::memory::{allocator, paging, paging::BootInfoFrameAllocator};
    use x86_64::VirtAddr;

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mut frame_allocator = unsafe { BootInfoFrameAllocator::init(&boot_info.memory_map) };
    let mut mapper = unsafe { paging::init(phys_mem_offset, &mut frame_allocator) };
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("Heap initialization failed");

    processors();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    sos::hlt_loop();
}
fn processors() -> ! {
    use sos::smp::nop;
    CPUS.init();
    println!("CPUs initialized");

    static mut PROCESSORS: [Processor; MAX_CPUS] = [const { Processor::new() }; MAX_CPUS];

    let processors_ptr: *mut Processor = unsafe { addr_of_mut!(PROCESSORS[0]) as *mut Processor };

    let scheduler = RRScheduler::new(20);
    let pool = Arc::new(ThreadPool::new(scheduler, MAX_CPUS));

    println!("Starting Application Processors...");

    start_one_ap(1, 1, pool.clone(), processors_ptr);
    println!("Started AP #1");

    nop(1_000_000);

    start_one_ap(2, 2, pool.clone(), processors_ptr);
    println!("Started AP #2");
    nop(1_000_000);

    start_one_ap(3, 3, pool.clone(), processors_ptr);
    println!("Started AP #3");
    nop(1_000_000);

    start_one_ap(4, 4, pool.clone(), processors_ptr);
    println!("Started AP #4");

    println!("All APs started! Running on {} total CPUs", 5);
    nop(5_000_000);

    for i in 0..5 {
        let cpu = CPUS.get(i);
        if cpu.online.load(core::sync::atomic::Ordering::SeqCst) == 1 {
            println!("CPU {} is online (APIC ID: {})", i, cpu.apic_id);
        } else {
            println!("CPU {} failed to start", i);
        }
    }

    let mut executor = Executor::new();

    for task_id in 0..10 {
        executor.spawn(Task::new(async move {
            println!("Task {} starting", task_id);
            for i in 0..3 {
                println!("Task {} iteration {}", task_id, i);
                sos::task::keyboard::read_line().await;
            }
            println!("Task {} completed", task_id);
        }));
    }

    executor.spawn(Task::new(async {
        println!("BSP main task running!");
        for i in 0..5 {
            println!("BSP main iteration {}", i);
            nop(1_000_000);
        }
    }));

    executor.run();
}
