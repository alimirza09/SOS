use crate::alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};
use crate::drivers::ata::{
    fs_create_file, fs_delete_file, fs_list_files, fs_read_file, init_global_filesystem,
};
use crate::serial_println;
use spin::Mutex;

static FD_TABLE: Mutex<BTreeMap<u64, FileDescriptor>> = Mutex::new(BTreeMap::new());
static NEXT_FD: Mutex<u64> = Mutex::new(3);

#[derive(Debug, Clone)]
struct FileDescriptor {
    filename: String,
    mode: FileMode,
    position: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FileMode {
    Read,
    Write,
    ReadWrite,
}

pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_UNLINK: u64 = 4;
pub const SYS_LSEEK: u64 = 5;
pub const SYS_STAT: u64 = 6;
pub const SYS_MKDIR: u64 = 7;
pub const SYS_RMDIR: u64 = 8;

type SyscallFn = fn(u64, u64, u64) -> u64;

static SYSCALLS: &[SyscallFn] = &[
    sys_read, sys_write, sys_open, sys_close, sys_unlink, sys_lseek, sys_stat, sys_mkdir, sys_rmdir,
];

unsafe fn get_string_from_ptr(ptr: u64, max_len: usize) -> Result<String, &'static str> {
    if ptr == 0 {
        return Err("null pointer");
    }

    if ptr < 0x1000 {
        return Err("invalid pointer range");
    }

    let slice = match try_read_user_memory(ptr, max_len) {
        Some(slice) => slice,
        None => return Err("memory access failed"),
    };

    let mut len = 0;
    for &byte in slice {
        if byte == 0 {
            break;
        }
        len += 1;
        if len >= max_len {
            return Err("string too long");
        }
    }

    let str_slice = core::str::from_utf8(&slice[..len]).map_err(|_| "invalid utf8")?;
    Ok(str_slice.to_string())
}

unsafe fn try_read_user_memory(ptr: u64, len: usize) -> Option<&'static [u8]> {
    if ptr == 0 || len == 0 || len > 4096 {
        return None;
    }

    Some(core::slice::from_raw_parts(ptr as *const u8, len))
}

unsafe fn copy_to_user(user_ptr: u64, data: &[u8]) -> Result<usize, &'static str> {
    if user_ptr == 0 {
        return Err("null pointer");
    }

    if user_ptr < 0x1000 {
        return Err("invalid pointer range");
    }

    let user_slice = core::slice::from_raw_parts_mut(user_ptr as *mut u8, data.len());
    user_slice.copy_from_slice(data);
    Ok(data.len())
}

unsafe fn copy_from_user(user_ptr: u64, len: usize) -> Result<Vec<u8>, &'static str> {
    if user_ptr == 0 {
        return Err("null pointer");
    }

    if user_ptr < 0x1000 || len > 1024 * 1024 {
        return Err("invalid pointer or size");
    }

    let user_slice = core::slice::from_raw_parts(user_ptr as *const u8, len);
    Ok(user_slice.to_vec())
}

fn sys_read(fd: u64, buf: u64, count: u64) -> u64 {
    let count = count as usize;

    match fd {
        0 => return 0,
        1 | 2 => return u64::MAX,
        _ => {}
    }

    let fd_table = FD_TABLE.lock();
    let file_desc = match fd_table.get(&fd) {
        Some(desc) => desc.clone(),
        None => {
            serial_println!("sys_read: invalid file descriptor {}", fd);
            return u64::MAX;
        }
    };
    drop(fd_table);

    if file_desc.mode == FileMode::Write {
        serial_println!("sys_read: file not open for reading");
        return u64::MAX;
    }

    let file_data = match fs_read_file(&file_desc.filename) {
        Ok(data) => data,
        Err(e) => {
            serial_println!(
                "sys_read: failed to read file '{}': {}",
                file_desc.filename,
                e
            );
            return u64::MAX;
        }
    };

    let available = file_data.len().saturating_sub(file_desc.position);
    let to_read = count.min(available);

    if to_read == 0 {
        return 0;
    }

    let data_slice = &file_data[file_desc.position..file_desc.position + to_read];
    match unsafe { copy_to_user(buf, data_slice) } {
        Ok(bytes_copied) => {
            let mut fd_table = FD_TABLE.lock();
            if let Some(desc) = fd_table.get_mut(&fd) {
                desc.position += bytes_copied;
            }
            serial_println!(
                "sys_read: read {} bytes from '{}'",
                bytes_copied,
                file_desc.filename
            );
            bytes_copied as u64
        }
        Err(e) => {
            serial_println!("sys_read: failed to copy to user buffer: {}", e);
            u64::MAX
        }
    }
}

