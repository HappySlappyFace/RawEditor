//! What the session changed that a front end draws from state it caches:
//! set by session methods, drained by the front end after each call.

/// A set of [`Changed`] flags.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Changed(u8);

impl Changed {
    /// Nothing.
    pub const NONE: Changed = Changed(0);
    /// What is drawn over the image (crop frame, mask handles, brush): its
    /// geometry or the view it is placed in.
    pub const OVERLAY: Changed = Changed(1);
    /// The histogram's counts.
    pub const HISTOGRAM: Changed = Changed(1 << 1);
    /// The profiler's frame times.
    pub const PROFILER: Changed = Changed(1 << 2);
    /// The image on screen is out of date. Only a front end that draws on
    /// its own clock (`Session::surface_drawn`) needs it: everywhere else the
    /// session starts the render itself.
    pub const FRAME: Changed = Changed(1 << 3);

    pub fn insert(&mut self, other: Changed) {
        self.0 |= other.0;
    }

    pub fn contains(self, other: Changed) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The flags, leaving none set.
    pub fn take(&mut self) -> Changed {
        std::mem::take(self)
    }
}

impl std::ops::BitOr for Changed {
    type Output = Changed;
    fn bitor(self, rhs: Changed) -> Changed {
        Changed(self.0 | rhs.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_combine_and_drain() {
        let mut c = Changed::NONE;
        assert!(c.is_empty());
        c.insert(Changed::OVERLAY);
        c.insert(Changed::HISTOGRAM | Changed::PROFILER);
        assert!(c.contains(Changed::OVERLAY | Changed::PROFILER));
        let taken = c.take();
        assert!(taken.contains(Changed::HISTOGRAM));
        assert!(c.is_empty());
        assert!(!Changed::OVERLAY.contains(Changed::HISTOGRAM));
    }
}
