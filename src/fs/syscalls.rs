use crate::fs::fat;
use alloc::string::String;
use core::ptr;

// Helper to copy a C string from userspace.
pub unsafe fn copy_in_cstr(ptr: u64) -> String {
    let mut buf = alloc::vec::Vec::new();
    let mut p = ptr as *const u8;
    loop {
        let c = ptr::read(p);
        if c == 0 {
            break;
        }
        buf.push(c);
        p = p.add(1);
    }
    String::from_utf8(buf).unwrap_or_default()
}

// Dummy global: only one file "open" at a time
static mut LAST_FILENAME: Option<String> = None;

pub fn sys_open(filename_ptr: u64, write_flag: u64, _unused: u64) -> u64 {
    let filename = unsafe { copy_in_cstr(filename_ptr) };
    unsafe {
        LAST_FILENAME = Some(filename);
    }
    if write_flag != 0 {
        0 // pretend fd 0 is for writing
    } else {
        1 // pretend fd 1 is for reading
    }
}

pub fn sys_read(_fd: u64, buf_ptr: u64, count: u64) -> u64 {
    let filename = unsafe { LAST_FILENAME.as_ref() }
        .cloned()
        .unwrap_or_default();
    let mut temp_buf = alloc::vec::Vec::with_capacity(count as usize);
    temp_buf.resize(count as usize, 0);
    match fat::read_file(&filename, &mut temp_buf[..]) {
        Ok(n) => {
            unsafe {
                ptr::copy_nonoverlapping(temp_buf.as_ptr(), buf_ptr as *mut u8, n);
            }
            n as u64
        }
        Err(_) => u64::MAX,
    }
}

pub fn sys_write(_fd: u64, buf_ptr: u64, count: u64) -> u64 {
    let filename = unsafe { LAST_FILENAME.as_ref() }
        .cloned()
        .unwrap_or_default();
    let buf = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, count as usize) };
    match fat::write_file(&filename, buf) {
        Ok(()) => count,
        Err(_) => u64::MAX,
    }
}

pub fn sys_close(_fd: u64, _a1: u64, _a2: u64) -> u64 {
    // No-op for this simple stub
    0
}

pub fn sys_unlink(filename_ptr: u64, _a1: u64, _a2: u64) -> u64 {
    let filename = unsafe { copy_in_cstr(filename_ptr) };
    fat::remove_file(&filename).is_ok() as u64
}
