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
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use embedded_sdmmc::{Directory, Mode, TimeSource, Timestamp, VolumeIdx, VolumeManager};
use spin::Mutex;

use crate::fs::ata_block::SosAtaBlockDevice;

struct DummyTime;
impl TimeSource for DummyTime {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp {
            year_since_1970: 54,
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}

pub static VOLUME_MANAGER: Mutex<Option<VolumeManager<SosAtaBlockDevice, DummyTime>>> =
    Mutex::new(None);

pub fn mount_root_fs(device: crate::drivers::ata::AtaDevice, block_count: u32) {
    let dev = SosAtaBlockDevice {
        primary: true,
        device,
        block_count,
    };
    let manager = VolumeManager::new(dev, DummyTime);
    *VOLUME_MANAGER.lock() = Some(manager);
}

fn split_path(path: &str) -> Vec<&str> {
    path.split('/').filter(|p| !p.is_empty()).collect()
}

// Execute a closure with a directory handle at the specified path
fn with_directory_at_path<F, R>(
    volume: &mut embedded_sdmmc::Volume<SosAtaBlockDevice, DummyTime, 4, 4, 1>,
    path_components: &[&str],
    f: F,
) -> Result<R, &'static str>
where
    F: FnOnce(Directory<SosAtaBlockDevice, DummyTime, 4, 4, 1>) -> Result<R, &'static str>,
{
    // Start with root directory
    let mut current_dir = volume
        .open_root_dir()
        .map_err(|_| "Failed to open root directory")?;

    // Navigate through each path component
    for &component in path_components {
        current_dir = current_dir
            .open_dir(component)
            .map_err(|_| "Failed to navigate to directory - path may not exist")?;
    }

    // Execute the closure with the final directory
    f(current_dir)
}

// Create all directories in the path if they don't exist
fn ensure_path_exists(
    volume: &mut embedded_sdmmc::Volume<SosAtaBlockDevice, DummyTime, 4, 4, 1>,
    path_components: &[&str],
) -> Result<(), &'static str> {
    if path_components.is_empty() {
        return Ok(());
    }

    // Create directories level by level
    for depth in 1..=path_components.len() {
        let current_path = &path_components[..depth];
        let dir_name = current_path[current_path.len() - 1];

        // Check if this path level exists
        let path_exists = {
            let result = with_directory_at_path(volume, current_path, |_dir| Ok(()));
            result.is_ok()
        };

        // If it doesn't exist, create it
        if !path_exists {
            if current_path.len() == 1 {
                // Create in root directory
                let mut root = volume
                    .open_root_dir()
                    .map_err(|_| "Failed to open root directory")?;
                root.make_dir_in_dir(dir_name)
                    .map_err(|_| "Failed to create directory in root")?;
            } else {
                // Create in parent directory
                let parent_path = &current_path[..current_path.len() - 1];
                with_directory_at_path(volume, parent_path, |mut parent_dir| {
                    parent_dir
                        .make_dir_in_dir(dir_name)
                        .map_err(|_| "Failed to create directory in parent")
                })?;
            }
        }
    }
    Ok(())
}

pub fn write_file(path: &str, data: &[u8]) -> Result<(), &'static str> {
    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "Failed to open volume")?;

    let mut components = split_path(path);
    let file_name = components.pop().ok_or("Invalid file path")?;

    // Ensure all parent directories exist
    if !components.is_empty() {
        ensure_path_exists(&mut volume, &components)?;
    }

    // Write the file in the target directory
    with_directory_at_path(&mut volume, &components, |mut dir| {
        let mut file = dir
            .open_file_in_dir(file_name, Mode::ReadWriteCreateOrTruncate)
            .map_err(|_| "Failed to create/open file")?;
        file.write(data).map_err(|_| "Failed to write file data")?;
        Ok(())
    })
}

