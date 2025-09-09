#![no_std]
#![no_main]

extern crate alloc;

use alloc::sync::Arc;
use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use sos::drivers::test_ata_driver;

use core::ptr::addr_of_mut;
use sos::arch::x86_64::smp::{start_one_ap, CPUS, MAX_CPUS};
use sos::drivers::vga_buffer::{set_colors, Color};
use sos::sched::processor::Processor;
use sos::sched::rr::RRScheduler;
use sos::sched::thread_pool::ThreadPool;
use sos::task::{executor::Executor, Task};
use sos::{println, serial_println};

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    set_colors(Color::Green, Color::Black);
    println!("Welcome to sOS!");

    // Initialize basic system first
    println!("Initializing basic system...");
    sos::init();
    println!("Basic system initialized");

    // Initialize memory management
    println!("Initializing memory management...");
    use sos::memory::{allocator, paging, paging::BootInfoFrameAllocator};
    use x86_64::VirtAddr;

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mut frame_allocator = unsafe { BootInfoFrameAllocator::init(&boot_info.memory_map) };
    let mut mapper = unsafe { paging::init(phys_mem_offset, &mut frame_allocator) };
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("Heap initialization failed");
    println!("Memory management initialized");

    // Wait a bit for system to stabilize
    println!("System stabilizing...");
    for _ in 0..1000000 {
        x86_64::instructions::nop();
    }

    // Test basic functionality first
    println!("Testing basic I/O...");

    // Test VGA output
    println!("VGA test passed");

    // Test serial output
    sos::serial_println!("Serial test from kernel");
    println!("Serial test passed");

    // Now test ATA driver with better error handling
    println!("About to test ATA driver...");

    // Add a small delay and try to catch any early faults
    for _ in 0..100000 {
        x86_64::instructions::nop();
    }

    test_ata_driver_safe();

    println!("ATA test completed, starting processors...");
    processors()
}

// Add this safer ATA test function
fn test_ata_driver_safe() {
    println!("Starting safe ATA test...");

    // First, test if we can even access the ports without faulting
    unsafe {
        use x86_64::instructions::port::Port;

        println!("Testing basic port access...");

        // Try to read status port - this is the safest operation
        let mut status_port = Port::<u8>::new(0x1F7);
        let status = status_port.read();
        println!("Primary ATA status port read: 0x{:02X}", status);

        // Test secondary controller status too
        let mut secondary_status_port = Port::<u8>::new(0x177);
        let secondary_status = secondary_status_port.read();
        println!("Secondary ATA status port read: 0x{:02X}", secondary_status);

        // If we get here without crashing, we can access I/O ports
        println!("I/O port access works, proceeding with full ATA test");
        test_ata_driver();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("=== KERNEL PANIC ===");
    serial_println!("PANIC: {}", info);

    // Try to print more detailed panic info
    if let Some(location) = info.location() {
        serial_println!(
            "Panic occurred in file '{}' at line {}",
            location.file(),
            location.line()
        );
    }

    // message() returns PanicMessage directly, not Option
    let message = info.message();
    serial_println!("Panic message: {}", message);

    // Print stack trace if possible
    serial_println!("System halted due to panic - entering infinite loop");

    sos::hlt_loop();
}

fn processors() -> ! {
    use sos::smp::nop;
    println!("Initializing CPU storage...");
    CPUS.init();
    println!("CPUs initialized");

    static mut PROCESSORS: [Processor; MAX_CPUS] = [const { Processor::new() }; MAX_CPUS];

    let processors_ptr: *mut Processor = unsafe { addr_of_mut!(PROCESSORS[0]) as *mut Processor };

    let scheduler = RRScheduler::new(20);
    let pool = Arc::new(ThreadPool::new(scheduler, MAX_CPUS));

    println!("Starting Application Processors...");

    println!("Starting AP #1...");
    start_one_ap(1, 1, pool.clone(), processors_ptr);
    println!("Started AP #1");
    nop(1_000_000);

    println!("Starting AP #2...");
    start_one_ap(2, 2, pool.clone(), processors_ptr);
    println!("Started AP #2");
    nop(1_000_000);

    println!("Starting AP #3...");
    start_one_ap(3, 3, pool.clone(), processors_ptr);
    println!("Started AP #3");
    nop(1_000_000);

    println!("Starting AP #4...");
    start_one_ap(4, 4, pool.clone(), processors_ptr);
    println!("Started AP #4");

    println!("All APs started! Running on {} total CPUs", 5);
    nop(5_000_000);

    // Check which CPUs came online
    for i in 0..5 {
        let cpu = CPUS.get(i);
        if cpu.online.load(core::sync::atomic::Ordering::SeqCst) == 1 {
            println!("CPU {} is online (APIC ID: {})", i, cpu.apic_id);
        } else {
            println!("CPU {} failed to start", i);
        }
    }

    println!("Creating task executor...");
    let mut executor = Executor::new();

    println!("Spawning test tasks...");
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
            // Use nop instead of the missing function
            for _ in 0..1_000_000 {
                x86_64::instructions::nop();
            }
        }
        println!("BSP main task completed");
    }));

    println!("Starting executor...");
    executor.run();
}
