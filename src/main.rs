#![no_std]
#![no_main]

extern crate alloc;

use alloc::sync::Arc;
use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;

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
    serial_println!("Welcome to sOS!");
    let (mut frame_allocator, mut mapper) = sos::init(boot_info);

    if let Some(gpu_dev) = sos::drivers::pci::find_virtio_gpu() {
        serial_println!("Initializing VirtIO-GPU");

        let mut gpu = sos::drivers::pci::VirtioGpu::new(gpu_dev);

        match gpu.init(&mut mapper, &mut frame_allocator) {
            Ok(()) => {
                serial_println!("VirtIO-GPU initialized.");

                let (fb_ptr, width, height) = gpu.get_framebuffer();
                serial_println!("Framebuffer ready: {}x{} at {:p}", width, height, fb_ptr);

                match gpu.refresh_display(&mut mapper, &mut frame_allocator) {
                    Ok(()) => {
                        serial_println!("Display refreshed")
                    }
                    Err(e) => serial_println!("Failed to refresh display: {}", e),
                }
                gpu.debug_and_refresh();
            }
            Err(e) => {
                serial_println!("Failed to initialize VirtIO-GPU: {}", e);
            }
        }
    } else {
        serial_println!("No VirtIO-GPU device found");
    }
    serial_println!("==================================");

    sos::ata::test_ata_driver_comprehensive();
    sos::fs::fat::test_fat32_with_device(sos::ata::AtaDevice::Slave, 131072);
    sos::syscall::test_syscalls();

    serial_println!("Entering an infinite loop.");
    sos::hlt_loop();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("=== KERNEL PANIC ===");
    serial_println!("PANIC: {}", info);

    if let Some(location) = info.location() {
        serial_println!(
            "Panic occurred in file '{}' at line {}",
            location.file(),
            location.line()
        );
    }

    let message = info.message();
    serial_println!("Panic message: {}", message);

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

            for _ in 0..1_000_000 {
                x86_64::instructions::nop();
            }
        }
        println!("BSP main task completed");
    }));

    println!("Starting executor...");
    executor.run();
}
