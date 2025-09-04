use crate::{print, println};
use conquer_once::spin::OnceCell;
use core::{
    pin::Pin,
    task::{Context, Poll},
};
use crossbeam_queue::ArrayQueue;
use futures_util::{
    stream::{Stream, StreamExt},
    task::AtomicWaker,
};
use lazy_static::lazy_static;
use pc_keyboard::{DecodedKey, HandleControl, Keyboard, ScancodeSet1, layouts};
use spin::Mutex;

pub static SCANCODE_QUEUE: OnceCell<ArrayQueue<u8>> = OnceCell::uninit();
static WAKER: AtomicWaker = AtomicWaker::new();
const KEYBUFFER_SIZE: usize = 1024; // buffer can hold 128 chars
lazy_static! {
    pub static ref SCANCODES: ScancodeStream = ScancodeStream::new();
}

pub struct RingBuffer {
    buffer: [Option<char>; KEYBUFFER_SIZE],
    head: usize,
    tail: usize,
}

impl RingBuffer {
    pub const fn new() -> Self {
        Self {
            buffer: [None; KEYBUFFER_SIZE],
            head: 0,
            tail: 0,
        }
    }

    pub fn push(&mut self, c: char) {
        let next = (self.head + 1) % KEYBUFFER_SIZE;
        if next != self.tail {
            self.buffer[self.head] = Some(c);
            self.head = next;
        }
        // TODO: else
    }

    pub fn pop(&mut self) -> Option<char> {
        if self.head == self.tail {
            None // empty
        } else {
            let c = self.buffer[self.tail];
            self.buffer[self.tail] = None;
            self.tail = (self.tail + 1) % KEYBUFFER_SIZE;
            c
        }
    }
}
lazy_static! {
    pub static ref KEYBUFFER: Mutex<RingBuffer> = Mutex::new(RingBuffer::new());
}

pub(crate) fn add_scancode(scancode: u8) {
    if let Ok(queue) = SCANCODE_QUEUE.try_get() {
        if let Err(_) = queue.push(scancode) {
            println!("WARNING: scancode queue full; dropping keyboard input");
        } else {
            WAKER.wake();
        }
    } else {
        println!("WARNING: scancode queue uninitialized");
    }
}

pub async fn read_line() -> Option<char> {
    let mut scancodes = SCANCODES.clone();
    let mut keyboard = Keyboard::new(
        ScancodeSet1::new(),
        layouts::Us104Key,
        HandleControl::Ignore,
    );

    while let Some(scancode) = scancodes.next().await {
        if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
            if let Some(key) = keyboard.process_keyevent(key_event) {
                match key {
                    DecodedKey::Unicode(character) => {
                        KEYBUFFER.lock().push(character);
                        return Some(character);
                    }
                    DecodedKey::RawKey(_) => (),
                }
            }
        }
    }
    None
}
#[derive(Debug, Clone, Copy)]
pub struct ScancodeStream {
    _private: (),
}

impl ScancodeStream {
    pub fn new() -> Self {
        SCANCODE_QUEUE
            .try_init_once(|| ArrayQueue::new(1024))
            .expect("ScancodeStream::new should only be called once");
        ScancodeStream { _private: () }
    }
}

impl Stream for ScancodeStream {
    type Item = u8;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<u8>> {
        let queue = SCANCODE_QUEUE
            .try_get()
            .expect("scancode queue not initialized");

        if let Some(scancode) = queue.pop() {
            return Poll::Ready(Some(scancode));
        }

        WAKER.register(&cx.waker());
        match queue.pop() {
            Some(scancode) => {
                WAKER.take();
                Poll::Ready(Some(scancode))
            }
            None => Poll::Pending,
        }
    }
}

pub async fn print_keypresses() {
    let mut scancodes = ScancodeStream::new();
    let mut keyboard = Keyboard::new(
        ScancodeSet1::new(),
        layouts::Us104Key,
        HandleControl::Ignore,
    );

    while let Some(scancode) = scancodes.next().await {
        if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
            if let Some(key) = keyboard.process_keyevent(key_event) {
                match key {
                    DecodedKey::Unicode(character) => print!("{}", character),
                    DecodedKey::RawKey(key) => print!("{:?}", key),
                }
            }
        }
    }
}

pub fn wait_for_keypress() -> char {
    loop {
        if let Some(c) = KEYBUFFER.lock().pop() {
            return c;
        }
        x86_64::instructions::hlt();
    }
}
