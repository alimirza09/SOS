use x86_64::{disable_and_store, restore};
mod x86_64 {

    use core::arch::asm;

    pub fn disable_and_store() -> usize {
        let rflags: usize;
        unsafe {
            asm!(
                "pushfq",
                "pop {}",
                "cli",
                out(reg) rflags,
                options(nomem, preserves_flags),
            );
        }
        rflags & (1 << 9)
    }

    pub fn restore(flags: usize) {
        unsafe {
            if flags != 0 {
                asm!("sti", options(nomem, preserves_flags));
            }
        }
    }
}

pub fn no_interrupt<T>(f: impl FnOnce() -> T) -> T {
    let flags = disable_and_store();
    let ret = f();
    restore(flags);
    ret
}
