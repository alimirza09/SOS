use crate::serial_println;

type SyscallFn = fn(u64, u64, u64) -> u64;

static SYSCALLS: &[SyscallFn] = &[sys_write];
fn sys_write(fd: u64, buf: u64, len: u64) -> u64 {
    serial_println!("{}", buf);
    0
}

pub fn syscall_identifier(num: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let idx = num as usize;
    if idx < SYSCALLS.len() {
        SYSCALLS[idx](a0, a1, a2)
    } else {
        u64::MAX
    }
}

pub fn test_syscalls() {
    let ret: u64;
    unsafe {
        core::arch::asm!(
            "mov rax, 0",      // syscall number: sys_write
            "mov rdi, 1",      // arg0
            "mov rsi, 0x1234", // arg1
            "mov rdx, 5",      // arg2
            "int 0x80",        // trigger interrupt
            out("rax") ret,    // get return value
        );
    }
    crate::println!("syscall returned {}", ret);
}
