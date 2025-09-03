use alloc::collections::VecDeque;

type Time = usize;

struct Event<T> {
    time: Time,
    data: T,
}

pub struct Timer<T> {
    tick: Time,
    timers: VecDeque<Event<T>>,
}

impl<T: PartialEq> Timer<T> {
    pub fn new() -> Self {
        Timer {
            tick: 0,
            timers: VecDeque::new(),
        }
    }
    pub fn tick(&mut self) {
        self.tick += 1;
    }
    pub fn pop(&mut self) -> Option<T> {
        match self.timers.front() {
            None => return None,
            Some(timer) if timer.time != self.tick => return None,
            _ => {}
        };
        self.timers.pop_front().map(|t| t.data)
    }
    pub fn start(&mut self, time_after: Time, data: T) {
        //debug!("{:?} {:?}", self.tick, time_after);
        let time = self.tick + time_after;
        let event = Event { time, data };
        let mut it = self.timers.iter();
        let mut i: usize = 0;
        loop {
            match it.next() {
                None => break,
                Some(e) if e.time >= time => break,
                _ => {}
            }
            i += 1;
        }
        self.timers.insert(i, event);
    }
    pub fn stop(&mut self, data: T) {
        if let Some(i) = self.timers.iter().position(|t| t.data == data) {
            self.timers.remove(i);
        }
    }
}
