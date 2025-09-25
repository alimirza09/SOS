use crate::fs::syscalls::{sys_close, sys_open, sys_read, sys_unlink, sys_write};
use crate::serial_println;

pub const SYS_OPEN: u64 = 0;
pub const SYS_READ: u64 = 1;
pub const SYS_WRITE: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_UNLINK: u64 = 4;

pub const SYSCALLS: &[fn(u64, u64, u64) -> u64] =
    &[sys_open, sys_read, sys_write, sys_close, sys_unlink];

pub fn syscall_identifier(num: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let idx = num as usize;
    if idx < SYSCALLS.len() {
        serial_println!("syscall: {} ({}, {}, {})", num, a0, a1, a2);
        SYSCALLS[idx](a0, a1, a2)
    } else {
        serial_println!("syscall: unknown syscall number {}", num);
        u64::MAX
    }
}

pub fn test_syscalls_filesystem_fixed() -> Result<(), &'static str> {
    serial_println!("=== Fixed Filesystem Syscall Test ===");

    static FILENAME: &[u8] = b"test.txt\0";
    static TEST_CONTENT: &[u8] = b"Hello, filesystem syscalls!\nThis is a test file.\n";

    static mut READ_BUFFER: [u8; 1024] = [0u8; 1024];

    let fd = syscall_identifier(SYS_OPEN, FILENAME.as_ptr() as u64, 1, 0);

    if fd == u64::MAX {
        return Err("failed to open file");
    }
    serial_println!("Opened file with fd: {}", fd);

    let write_ret = syscall_identifier(
        SYS_WRITE,
        fd,
        TEST_CONTENT.as_ptr() as u64,
        TEST_CONTENT.len() as u64,
    );
    serial_println!("Write returned: {}", write_ret);

    let close_ret = syscall_identifier(SYS_CLOSE, fd, 0, 0);
    serial_println!("Close returned: {}", close_ret);

    let read_fd = syscall_identifier(SYS_OPEN, FILENAME.as_ptr() as u64, 0, 0);

    if read_fd == u64::MAX {
        return Err("failed to open file for reading");
    }
    serial_println!("Opened file for reading with fd: {}", read_fd);

    let read_ret = syscall_identifier(
        SYS_READ,
        read_fd,
        unsafe { READ_BUFFER.as_mut_ptr() as u64 },
        unsafe { READ_BUFFER.len() as u64 },
    );
    serial_println!("Read returned: {} bytes", read_ret);

    if read_ret != u64::MAX && read_ret > 0 {
        let bytes_read = read_ret as usize;
        let read_data = unsafe { &READ_BUFFER[..bytes_read] };

        if read_data == TEST_CONTENT {
            serial_println!("✓ File content verification passed!");
        } else {
            serial_println!("✗ File content verification failed");
            serial_println!(
                "Expected: {:?}",
                core::str::from_utf8(TEST_CONTENT).unwrap()
            );
            serial_println!("Got: {:?}", core::str::from_utf8(read_data).unwrap());
            return Err("content verification failed");
        }
    }

    syscall_identifier(SYS_CLOSE, read_fd, 0, 0);

    let unlink_ret = syscall_identifier(SYS_UNLINK, FILENAME.as_ptr() as u64, 0, 0);

    if unlink_ret == 0 {
        serial_println!("✓ File deletion successful");
    } else {
        serial_println!("✗ File deletion failed");
    }

    serial_println!("=== Fixed Filesystem Syscall Test Complete ===");
    Ok(())
}

pub fn test_syscalls() {
    let _ = test_syscalls_filesystem_fixed();
}
