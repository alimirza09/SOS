#![no_std]
#![no_main]

extern crate alloc;

use bootloader::{BootInfo, entry_point};
use core::panic::PanicInfo;
use sos::println;
use sos::task::{Task, executor::Executor};

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    use sos::allocator;
    use sos::memory::{self, BootInfoFrameAllocator};
    use sos::sshell::shell;
    use sos::vga_buffer::{Color, set_colors};
    use x86_64::VirtAddr;
    set_colors(Color::Green, Color::Black);

    println!("Welcome to sOS{}", "!");
    sos::init();

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe { BootInfoFrameAllocator::init(&boot_info.memory_map) };

    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("heap initialization failed");

    let mut executor = Executor::new();
    executor.spawn(Task::new(shell()));
    executor.run();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    sos::hlt_loop();
}