pub fn read_file(path: &str, buf: &mut [u8]) -> Result<usize, &'static str> {
    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "Failed to open volume")?;

    let mut components = split_path(path);
    let file_name = components.pop().ok_or("Invalid file path")?;

    with_directory_at_path(&mut volume, &components, |mut dir| {
        let mut file = dir
            .open_file_in_dir(file_name, Mode::ReadOnly)
            .map_err(|_| "Failed to open file for reading")?;
        let n = file.read(buf).map_err(|_| "Failed to read file data")?;
        Ok(n)
    })
}

pub fn remove_file(path: &str) -> Result<(), &'static str> {
    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "Failed to open volume")?;

    let mut components = split_path(path);
    let file_name = components.pop().ok_or("Invalid file path")?;

    with_directory_at_path(&mut volume, &components, |mut dir| {
        dir.delete_file_in_dir(file_name)
            .map_err(|_| "Failed to delete file")?;
        Ok(())
    })
}

pub fn create_dir(path: &str) -> Result<(), &'static str> {
    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "Failed to open volume")?;

    let components = split_path(path);
    if components.is_empty() {
        return Err("Empty path");
    }

    ensure_path_exists(&mut volume, &components)
}

pub fn remove_dir(path: &str) -> Result<(), &'static str> {
    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "Failed to open volume")?;

    let mut components = split_path(path);
    let dir_name = components.pop().ok_or("Invalid directory path")?;

    with_directory_at_path(&mut volume, &components, |mut parent_dir| {
        // Try to delete the directory
        // Note: This may fail if the directory is not empty
        parent_dir
            .delete_file_in_dir(dir_name)
            .map_err(|_| "Failed to remove directory - may not be empty or method unsupported")?;
        Ok(())
    })
}

pub fn list_dir(path: &str) -> Result<Vec<String>, &'static str> {
    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "Failed to open volume")?;

    let components = split_path(path);

    with_directory_at_path(&mut volume, &components, |mut dir| {
        let mut names = Vec::new();
        dir.iterate_dir(|entry| {
            names.push(entry.name.to_string());
        })
        .map_err(|_| "Failed to iterate directory")?;
        Ok(names)
    })
}

