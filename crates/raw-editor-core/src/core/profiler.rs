use std::collections::VecDeque;

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
    pub capacity: usize,
}

impl Default for Profiler {
    fn default() -> Self {
        Self {
            history: VecDeque::with_capacity(200),
            capacity: 200,
        }
    }
}

impl Profiler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_frame(&mut self, frame: ProfilerFrame) {
        if self.history.len() >= self.capacity {
            self.history.pop_front();
        }
        self.history.push_back(frame);
    }
}
