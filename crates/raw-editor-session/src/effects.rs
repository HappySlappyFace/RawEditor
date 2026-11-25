//! Work the session asks its host to run.
//!
//! A session method never blocks and never spawns: it changes its state and
//! returns [`Effects`], each a future that resolves to an event (`E`). The
//! host runs them on its executor — the desktop app turns each into an iced
//! `Task` — and hands every event back to the session. The same contract as
//! iced's `Task::perform`/`Task::batch`, without the dependency.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// A boxed, sendable future, as the host's executor needs it.
pub type BoxFuture<E> = Pin<Box<dyn Future<Output = E> + Send + 'static>>;

/// One piece of work: what it is (for logs and tests) and the future itself.
pub struct Effect<E> {
    pub label: &'static str,
    pub future: BoxFuture<E>,
}

/// Work to run, in no particular order (like `Task::batch`). Empty costs no
/// allocation.
#[must_use = "effects do nothing unless the host runs them"]
pub struct Effects<E> {
    items: Vec<Effect<E>>,
}

impl<E> Effects<E> {
    /// Nothing to run.
    pub fn none() -> Self {
        Self { items: Vec::new() }
    }

    /// One future, resolving to the event the session will be handed back.
    pub fn perform(label: &'static str, future: impl Future<Output = E> + Send + 'static) -> Self {
        Self { items: vec![Effect { label, future: Box::pin(future) }] }
    }

    /// All of these.
    pub fn batch(all: impl IntoIterator<Item = Self>) -> Self {
        let mut items = Vec::new();
        for e in all {
            items.extend(e.items);
        }
        Self { items }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// What is asked for, for tests and logs.
    pub fn labels(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.items.iter().map(|e| e.label)
    }
}

impl<E> Default for Effects<E> {
    fn default() -> Self {
        Self::none()
    }
}

impl<E> IntoIterator for Effects<E> {
    type Item = Effect<E>;
    type IntoIter = std::vec::IntoIter<Effect<E>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl<E> fmt::Debug for Effects<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.labels()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::{Context, Poll, Waker};

    fn ready<E>(effect: Effect<E>) -> E {
        let mut future = effect.future;
        match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(e) => e,
            Poll::Pending => panic!("{} did not resolve", effect.label),
        }
    }

    #[test]
    fn none_is_empty_and_batching_keeps_every_effect() {
        let none: Effects<u32> = Effects::none();
        assert!(none.is_empty());
        let all = Effects::batch([
            Effects::perform("a", async { 1 }),
            Effects::none(),
            Effects::perform("b", async { 2 }),
        ]);
        assert_eq!(all.labels().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(format!("{all:?}"), r#"["a", "b"]"#);
        let events: Vec<u32> = all.into_iter().map(ready).collect();
        assert_eq!(events, [1, 2]);
    }
}