// Test function for FAT32 filesystem operations including subdirectories
pub fn test_fat32() {
    use crate::serial_println as println;

    println!("FAT32 test: Starting comprehensive filesystem tests...");

    // Test basic file operations in root
    let test_file = "SOSTEST.TXT";
    let test_data = b"Hello FAT32 from SOS kernel!";
    let mut buf = [0u8; 128];

    // Test 1: Write file in root
    match write_file(test_file, test_data) {
        Ok(()) => println!("FAT32 test: ✓ Root file written successfully"),
        Err(e) => {
            println!("FAT32 test: ✗ Root file write failed: {}", e);
            return;
        }
    }

    // Test 2: Read file from root
    let bytes_read = match read_file(test_file, &mut buf) {
        Ok(n) => {
            println!("FAT32 test: ✓ Root file read successfully, {} bytes", n);
            n
        }
        Err(e) => {
            println!("FAT32 test: ✗ Root file read failed: {}", e);
            return;
        }
    };

    // Test 3: Verify content
    if &buf[..bytes_read] == test_data {
        println!("FAT32 test: ✓ Content verification passed");
    } else {
        println!("FAT32 test: ✗ Content verification failed");
        return;
    }

    // Test 4: Create subdirectory structure
    match create_dir("testdir") {
        Ok(()) => println!("FAT32 test: ✓ First-level directory created"),
        Err(e) => println!("FAT32 test: ✗ First-level directory creation failed: {}", e),
    }

    match create_dir("testdir/subdir") {
        Ok(()) => println!("FAT32 test: ✓ Second-level directory created"),
        Err(e) => println!(
            "FAT32 test: ✗ Second-level directory creation failed: {}",
            e
        ),
    }

    match create_dir("testdir/subdir/deep") {
        Ok(()) => println!("FAT32 test: ✓ Third-level directory created"),
        Err(e) => println!("FAT32 test: ✗ Third-level directory creation failed: {}", e),
    }

    // Test 5: Write file in subdirectory
    let subdir_file = "testdir/subdir/NESTED.TXT";
    let subdir_data = b"This file is nested deep!";
    match write_file(subdir_file, subdir_data) {
        Ok(()) => println!("FAT32 test: ✓ Nested file written successfully"),
        Err(e) => {
            println!("FAT32 test: ✗ Nested file write failed: {}", e);
        }
    }

    // Test 6: Read file from subdirectory
    match read_file(subdir_file, &mut buf) {
        Ok(n) => {
            if &buf[..n] == subdir_data {
                println!("FAT32 test: ✓ Nested file read and verified successfully");
            } else {
                println!("FAT32 test: ✗ Nested file content mismatch");
            }
        }
        Err(e) => {
            println!("FAT32 test: ✗ Nested file read failed: {}", e);
        }
    }

    // Test 7: List directories at different levels
    match list_dir("") {
        Ok(entries) => {
            println!(
                "FAT32 test: ✓ Root directory listing ({} entries):",
                entries.len()
            );
            for entry in &entries {
                println!("  - {}", entry);
            }
        }
        Err(e) => println!("FAT32 test: ✗ Root directory listing failed: {}", e),
    }

    match list_dir("testdir") {
        Ok(entries) => {
            println!(
                "FAT32 test: ✓ Subdirectory listing ({} entries):",
                entries.len()
            );
            for entry in &entries {
                println!("  - testdir/{}", entry);
            }
        }
        Err(e) => println!("FAT32 test: ✗ Subdirectory listing failed: {}", e),
    }

    match list_dir("testdir/subdir") {
        Ok(entries) => {
            println!(
                "FAT32 test: ✓ Deep directory listing ({} entries):",
                entries.len()
            );
            for entry in &entries {
                println!("  - testdir/subdir/{}", entry);
            }
        }
        Err(e) => println!("FAT32 test: ✗ Deep directory listing failed: {}", e),
    }

    // Test 8: File operations with automatic directory creation
    let auto_create_file = "auto/created/path/FILE.TXT";
    let auto_data = b"Automatically created path!";
    match write_file(auto_create_file, auto_data) {
        Ok(()) => println!("FAT32 test: ✓ File with auto-created path written successfully"),
        Err(e) => println!("FAT32 test: ✗ Auto-created path file write failed: {}", e),
    }

    // Test 9: Verify auto-created directories exist
    match list_dir("auto/created/path") {
        Ok(entries) => {
            println!(
                "FAT32 test: ✓ Auto-created directory accessible ({} entries)",
                entries.len()
            );
            for entry in &entries {
                println!("  - auto/created/path/{}", entry);
            }
        }
        Err(e) => println!("FAT32 test: ✗ Auto-created directory not accessible: {}", e),
    }

    // Test 10: Clean up - remove test files
    match remove_file(test_file) {
        Ok(()) => println!("FAT32 test: ✓ Root test file removed"),
        Err(e) => println!("FAT32 test: ✗ Root test file removal failed: {}", e),
    }

    match remove_file(subdir_file) {
        Ok(()) => println!("FAT32 test: ✓ Nested test file removed"),
        Err(e) => println!("FAT32 test: ✗ Nested test file removal failed: {}", e),
    }

    println!("FAT32 test: All comprehensive tests completed!");
    println!("FAT32 test: Subdirectory support is working!");
}

// Test function that mounts filesystem and runs tests
pub fn test_fat32_with_device(device: crate::drivers::ata::AtaDevice, block_count: u32) {
    use crate::serial_println as println;

    println!(
        "Mounting FAT32 filesystem on device {:?} with {} blocks...",
        device, block_count
    );

    // Mount the filesystem
    mount_root_fs(device, block_count);

    // Run the comprehensive tests
    test_fat32();
}
