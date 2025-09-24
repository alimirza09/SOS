use crate::{gdt, hlt_loop, println};
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
    AtaPrimary = PIC_1_OFFSET + 14,
    AtaSecondary = PIC_1_OFFSET + 15,
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }

    fn as_usize(self) -> usize {
        usize::from(self.as_u8())
    }
}

pub static PICS: spin::Mutex<ChainedPics> =
    spin::Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt[InterruptIndex::Timer.as_usize()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_usize()].set_handler_fn(keyboard_interrupt_handler);

        idt[InterruptIndex::AtaPrimary.as_usize()].set_handler_fn(ata_primary_interrupt_handler);
        idt[InterruptIndex::AtaSecondary.as_usize()]
            .set_handler_fn(ata_secondary_interrupt_handler);
        idt[0x80].set_handler_fn(syscall_handler);

        idt
    };
}

pub fn init_idt() {
    IDT.load();
}

extern "x86-interrupt" fn syscall_handler(_stack_frame: InterruptStackFrame) {
    let num: u64;
    let a0: u64;
    let a1: u64;
    let a2: u64;

    unsafe {
        core::arch::asm!("mov {}, rax", out(reg) num);
        core::arch::asm!("mov {}, rdi", out(reg) a0);
        core::arch::asm!("mov {}, rsi", out(reg) a1);
        core::arch::asm!("mov {}, rdx", out(reg) a2);
    }

    let result = crate::syscall::syscall_identifier(num, a0, a1, a2);

    unsafe {
        core::arch::asm!("mov rax, {}", in(reg) result);
    }
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use crate::serial_println;
    use x86_64::registers::control::Cr2;

    serial_println!("EXCEPTION: PAGE FAULT");
    serial_println!("Accessed Address: {:?}", Cr2::read());
    serial_println!("Error Code: {:?}", error_code);
    serial_println!("{:#?}", stack_frame);
    hlt_loop();
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };

    crate::task::keyboard::add_scancode(scancode);

    unsafe {
        crate::interrupts::PICS
            .lock()
            .notify_end_of_interrupt(crate::interrupts::InterruptIndex::Keyboard.as_u8());
    }
}

extern "x86-interrupt" fn ata_primary_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use crate::drivers::ata::PRIMARY_ATA;
    unsafe {
        let status = PRIMARY_ATA.lock().status_port.read();
        crate::serial_println!("ATA Primary interrupt: status 0x{:02X}", status);
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::AtaPrimary.as_u8());
    }
}

extern "x86-interrupt" fn ata_secondary_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use crate::drivers::ata::SECONDARY_ATA;
    unsafe {
        let status = SECONDARY_ATA.lock().status_port.read();
        crate::serial_println!("ATA Secondary interrupt: status 0x{:02X}", status);
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::AtaSecondary.as_u8());
    }
}
