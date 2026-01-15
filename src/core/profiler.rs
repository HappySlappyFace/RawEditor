use std::collections::{HashMap, VecDeque};
use std::time::Instant;

#[derive(Debug, Clone, Copy, Default)]
pub struct ProfilerFrame {
    pub update_ms: f32,
    pub upload_ms: f32,
    pub render_ms: f32,
    pub total_ms: f32,
}

#[derive(Debug)]
pub struct Profiler {
    pub history: VecDeque<ProfilerFrame>,
    pub active_timers: HashMap<&'static str, Instant>,
    pub capacity: usize,
}

impl Default for Profiler {
    fn default() -> Self {
        Self {
            history: VecDeque::with_capacity(200),
            active_timers: HashMap::new(),
            capacity: 200,
        }
    }
}

impl Profiler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start_frame(&mut self) {
        self.active_timers.clear();
        self.active_timers.insert("frame", Instant::now());
    }

    pub fn start_timer(&mut self, name: &'static str) {
        self.active_timers.insert(name, Instant::now());
    }

    pub fn stop_timer(&mut self, name: &'static str) -> f32 {
        if let Some(start) = self.active_timers.remove(name) {
            start.elapsed().as_secs_f32() * 1000.0
        } else {
            0.0
        }
    }

    pub fn push_frame(&mut self, frame: ProfilerFrame) {
        if self.history.len() >= self.capacity {
            self.history.pop_front();
        }
        self.history.push_back(frame);
    }
}
