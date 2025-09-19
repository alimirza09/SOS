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
    sos::init();

    use sos::memory::{allocator, paging, paging::BootInfoFrameAllocator};
    use x86_64::VirtAddr;

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mut frame_allocator = unsafe { BootInfoFrameAllocator::init(&boot_info.memory_map) };
    let mut mapper = unsafe { paging::init(phys_mem_offset, &mut frame_allocator) };
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("Heap initialization failed");

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

use sos::drivers::{ata::*, clear_screen};
use sos::serial_print;

fn test_ata_driver_safe() {
    println!("Starting comprehensive ATA test...");

    unsafe {
        use x86_64::instructions::port::Port;

        println!("Testing basic port access...");

        let mut status_port = Port::<u8>::new(0x1F7);
        let status = status_port.read();
        println!("Primary ATA status port read: 0x{:02X}", status);

        let mut secondary_status_port = Port::<u8>::new(0x177);
        let secondary_status = secondary_status_port.read();
        println!("Secondary ATA status port read: 0x{:02X}", secondary_status);

        println!("I/O port access works, proceeding with comprehensive ATA test");
        test_ata_driver_comprehensive();
    }
}

pub fn test_ata_driver_comprehensive() {
    crate::serial_println!("=== COMPREHENSIVE ATA DRIVER TEST START ===");

    let devices_to_test = [
        ("Primary Master", AtaDevice::Master, true),
        ("Primary Slave", AtaDevice::Slave, true),
        ("Secondary Master", AtaDevice::Master, false),
        ("Secondary Slave", AtaDevice::Slave, false),
    ];

    let mut found_devices = 0;

    for (name, device, use_primary) in devices_to_test.iter() {
        crate::serial_println!("Checking {}...", name);

        let controller = if *use_primary {
            &mut PRIMARY_ATA.lock()
        } else {
            &mut SECONDARY_ATA.lock()
        };

        match controller.identify(*device) {
            Ok(identify_data) => {
                found_devices += 1;
                let info = identify_data;

                crate::serial_println!("{} found:", name);
                crate::serial_println!("  Model: {}", info.model);
                crate::serial_println!("  Serial: {}", info.serial);
                crate::serial_println!("  Firmware: {}", info.firmware);
                crate::serial_println!("  Sectors: {}", info.sectors);
                crate::serial_println!(
                    "  Capacity: {} MB ({} GB)",
                    info.capacity_mb(),
                    info.capacity_gb()
                );
                crate::serial_println!("  LBA48 Support: {}", info.supports_lba48);
                crate::serial_println!("  Sector Size: {} bytes", info.sector_size);

                crate::println!("{}: {} - {} MB", name, info.model, info.capacity_mb());

                if info.sectors > 0 {
                    test_read_sectors(name, controller, *device);
                }
            }
            Err(e) => {
                crate::serial_println!("{} error: {:?}", name, e);
            }
        }
    }

    crate::println!("Found {} ATA devices total", found_devices);
    crate::serial_println!("=== COMPREHENSIVE ATA DRIVER TEST COMPLETE ===");
}

fn test_read_sectors(name: &str, controller: &mut AtaController, device: AtaDevice) {
    let mut buffer = [0u8; 512];

    match { controller.read_sectors(device, 0, 1, &mut buffer) } {
        Ok(()) => {
            serial_println!("Successfully read sector 0 from {}", name);

            if buffer[510] == 0x55 && buffer[511] == 0xAA {
                serial_println!("Valid MBR signature found");
                println!("{} MBR: Valid", name);
            } else {
                println!("{} MBR: Invalid or missing", name);
            }

            serial_print!("First 32 bytes of sector 0: ");
            for i in 0..32 {
                if i % 16 == 0 {
                    serial_println!();
                    serial_print!("{:04X}: ", i);
                }
                serial_print!("{:02X} ", buffer[i]);
            }
            serial_println!();

            serial_print!("As ASCII: ");
            for i in 0..32 {
                let c = buffer[i];
                if c >= 0x20 && c <= 0x7E {
                    serial_print!("{}", c as char);
                } else {
                    serial_print!(".");
                }
            }
            serial_println!();
        }
        Err(e) => {
            serial_println!("Error reading sector from {}: {:?}", name, e);
        }
    }
}
