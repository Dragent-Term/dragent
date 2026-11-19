//! Thread-safe lifecycle for expensive, lazily loaded resources.
//!
//! The crate deliberately has no runtime or model dependencies. A desktop app,
//! daemon, plugin, or test can use the same primitive for an embedding model,
//! language server, database cache, or any other resource that should be shared
//! while active and released after an idle period.

use std::fmt;
use std::ops::Deref;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

/// A clonable owner for one lazily initialized resource.
pub struct LazyResource<T> {
    core: Arc<Core<T>>,
}

struct Core<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
}

struct State<T> {
    value: Option<Arc<T>>,
    loading: bool,
    last_used: Instant,
}

/// A lease keeps the resource resident for the duration of an operation.
/// Dropping the final operation lease makes it eligible for idle eviction.
pub struct ResourceLease<T> {
    value: Arc<T>,
    owner: Weak<Core<T>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireError {
    Timeout,
    Load(String),
}

impl fmt::Display for AcquireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("timed out waiting for resource initialization"),
            Self::Load(message) => write!(f, "resource initialization failed: {message}"),
        }
    }
}

impl std::error::Error for AcquireError {}

impl<T> Default for LazyResource<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for LazyResource<T> {
    fn clone(&self) -> Self {
        Self {
            core: self.core.clone(),
        }
    }
}

impl<T> LazyResource<T> {
    pub fn new() -> Self {
        Self {
            core: Arc::new(Core {
                state: Mutex::new(State {
                    value: None,
                    loading: false,
                    last_used: Instant::now(),
                }),
                ready: Condvar::new(),
            }),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.core.state.lock().unwrap().value.is_some()
    }

    pub fn is_loading(&self) -> bool {
        self.core.state.lock().unwrap().loading
    }

    /// Acquire the shared resource, initializing it exactly once when absent.
    /// Concurrent callers wait on a condition variable rather than polling or
    /// starting duplicate heavyweight loads.
    pub fn acquire_with<F, E>(
        &self,
        timeout: Duration,
        loader: F,
    ) -> Result<ResourceLease<T>, AcquireError>
    where
        F: FnOnce() -> Result<T, E>,
        E: fmt::Display,
    {
        let deadline = Instant::now() + timeout;
        let mut loader = Some(loader);

        loop {
            let mut state = self.core.state.lock().unwrap();
            if let Some(value) = state.value.clone() {
                state.last_used = Instant::now();
                return Ok(ResourceLease {
                    value,
                    owner: Arc::downgrade(&self.core),
                });
            }

            if !state.loading {
                state.loading = true;
                drop(state);

                let result = loader
                    .take()
                    .expect("loader consumed before initialization")();
                let mut state = self.core.state.lock().unwrap();
                state.loading = false;
                match result {
                    Ok(value) => {
                        let value = Arc::new(value);
                        state.value = Some(value.clone());
                        state.last_used = Instant::now();
                        self.core.ready.notify_all();
                        return Ok(ResourceLease {
                            value,
                            owner: Arc::downgrade(&self.core),
                        });
                    }
                    Err(error) => {
                        self.core.ready.notify_all();
                        return Err(AcquireError::Load(error.to_string()));
                    }
                }
            }

            let now = Instant::now();
            if now >= deadline {
                return Err(AcquireError::Timeout);
            }
            let (next, wait) = self.core.ready.wait_timeout(state, deadline - now).unwrap();
            state = next;
            if wait.timed_out() && state.value.is_none() {
                return Err(AcquireError::Timeout);
            }
            drop(state);
        }
    }

    /// Evict a resident resource only when it has been idle long enough and no
    /// operation currently holds a lease. Returns whether an eviction occurred.
    pub fn evict_if_idle(&self, idle_for: Duration) -> bool {
        let mut state = self.core.state.lock().unwrap();
        let eligible = !state.loading
            && state.last_used.elapsed() >= idle_for
            && state
                .value
                .as_ref()
                .is_some_and(|value| Arc::strong_count(value) == 1);
        if eligible {
            state.value = None;
        }
        eligible
    }

    /// Immediately evict when no operation is using the resource.
    pub fn clear_if_unused(&self) -> bool {
        self.evict_if_idle(Duration::ZERO)
    }
}

impl<T> Deref for ResourceLease<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> Drop for ResourceLease<T> {
    fn drop(&mut self) {
        if let Some(core) = self.owner.upgrade() {
            core.state.lock().unwrap().last_used = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn concurrent_acquires_only_load_once() {
        let resource = LazyResource::new();
        let loads = Arc::new(AtomicUsize::new(0));
        let mut threads = Vec::new();
        for _ in 0..8 {
            let resource = resource.clone();
            let loads = loads.clone();
            threads.push(std::thread::spawn(move || {
                let lease = resource
                    .acquire_with(Duration::from_secs(2), || {
                        loads.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(20));
                        Ok::<_, String>(42)
                    })
                    .unwrap();
                assert_eq!(*lease, 42);
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn active_lease_prevents_eviction() {
        let resource = LazyResource::new();
        let lease = resource
            .acquire_with(Duration::from_secs(1), || Ok::<_, String>("model"))
            .unwrap();
        assert!(!resource.clear_if_unused());
        drop(lease);
        assert!(resource.clear_if_unused());
        assert!(!resource.is_ready());
    }
}