fn sys_write(fd: u64, buf: u64, count: u64) -> u64 {
    let count = count as usize;

    match fd {
        0 => return u64::MAX,
        1 | 2 => {
            if let Ok(data) = unsafe { copy_from_user(buf, count) } {
                if let Ok(s) = core::str::from_utf8(&data) {
                    crate::serial_print!("{}", s);
                    return count as u64;
                }
            }
            return u64::MAX;
        }
        _ => {}
    }

    let fd_table = FD_TABLE.lock();
    let file_desc = match fd_table.get(&fd) {
        Some(desc) => desc.clone(),
        None => {
            serial_println!("sys_write: invalid file descriptor {}", fd);
            return u64::MAX;
        }
    };
    drop(fd_table);

    if file_desc.mode == FileMode::Read {
        serial_println!("sys_write: file not open for writing");
        return u64::MAX;
    }

    let data = match unsafe { copy_from_user(buf, count) } {
        Ok(data) => data,
        Err(e) => {
            serial_println!("sys_write: failed to copy from user buffer: {}", e);
            return u64::MAX;
        }
    };

    match fs_create_file(&file_desc.filename, &data) {
        Ok(()) => {
            serial_println!(
                "sys_write: wrote {} bytes to '{}'",
                count,
                file_desc.filename
            );
            count as u64
        }
        Err(e) => {
            serial_println!(
                "sys_write: failed to write file '{}': {}",
                file_desc.filename,
                e
            );
            u64::MAX
        }
    }
}

fn sys_open(filename_ptr: u64, flags: u64, _mode: u64) -> u64 {
    serial_println!(
        "sys_open: attempting to open file at ptr 0x{:x}",
        filename_ptr
    );

    let filename = match unsafe { get_string_from_ptr(filename_ptr, 256) } {
        Ok(name) => {
            serial_println!("sys_open: successfully read filename: '{}'", name);
            name
        }
        Err(e) => {
            serial_println!("sys_open: failed to get filename: {}", e);
            return u64::MAX;
        }
    };

    let mode = match flags & 0x3 {
        0 => FileMode::Read,
        1 => FileMode::Write,
        2 => FileMode::ReadWrite,
        _ => FileMode::Read,
    };

    let mut next_fd = NEXT_FD.lock();
    let fd = *next_fd;
    *next_fd += 1;
    drop(next_fd);

    let mut fd_table = FD_TABLE.lock();
    fd_table.insert(
        fd,
        FileDescriptor {
            filename: filename.clone(),
            mode,
            position: 0,
        },
    );

    serial_println!(
        "sys_open: opened '{}' as fd {} (mode: {:?})",
        filename,
        fd,
        mode
    );
    fd
}

fn sys_close(fd: u64, _unused1: u64, _unused2: u64) -> u64 {
    let mut fd_table = FD_TABLE.lock();
    match fd_table.remove(&fd) {
        Some(desc) => {
            serial_println!("sys_close: closed fd {} ('{}')", fd, desc.filename);
            0
        }
        None => {
            serial_println!("sys_close: invalid file descriptor {}", fd);
            u64::MAX
        }
    }
}

fn sys_unlink(filename_ptr: u64, _unused1: u64, _unused2: u64) -> u64 {
    let filename = match unsafe { get_string_from_ptr(filename_ptr, 256) } {
        Ok(name) => name,
        Err(e) => {
            serial_println!("sys_unlink: failed to get filename: {}", e);
            return u64::MAX;
        }
    };

    match fs_delete_file(&filename) {
        Ok(()) => {
            serial_println!("sys_unlink: deleted '{}'", filename);
            0
        }
        Err(e) => {
            serial_println!("sys_unlink: failed to delete '{}': {}", filename, e);
            u64::MAX
        }
    }
}

fn sys_lseek(fd: u64, offset: u64, whence: u64) -> u64 {
    let mut fd_table = FD_TABLE.lock();
    let desc = match fd_table.get_mut(&fd) {
        Some(desc) => desc,
        None => {
            serial_println!("sys_lseek: invalid file descriptor {}", fd);
            return u64::MAX;
        }
    };

    let new_pos = match whence {
        0 => offset as usize,
        1 => desc.position + offset as usize,
        2 => {
            serial_println!("sys_lseek: SEEK_END not implemented");
            return u64::MAX;
        }
        _ => {
            serial_println!("sys_lseek: invalid whence {}", whence);
            return u64::MAX;
        }
    };

    desc.position = new_pos;
    serial_println!("sys_lseek: fd {} position set to {}", fd, new_pos);
    new_pos as u64
}

fn sys_stat(_filename_ptr: u64, _stat_ptr: u64, _unused: u64) -> u64 {
    serial_println!("sys_stat: not implemented");
    u64::MAX
}

fn sys_mkdir(_path_ptr: u64, _mode: u64, _unused: u64) -> u64 {
    serial_println!("sys_mkdir: not implemented");
    u64::MAX
}

fn sys_rmdir(_path_ptr: u64, _unused1: u64, _unused2: u64) -> u64 {
    serial_println!("sys_rmdir: not implemented");
    u64::MAX
}

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

    if let Err(e) = init_global_filesystem() {
        serial_println!("Failed to initialize filesystem: {}", e);
        return Err("filesystem init failed");
    }

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
    if let Err(e) = test_syscalls_filesystem_fixed() {
        serial_println!("Fixed filesystem syscall test failed: {}", e);
    }
}
