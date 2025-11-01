use crate::SignalId;
use std::fmt::{Debug, Display};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::sync::{RwLock, RwLockReadGuard, TryLockError};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum ErrorSource {
    /// Error coming from [`Signaled`] `value`.
    Value,
    /// Error coming from [`Signaled`] `signals`.
    Signals,
    /// Error coming from [`Signaled`] `throttle_instant`.
    ThrottleInstant,
    /// Error coming from [`Signaled`] `throttle_duration`.
    ThrottleDuration,
    /// Error coming from [`Signal`] `callback`.
    SignalCallback,
    /// Error coming from [`Signal`] `trigger`.
    SignalTrigger,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum SignaledError {
    /// Tried to acquire a poisoned lock.
    PoisonedLock { source: ErrorSource },
    /// Tried to acquire a lock that is held elsewhere.
    WouldBlock { source: ErrorSource },
    /// The provided `SignalId` does not correspond to any [`Signal`].
    InvalidSignalId { id: SignalId },
}

static SIGNAL_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generates a new [`SignalId`] for a [`Signal`] using the global identifier counter.
///
/// # Panics
///
/// Panics in debug builds if the identifier counter overflows (`u64::MAX`).
pub fn new_signal_id() -> SignalId {
    let id = SIGNAL_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    debug_assert!(id != u64::MAX, "SignalId counter overflow");
    id
}

pub type CallbackSync<T> = dyn Fn(&T, &T) + Send + Sync + 'static;
pub type SignalCallbackSync<T> = Arc<RwLock<Arc<CallbackSync<T>>>>;
pub type TriggerSync<T> = dyn Fn(&T, &T) -> bool + Send + Sync + 'static;
pub type SignalTriggerSync<T> = Arc<RwLock<Arc<TriggerSync<T>>>>;

/// A signal that executes a callback when a [`Signaled`] value changes, if its trigger condition is met.
///
/// Signals have a unique `id`, a priority for ordering execution, and a trigger function to
/// conditionally execute the callback.
///
/// # Examples
///
/// ```
/// use signaled::{signal_sync, sync::{Signal, Signaled}};
///
/// let signal = signal_sync!(|old: &i32, new: &i32| println!("Old: {} | New: {}", old, new));
/// let signaled = Signaled::new(0);
/// signaled.add_signal(signal).unwrap();
/// signaled.set(42).unwrap(); // Prints "Old: 0 | New: 42"
/// ```
pub struct Signal<T: Send + Sync + 'static> {
    /// Function to run when the [`Signal`] is emitted and the trigger condition is met.
    callback: SignalCallbackSync<T>,
    /// Function that decides if the callback will be invoked or not when the [`Signal`] is emitted.
    trigger: SignalTriggerSync<T>,
    /// Identifier for the [`Signal`].
    id: u64,
    /// Number used in the [`Signaled`] struct to decide the order of execution of the signals.
    priority: AtomicU64,
    /// Boolean representing if the [`Signal`] should be removed from the [`Signaled`] after its callback is successfully invoked once. The removal only occurs if the signal's trigger condition is met during emission.
    once: AtomicBool,
    /// Boolean representing if the [`Signal`] should not invoke the callback when emitted.
    mute: AtomicBool,
}

impl<T: Send + Sync + 'static> Signal<T> {
    /// Creates a new [`Signal`] instance.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function to execute when the signal is emitted.
    /// * `trigger` -  Function that returns a boolean representing if the `callback` will be invoked or not.
    /// * `priority` - Number representing the priority in which the [`Signal`] will be emitted from the parent [`Signaled`].
    /// * `once` - Boolean representing if the [`Signal`] will be removed from the parent [`Signaled`] `signals` after being emitted once. The removal only occurs if the signal's trigger condition is met during emission.
    /// * `mute` - Boolean representing if the `callback` will be invoked or not.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{signal_sync, sync::{Signaled, Signal}};
    ///
    /// let signal = signal_sync!(|old: &i32, new: &i32| println!("Old: {} | New: {}", old, new));
    /// let signaled = Signaled::new(5);
    /// signaled.add_signal(signal).unwrap();
    /// signaled.set(6).unwrap(); // Prints "Old: 5 | New: 6"
    /// ```
    pub fn new<F, G>(callback: F, trigger: G, priority: u64, once: bool, mute: bool) -> Self
    where
        F: Fn(&T, &T) + Send + Sync + 'static,
        G: Fn(&T, &T) -> bool + Send + Sync + 'static,
    {
        Signal {
            callback: Arc::new(RwLock::new(Arc::new(callback))),
            trigger: Arc::new(RwLock::new(Arc::new(trigger))),
            id: new_signal_id(),
            priority: AtomicU64::new(priority),
            once: AtomicBool::new(once),
            mute: AtomicBool::new(mute),
        }
    }

    /// Emits the signal, executing the callback if the trigger condition is met.
    ///
    /// # Arguments
    ///
    /// * `old` - The old `val` of the [`Signaled`].
    /// * `new` - The new `val` of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `callback` or `trigger` locks are poisoned.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{signal_sync, sync::{Signal}};
    ///
    /// let signal = signal_sync!(|old: &i32, new: &i32| println!("Old: {}, New: {}", old, new));
    /// signal.set_trigger(|old: &i32, new: &i32| *new > *old + 5).unwrap();
    /// signal.emit(&4, &6).unwrap(); // Doesn't print because trigger condition is not met
    /// signal.emit(&4, &10).unwrap(); // Prints "Old: 4, New: 10"
    /// ```
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signal::try_emit`].
    pub fn emit(&self, old: &T, new: &T) -> Result<(), SignaledError> {
        if self.mute.load(Ordering::Relaxed) {
            return Ok(());
        }
        let trigger = self
            .trigger
            .read()
            .map_err(|_| SignaledError::PoisonedLock {
                source: ErrorSource::SignalTrigger,
            })?;
        if trigger(old, new) {
            let callback = self
                .callback
                .read()
                .map_err(|_| SignaledError::PoisonedLock {
                    source: ErrorSource::SignalCallback,
                })?;
            callback(old, new);
        }
        Ok(())
    }

    /// Emits the signal, executing the callback if the trigger condition is met.
    ///
    /// This function unlike [`Signal::emit`], is non-blocking so there are no re-entrant calls that block until the `trigger` and `callback` locks can be acquired.
    ///
    /// # Arguments
    ///
    /// * `old` - The old `val` of the [`Signaled`].
    /// * `new` - The new `val` of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `callback` or `trigger` locks are poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `callback` or `trigger` locks are held elsewhere.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{signal_sync, sync::{Signal}};
    ///
    /// let signal = signal_sync!(|old: &i32, new: &i32| println!("Old: {}, New: {}", old, new));
    /// signal.set_trigger(|old: &i32, new: &i32| *new > *old + 5).unwrap();
    /// signal.try_emit(&4, &6).unwrap(); // Doesn't print because trigger condition is not met
    /// signal.try_emit(&4, &10).unwrap(); // Prints "Old: 4, New: 10"
    /// ```
    pub fn try_emit(&self, old: &T, new: &T) -> Result<(), SignaledError> {
        if self.mute.load(Ordering::Relaxed) {
            return Ok(());
        }
        let trigger = self.trigger.try_read().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::SignalTrigger,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::SignalTrigger,
            },
        })?;
        if trigger(old, new) {
            let callback = self.callback.try_read().map_err(|e| match e {
                TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                    source: ErrorSource::SignalCallback,
                },
                TryLockError::WouldBlock => SignaledError::WouldBlock {
                    source: ErrorSource::SignalCallback,
                },
            })?;
            callback(old, new);
        }
        Ok(())
    }

    /// Sets a new callback for the [`Signal`].
    ///
    /// # Arguments
    ///
    /// * `callback` - The new callback function.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `callback` lock is poisoned.
    ///     
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signal::try_set_callback`].
    pub fn set_callback<F: Fn(&T, &T) + Send + Sync + 'static>(
        &self,
        callback: F,
    ) -> Result<(), SignaledError> {
        let mut lock = self
            .callback
            .write()
            .map_err(|_| SignaledError::PoisonedLock {
                source: ErrorSource::SignalCallback,
            })?;
        *lock = Arc::new(callback);
        Ok(())
    }

    /// Sets a new callback for the [`Signal`].
    ///
    /// This function unlike [`Signal::set_callback`], is non-blocking so there are no re-entrant calls that block until the `callback` lock can be acquired.
    ///
    /// # Arguments
    ///
    /// * `callback` - The new callback function.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `callback` lock is poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `callback` lock is held elsewhere.
    pub fn try_set_callback<F: Fn(&T, &T) + Send + Sync + 'static>(
        &self,
        callback: F,
    ) -> Result<(), SignaledError> {
        let mut lock = self.callback.try_write().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::SignalCallback,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::SignalCallback,
            },
        })?;
        *lock = Arc::new(callback);
        Ok(())
    }

    /// Sets a new trigger for the [`Signal`].
    ///
    /// Default trigger always returns `true`.
    ///
    /// # Arguments
    ///
    /// * `trigger` - The new trigger function.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `trigger` lock is poisoned.
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signal::try_set_trigger`]
    pub fn set_trigger<F: Fn(&T, &T) -> bool + Send + Sync + 'static>(
        &self,
        trigger: F,
    ) -> Result<(), SignaledError> {
        let mut lock = self
            .trigger
            .write()
            .map_err(|_| SignaledError::PoisonedLock {
                source: ErrorSource::SignalTrigger,
            })?;
        *lock = Arc::new(trigger);
        Ok(())
    }

    /// Sets a new trigger for the [`Signal`].
    ///
    /// This function unlike [`Signal::set_trigger`], is non-blocking so there are no re-entrant calls that block until the `trigger` lock can be acquired.
    ///
    /// Default trigger always returns `true`.
    ///
    /// # Arguments
    ///
    /// * `trigger` - The new trigger function.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `trigger` lock is poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `trigger` lock is held elsewhere.
    pub fn try_set_trigger<F: Fn(&T, &T) -> bool + Send + Sync + 'static>(
        &self,
        trigger: F,
    ) -> Result<(), SignaledError> {
        let mut lock = self.trigger.try_write().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::SignalTrigger,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::SignalTrigger,
            },
        })?;
        *lock = Arc::new(trigger);
        Ok(())
    }

    /// Sets the [`Signal`] `trigger` to always return true.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `trigger` lock is poisoned.
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative see [`Signal::try_remove_trigger`]
    pub fn remove_trigger(&self) -> Result<(), SignaledError> {
        let mut lock = self
            .trigger
            .write()
            .map_err(|_| SignaledError::PoisonedLock {
                source: ErrorSource::SignalTrigger,
            })?;
        *lock = Arc::new(|_, _| true);
        Ok(())
    }

    /// Sets the [`Signal`] `trigger` to always return true.
    ///
    /// This function unlike [`Signal::remove_trigger`], is non-blocking so there are no re-entrant calls that block until the `trigger` lock can be acquired.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `trigger` lock is poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `trigger` lock is held elsewhere.
    pub fn try_remove_trigger(&self) -> Result<(), SignaledError> {
        let mut lock = self.trigger.try_write().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::SignalTrigger,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::SignalTrigger,
            },
        })?;
        *lock = Arc::new(|_, _| true);
        Ok(())
    }

    /// Sets the execution priority of the [`Signal`].
    ///
    /// Default priority is 0.
    ///
    /// # Arguments
    ///
    /// * `priority` The new priority number, bigger number means higher priority.
    pub fn set_priority(&self, priority: u64) {
        self.priority.store(priority, Ordering::Relaxed);
    }

    /// Sets the `once` flag of the [`Signal`].
    pub fn set_once(&self, is_once: bool) {
        self.once.store(is_once, Ordering::Relaxed);
    }

    /// Sets the `mute` flag of the [`Signal`].
    pub fn set_mute(&self, is_mute: bool) {
        self.mute.store(is_mute, Ordering::Relaxed);
    }

    /// Combines `N` amount of [`Signal`]s returning a single combined [`Signal`] instance.
    ///
    /// The order in which each `callback` will be called depends on the order that the [`Signal`]s are passed into the argument's slice.
    ///
    /// * `callback` will invoke all callbacks from the combined [`Signal`]s.
    ///
    /// * `trigger` will combine all triggers only returning `true` if every `trigger` does so.
    ///
    /// * `priority` will be the highest `priority` of the provided `signals`.
    ///
    /// * `mute` and `once` will be `false` by default.
    ///
    /// * `id` will be a new unique [`SignalId`].
    ///
    /// # Arguments
    ///
    /// * `signals` A slice containing the [`Signal`]s that will be combined into a single [`Signal`] instance.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if any of the provided [`Signal`] instances `callback` or `trigger` locks are poisoned.
    ///
    /// # Examples
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use signaled::{signal_sync, sync::{Signal}};
    ///
    /// let calls = Arc::new(Mutex::new(0));
    ///
    /// let calls_clone = Arc::clone(&calls);
    /// let signal_a = signal_sync!(move |_, _| {
    ///     let mut lock = calls_clone.lock().unwrap();
    ///     *lock = *lock + 1;
    /// });
    ///
    /// let calls_clone = Arc::clone(&calls);
    /// let signal_b = signal_sync!(move |_, _| {
    ///     let mut lock = calls_clone.lock().unwrap();
    ///     *lock = *lock + 2;
    /// });
    ///
    /// let signal_c = Signal::combine(&[signal_a, signal_b]).unwrap();
    ///
    /// signal_c.emit(&(), &()).unwrap();
    /// assert_eq!(*calls.lock().unwrap(), 3); // `signal_a` increases calls by 1 and `signal_b` increases calls by 2 so the `combined` signal increases calls by 3
    /// ```
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signal::try_combine`].
    pub fn combine(signals: &[Signal<T>]) -> Result<Self, SignaledError> {
        let callbacks: Vec<Arc<CallbackSync<T>>> = signals
            .iter()
            .map(|signal| {
                signal.callback.read().map(|cb| cb.clone()).map_err(|_| {
                    SignaledError::PoisonedLock {
                        source: ErrorSource::SignalCallback,
                    }
                })
            })
            .collect::<Result<_, _>>()?;

        let triggers: Vec<Arc<TriggerSync<T>>> = signals
            .iter()
            .map(|signal| {
                signal.trigger.read().map(|tr| tr.clone()).map_err(|_| {
                    SignaledError::PoisonedLock {
                        source: ErrorSource::SignalTrigger,
                    }
                })
            })
            .collect::<Result<_, _>>()?;

        let priority = signals
            .iter()
            .max_by_key(|s| s.priority.load(Ordering::Relaxed))
            .map(|s| s.priority.load(Ordering::Relaxed))
            .unwrap_or(0);
        let id = new_signal_id();
        Ok(Signal {
            callback: Arc::new(RwLock::new(Arc::new(move |old, new| {
                for callback in &callbacks {
                    callback(old, new);
                }
            }))),
            trigger: Arc::new(RwLock::new(Arc::new(move |old, new| {
                triggers.iter().all(|tr| tr(old, new))
            }))),
            id,
            priority: AtomicU64::new(priority),
            once: AtomicBool::new(false),
            mute: AtomicBool::new(false),
        })
    }

    /// Combines `N` amount of [`Signal`]s returning a single combined [`Signal`] instance.
    ///
    /// This function unlike [`Signal::combine`], is non-blocking so there are no re-entrant calls that block until the `callback` and `trigger` locks can be acquired.
    ///
    /// The order in which each `callback` will be called depends on the order that the [`Signal`]s are passed into the argument's slice.
    ///
    /// * `callback` will invoke all callbacks from the combined [`Signal`]s.
    ///
    /// * `trigger` will combine all triggers only returning `true` if every `trigger` does so.
    ///
    /// * `priority` will be the highest `priority` of the provided `signals`.
    ///
    /// * `mute` and `once` will be `false` by default.
    ///
    /// * `id` will be a new unique [`SignalId`].
    ///
    /// # Arguments
    ///
    /// * `signals` A slice containing the [`Signal`]s that will be combined into a single [`Signal`] instance.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if any of the provided [`Signal`] instances `callback` or `trigger` locks are poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if any of the provided [`Signal`] instances `callback` or `trigger` locks are held elsewhere.
    ///
    /// # Examples
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use signaled::{signal_sync, sync::{Signal}};
    ///
    /// let calls = Arc::new(Mutex::new(0));
    ///
    /// let calls_clone = Arc::clone(&calls);
    /// let signal_a = signal_sync!(move |_, _| {
    ///     let mut lock = calls_clone.lock().unwrap();
    ///     *lock = *lock + 1;
    /// });
    ///
    /// let calls_clone = Arc::clone(&calls);
    /// let signal_b = signal_sync!(move |_, _| {
    ///     let mut lock = calls_clone.lock().unwrap();
    ///     *lock = *lock + 2;
    /// });
    ///
    /// let signal_c = Signal::try_combine(&[signal_a, signal_b]).unwrap();
    ///
    /// signal_c.emit(&(), &()).unwrap();
    /// assert_eq!(*calls.lock().unwrap(), 3); // `signal_a` increases calls by 1 and `signal_b` increases calls by 2 so the `combined` signal increases calls by 3
    /// ```
    pub fn try_combine(signals: &[Signal<T>]) -> Result<Self, SignaledError> {
        let callbacks: Vec<Arc<CallbackSync<T>>> = signals
            .iter()
            .map(|signal| {
                signal
                    .callback
                    .try_read()
                    .map(|cb| cb.clone())
                    .map_err(|e| match e {
                        TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                            source: ErrorSource::SignalCallback,
                        },
                        TryLockError::WouldBlock => SignaledError::WouldBlock {
                            source: ErrorSource::SignalCallback,
                        },
                    })
            })
            .collect::<Result<_, _>>()?;

        let triggers: Vec<Arc<TriggerSync<T>>> = signals
            .iter()
            .map(|signal| {
                signal
                    .trigger
                    .try_read()
                    .map(|tr| tr.clone())
                    .map_err(|e| match e {
                        TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                            source: ErrorSource::SignalTrigger,
                        },
                        TryLockError::WouldBlock => SignaledError::WouldBlock {
                            source: ErrorSource::SignalTrigger,
                        },
                    })
            })
            .collect::<Result<_, _>>()?;

        let priority = signals
            .iter()
            .max_by_key(|s| s.priority.load(Ordering::Relaxed))
            .map(|s| s.priority.load(Ordering::Relaxed))
            .unwrap_or(0);
        let id = new_signal_id();
        Ok(Signal {
            callback: Arc::new(RwLock::new(Arc::new(move |old, new| {
                for callback in &callbacks {
                    callback(old, new);
                }
            }))),
            trigger: Arc::new(RwLock::new(Arc::new(move |old, new| {
                triggers.iter().all(|tr| tr(old, new))
            }))),
            id,
            priority: AtomicU64::new(priority),
            once: AtomicBool::new(false),
            mute: AtomicBool::new(false),
        })
    }
}

impl<T: Send + Sync + 'static> Display for Signal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Signal {{ id: {}, priority: {}, once: {}, mute: {} }}",
            self.id,
            self.priority.load(Ordering::Relaxed),
            self.once.load(Ordering::Relaxed),
            self.mute.load(Ordering::Relaxed)
        )
    }
}

impl<T: Send + Sync + 'static> Debug for Signal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signal")
            .field("id", &self.id)
            .field("priority", &self.priority.load(Ordering::Relaxed))
            .field("once", &self.once.load(Ordering::Relaxed))
            .field("mute", &self.mute.load(Ordering::Relaxed))
            .field("callback", &"<function>")
            .field("trigger", &"<function>")
            .finish()
    }
}

impl<T: Send + Sync + 'static> Default for Signal<T> {
    fn default() -> Self {
        Self {
            callback: Arc::new(RwLock::new(Arc::new(|_, _| {}))),
            trigger: Arc::new(RwLock::new(Arc::new(|_, _| true))),
            id: new_signal_id(),
            priority: AtomicU64::new(0),
            once: AtomicBool::new(false),
            mute: AtomicBool::new(false),
        }
    }
}

/// Creates a [`Signal`] with any number of parameters.
///
/// # Examples
/// ```
/// use signaled::{signal_sync, sync::{Signal}};
/// let s1: Signal<i32> = signal_sync!();
/// let s2: Signal<i32> = signal_sync!(|_, _| println!("changed"));
/// let s3: Signal<i32> = signal_sync!(|_, _| {}, |_, _| true, 1, false, true);
/// ```
///
/// Arguments fill fields in this order:
/// 1. callback  
/// 2. trigger  
/// 3. priority  
/// 4. once  
/// 5. mute  
///
/// Missing fields use their [`Default`] values.
#[macro_export]
macro_rules! signal_sync {
    () => {
        Signal::default()
    };

    ($callback:expr) => {
        Signal::new($callback, |_, _| true, 0, false, false)
    };

    ($callback:expr, $trigger:expr) => {
        Signal::new($callback, $trigger, 0, false, false)
    };

    ($callback:expr, $trigger:expr, $priority:expr) => {
        Signal::new($callback, $trigger, $priority, false, false)
    };

    ($callback:expr, $trigger:expr, $priority:expr, $once:expr) => {
        Signal::new($callback, $trigger, $priority, $once, false)
    };

    ($callback:expr, $trigger:expr, $priority:expr, $once:expr, $mute:expr) => {
        Signal::new($callback, $trigger, $priority, $once, $mute)
    };
}

/// A reactive container that holds a value and emits [`Signal`]s when the value changes.
///
/// [`Signaled<T>`] manages a value of type `T` and a collection of [`Signal<T>`] instances.
///
/// When the value is updated via `set`, all [`Signal`]s are emitted in order of descending priority,
/// provided their trigger conditions are met.
///
/// # Examples
///
/// ```
/// use signaled::{signal_sync, sync::{Signaled, Signal, SignaledError}};
///
/// let signaled = Signaled::new(0);
/// signaled.add_signal(signal_sync!(|old, new| println!("Old: {} | New: {}", old, new))).unwrap();
/// signaled.set(42).unwrap(); // Prints "Old: 0 | New: 42"
/// ```
pub struct Signaled<T: Send + Sync + 'static> {
    /// Reactive value, the mutation of this value through `set` will emit all [`Signal`] inside `signals`.
    val: RwLock<T>,
    /// Collection of [`Signal`]s that will be emitted when `val` is changed through `set`.
    signals: Arc<Mutex<Vec<Signal<T>>>>,

    /// Instant to use as reference for throttling.
    throttle_instant: Arc<Mutex<Instant>>,
    /// Duration of the throttling.
    throttle_duration: Arc<Mutex<Duration>>,
}

impl<T: Send + Sync + 'static> Signaled<T> {
    /// Creates a new instance of [`Signaled`] with the given initial value.
    ///
    /// # Arguments
    ///
    /// * `val` - The initial value.
    pub fn new(val: T) -> Self {
        Self {
            val: RwLock::new(val),
            signals: Arc::new(Mutex::new(Vec::new())),
            throttle_instant: Arc::new(Mutex::new(Instant::now())),
            throttle_duration: Arc::new(Mutex::new(Duration::ZERO)),
        }
    }

    /// Sets a new value for `val` and emits all [`Signal`]s.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if `val`, `signals` or [`Signal`]s inside `signals` locks are poisoned.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{signal_sync, sync::{Signaled, Signal}};
    ///
    /// let signaled = Signaled::new(0);
    /// signaled.add_signal(signal_sync!(|old, new| println!("Old: {} | New: {}", old, new)));
    /// signaled.set(1).unwrap(); /// Prints "Old: 0 | New: 1"
    /// ```
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_set`].
    pub fn set(&self, new_value: T) -> Result<(), SignaledError> {
        let mut guard = self.val.write().map_err(|_| SignaledError::PoisonedLock {
            source: ErrorSource::Value,
        })?;
        let old_value = std::mem::replace(&mut *guard, new_value);
        self.emit_signals(&old_value, &guard)
    }

    /// Sets a new value for `val` and emits all [`Signal`]s.
    ///
    /// This function unlike [`Signaled::set`], is non-blocking so there are no re-entrant calls that block until the lock for `val` is acquired.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if `val`, `signals` or [`Signal`]s inside `signals` locks are poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if `val`, `signals` or [`Signal`]s inside `signals` locks are held elsewhere.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{signal_sync, sync::{Signaled, Signal}};
    ///
    /// let signaled = Signaled::new(0);
    /// signaled.add_signal(signal_sync!(|old, new| println!("Old: {} | New: {}", old, new)));
    /// signaled.try_set(1).unwrap(); /// Prints "Old: 0 | New: 1"
    /// ```
    pub fn try_set(&self, new_value: T) -> Result<(), SignaledError> {
        let mut guard = self.val.try_write().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::Value,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::Value,
            },
        })?;
        let old_value = std::mem::replace(&mut *guard, new_value);
        self.try_emit_signals(&old_value, &*guard)
    }

    /// Sets a new value for `val` without emitting `signals`.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if `val` lock is poisoned.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{signal_sync, sync::{Signaled, Signal}};
    ///
    /// let signaled = Signaled::new(0);
    /// signaled.add_signal(signal_sync!(|_, _| { println!("do something")})).unwrap();
    /// signaled.set_silent(1).unwrap(); // This does not emit the signal so "do something" is not printed.
    /// assert_eq!(signaled.get().unwrap(), 1);
    /// ```
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_set_silent`].
    pub fn set_silent(&self, new_value: T) -> Result<(), SignaledError> {
        let mut guard = self.val.write().map_err(|_| SignaledError::PoisonedLock {
            source: ErrorSource::Value,
        })?;
        *guard = new_value;
        Ok(())
    }

    /// Sets a new value for `val` without emitting `signals`.
    ///
    /// This function unlike [`Signaled::set_silent`], is non-blocking so there are no re-entrant calls that block until the lock for `val` is acquired.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if `val` lock is poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if `val` lock is held elsewhere.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{signal_sync, sync::{Signaled, Signal}};
    ///
    /// let signaled = Signaled::new(0);
    /// signaled.add_signal(signal_sync!(|_, _| { println!("do something")})).unwrap();
    /// signaled.try_set_silent(1).unwrap(); // This does not emit the signal so "do something" is not printed.
    /// assert_eq!(signaled.get().unwrap(), 1);
    /// ```
    pub fn try_set_silent(&self, new_value: T) -> Result<(), SignaledError> {
        let mut guard = self.val.try_write().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::Value,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::Value,
            },
        })?;
        *guard = new_value;
        Ok(())
    }

    /// Sets a new value for `val` without emitting `signals`, but only if enough time
    /// has passed since the previous throttled update.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if any of the underlying locks for
    /// `throttle_instant`, `throttle_duration`, `val`, or `signals` are poisoned.
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_set_silent_throttled`].
    pub fn set_silent_throttled(&self, new_value: T) -> Result<(), SignaledError> {
        let mut throttle_instant =
            self.throttle_instant
                .lock()
                .map_err(|_| SignaledError::PoisonedLock {
                    source: ErrorSource::ThrottleInstant,
                })?;
        if Instant::now() < *throttle_instant {
            return Ok(());
        }

        self.set_silent(new_value)?;
        *throttle_instant = Instant::now()
            + *self
                .throttle_duration
                .lock()
                .map_err(|_| SignaledError::PoisonedLock {
                    source: ErrorSource::ThrottleDuration,
                })?;
        Ok(())
    }

    /// Sets a new value for `val` without emitting `signals`, but only if enough time
    /// has passed since the previous throttled update.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if any of the underlying locks for
    /// `throttle_instant`, `throttle_duration` or `val` are poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if any of the underlying locks for
    /// `throttle_instant`, `throttle_duration` or `val` are held elsewhere.
    pub fn try_set_silent_throttled(&self, new_value: T) -> Result<(), SignaledError> {
        let mut throttle_instant = self.throttle_instant.try_lock().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::ThrottleInstant,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::ThrottleInstant,
            },
        })?;
        if Instant::now() < *throttle_instant {
            return Ok(());
        }

        self.try_set_silent(new_value)?;
        *throttle_instant = Instant::now()
            + *self.throttle_duration.try_lock().map_err(|e| match e {
                TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                    source: ErrorSource::ThrottleDuration,
                },
                TryLockError::WouldBlock => SignaledError::WouldBlock {
                    source: ErrorSource::ThrottleDuration,
                },
            })?;
        Ok(())
    }

    /// Sets the minimum [`Duration`] that must elapse between consecutive calls to
    /// [`Signaled::set_throttled`].
    ///
    /// # Arguments
    ///
    /// * `duration` - The time interval to wait before another throttled update is allowed.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `throttle_duration` lock is poisoned.
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_set_throttle_duration`].
    pub fn set_throttle_duration(&self, duration: Duration) -> Result<(), SignaledError> {
        *self
            .throttle_duration
            .lock()
            .map_err(|_| SignaledError::PoisonedLock {
                source: ErrorSource::ThrottleDuration,
            })? = duration;
        Ok(())
    }

    /// Sets the minimum [`Duration`] that must elapse between consecutive calls to
    /// [`Signaled::set_throttled`].
    ///
    /// This function unlike [`Signaled::set_throttle_duration`], is non-blocking so there are no re-entrant calls that block until the lock can be acquired.
    ///
    /// # Arguments
    ///
    /// * `duration` - The time interval to wait before another throttled update is allowed.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `throttle_duration` lock is poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `throttle_duration` lock is held elsewhere.
    pub fn try_set_throttle_duration(&self, duration: Duration) -> Result<(), SignaledError> {
        *self.throttle_duration.try_lock().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::ThrottleDuration,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::ThrottleDuration,
            },
        })? = duration;
        Ok(())
    }

    /// Sets a new value for `val` and emits all [`Signal`]s, but only if enough time
    /// has passed since the previous throttled update.
    ///
    /// This method uses the configured [`throttle_duration`](Self::set_throttle_duration)
    /// and the internal `throttle_instant` to prevent signals from being emitted
    /// more frequently than allowed.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if any of the underlying locks for
    /// `throttle_instant`, `throttle_duration`, `val`, or `signals` are poisoned.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{sync::{Signaled, Signal}, signal_sync};
    /// use std::sync::{Arc, Mutex};
    /// use std::time::Duration;
    ///
    /// let calls = Arc::new(Mutex::new(0));
    /// let signaled = Signaled::new(());
    /// signaled.set_throttle_duration(Duration::from_millis(500)).unwrap();
    ///
    /// let calls_clone = Arc::clone(&calls);
    /// signaled.add_signal(signal_sync!(move |_, _| {
    ///     let mut lock = calls_clone.lock().unwrap();
    ///     *lock += 1;
    /// })).unwrap();
    ///
    /// signaled.set_throttled(()).unwrap();
    /// assert_eq!(*calls.lock().unwrap(), 1);
    /// std::thread::sleep(Duration::from_millis(100));
    ///
    /// signaled.set_throttled(()).unwrap();
    /// assert_eq!(*calls.lock().unwrap(), 1);
    /// std::thread::sleep(Duration::from_millis(500));
    ///
    /// signaled.set_throttled(()).unwrap();
    /// assert_eq!(*calls.lock().unwrap(), 2);
    /// ```
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_set_throttled`].
    pub fn set_throttled(&self, new_value: T) -> Result<(), SignaledError> {
        let mut throttle_instant =
            self.throttle_instant
                .lock()
                .map_err(|_| SignaledError::PoisonedLock {
                    source: ErrorSource::ThrottleInstant,
                })?;
        if Instant::now() < *throttle_instant {
            return Ok(());
        }

        self.set(new_value)?;
        *throttle_instant = Instant::now()
            + *self
                .throttle_duration
                .lock()
                .map_err(|_| SignaledError::PoisonedLock {
                    source: ErrorSource::ThrottleDuration,
                })?;
        Ok(())
    }

    /// Sets a new value for `val` and emits all [`Signal`]s, but only if enough time
    /// has passed since the previous throttled update.
    ///
    /// This function unlike [`Signaled::set_throttled`], is non-blocking so there are no re-entrant calls that block until a lock can be acquired.
    ///
    /// This method uses the configured [`throttle_duration`](Self::set_throttle_duration)
    /// and the internal `throttle_instant` to prevent signals from being emitted
    /// more frequently than allowed.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if any of the underlying locks for
    /// `throttle_instant`, `throttle_duration`, `val`, or `signals` are poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if any of the underlying locks for
    /// `throttle_instant`, `throttle_duration`, `val`, or `signals` are held elsewhere.
    pub fn try_set_throttled(&self, new_value: T) -> Result<(), SignaledError> {
        let mut throttle_instant = self.throttle_instant.try_lock().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::ThrottleInstant,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::ThrottleInstant,
            },
        })?;
        if Instant::now() < *throttle_instant {
            return Ok(());
        }

        self.try_set(new_value)?;
        *throttle_instant = Instant::now()
            + *self.throttle_duration.try_lock().map_err(|e| match e {
                TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                    source: ErrorSource::ThrottleDuration,
                },
                TryLockError::WouldBlock => SignaledError::WouldBlock {
                    source: ErrorSource::ThrottleDuration,
                },
            })?;

        Ok(())
    }

    /// Returns the lock of the current value.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if `val` lock is poisoned.
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_get_lock`].
    pub fn get_lock(&self) -> Result<RwLockReadGuard<'_, T>, SignaledError> {
        match self.val.read() {
            Ok(r) => Ok(r),
            Err(_) => Err(SignaledError::PoisonedLock {
                source: ErrorSource::Value,
            }),
        }
    }

    /// Returns the lock of the current value.
    ///
    /// This function unlike [`Signaled::get_lock`], is non-blocking so there are no re-entrant calls that block until the lock is acquired.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if `val` lock is poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if `val` lock is held elsewhere.
    pub fn try_get_lock(&self) -> Result<RwLockReadGuard<'_, T>, SignaledError> {
        match self.val.try_read() {
            Ok(r) => Ok(r),
            Err(TryLockError::WouldBlock) => Err(SignaledError::WouldBlock {
                source: ErrorSource::Value,
            }),
            Err(TryLockError::Poisoned(_)) => Err(SignaledError::PoisonedLock {
                source: ErrorSource::Value,
            }),
        }
    }

    /// Emits all [`Signal`]s in descending priority order, invoking their callbacks if their trigger condition is met.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` lock or [`Signal`]s inside `signals` locks are poisoned.
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_emit_signals`].
    fn emit_signals(&self, old: &T, new: &T) -> Result<(), SignaledError> {
        match self.signals.lock() {
            Ok(mut signals) => {
                signals.sort_by(|a, b| {
                    b.priority
                        .load(Ordering::Relaxed)
                        .cmp(&a.priority.load(Ordering::Relaxed))
                });
                for signal in signals.iter() {
                    signal.emit(old, new)?
                }
                signals.retain(|s| {
                    if !s.once.load(Ordering::Relaxed) {
                        return true;
                    }
                    match s.trigger.read() {
                        Ok(trigger) => !trigger(old, new),
                        Err(_) => true,
                    }
                });
                Ok(())
            }
            Err(_) => Err(SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            }),
        }
    }

    /// Emits all [`Signal`]s in descending priority order, invoking their callbacks if their trigger condition is met.
    ///
    /// This function unlike [`Signaled::emit_signals`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` lock or [`Signal`]s inside `signals` locks are poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `signals` lock or [`Signal`]s inside `signals` locks are held elsewhere.
    fn try_emit_signals(&self, old: &T, new: &T) -> Result<(), SignaledError> {
        match self.signals.try_lock() {
            Ok(mut signals) => {
                signals.sort_by(|a, b| {
                    b.priority
                        .load(Ordering::Relaxed)
                        .cmp(&a.priority.load(Ordering::Relaxed))
                });
                for signal in signals.iter() {
                    signal.try_emit(old, new)?
                }
                signals.retain(|s| {
                    if !s.once.load(Ordering::Relaxed) {
                        return true;
                    }
                    match s.trigger.try_read() {
                        Ok(trigger) => !trigger(old, new),
                        Err(_) => true,
                    }
                });
                Ok(())
            }
            Err(TryLockError::Poisoned(_)) => Err(SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            }),
            Err(TryLockError::WouldBlock) => Err(SignaledError::WouldBlock {
                source: ErrorSource::Signals,
            }),
        }
    }

    /// Adds a signal to the collection, returning its [`SignalId`].
    ///
    /// If a [`Signal`] with the same `id` is already in the collection, returns the existing `id` without adding the [`Signal`] to the collection.
    ///
    /// # Arguments
    /// * `signal` - The [`Signal`] to add.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` lock is poisoned.
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_add_signal`].
    pub fn add_signal(&self, signal: Signal<T>) -> Result<SignalId, SignaledError> {
        match self.signals.lock() {
            Ok(mut s) => {
                let id = signal.id;
                if s.iter().any(|sig| sig.id == id) {
                    return Ok(id);
                }
                s.push(signal);
                Ok(id)
            }
            Err(_) => Err(SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            }),
        }
    }

    /// Adds a signal to the collection, returning its [`SignalId`].
    ///
    /// If a [`Signal`] with the same `id` is already in the collection, returns the existing `id` without adding the [`Signal`] to the collection.
    ///
    /// This function unlike [`Signaled::add_signal`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    ///
    /// # Arguments
    /// * `signal` - The [`Signal`] to add.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` lock is poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `signals` lock is held elsewhere.
    pub fn try_add_signal(&self, signal: Signal<T>) -> Result<SignalId, SignaledError> {
        match self.signals.try_lock() {
            Ok(mut s) => {
                let id = signal.id;
                if s.iter().any(|sig| sig.id == id) {
                    return Ok(id);
                }
                s.push(signal);
                Ok(id)
            }
            Err(TryLockError::Poisoned(_)) => Err(SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            }),
            Err(TryLockError::WouldBlock) => Err(SignaledError::WouldBlock {
                source: ErrorSource::Signals,
            }),
        }
    }

    /// Removes a [`Signal`] by `id`, returning the removed [`Signal`].
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`] to remove.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` lock is poisoned.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the `id` does not match any [`Signal`].
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_remove_signal`].
    pub fn remove_signal(&self, id: SignalId) -> Result<Signal<T>, SignaledError> {
        match self.signals.lock() {
            Ok(mut s) => {
                let index = s
                    .iter()
                    .position(|s| s.id == id)
                    .ok_or(SignaledError::InvalidSignalId { id })?;
                Ok(s.remove(index))
            }
            Err(_) => Err(SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            }),
        }
    }

    /// Removes a [`Signal`] by `id`, returning the removed [`Signal`].
    ///
    /// This function unlike [`Signaled::remove_signal`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`] to remove.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` lock is poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `signals` lock is held elsewhere.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the `id` does not match any [`Signal`].
    pub fn try_remove_signal(&self, id: SignalId) -> Result<Signal<T>, SignaledError> {
        match self.signals.try_lock() {
            Ok(mut s) => {
                let index = s
                    .iter()
                    .position(|s| s.id == id)
                    .ok_or(SignaledError::InvalidSignalId { id })?;
                Ok(s.remove(index))
            }
            Err(TryLockError::Poisoned(_)) => Err(SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            }),
            Err(TryLockError::WouldBlock) => Err(SignaledError::WouldBlock {
                source: ErrorSource::Signals,
            }),
        }
    }

    /// Sets the callback for a [`Signal`] by `id`.
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`].
    /// * `callback` - The new callback function.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` or the targeted [`Signal`] `callback` locks are poisoned.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the `id` does not match any [`Signal`].
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_set_signal_callback`].
    pub fn set_signal_callback<F: Fn(&T, &T) + Send + Sync + 'static>(
        &self,
        id: SignalId,
        callback: F,
    ) -> Result<(), SignaledError> {
        let signals = self
            .signals
            .lock()
            .map_err(|_| SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_callback(callback)
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the callback for a [`Signal`] by `id`.
    ///
    /// This function unlike [`Signaled::set_signal_callback`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`].
    /// * `callback` - The new callback function.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` or the targeted [`Signal`] `callback` locks are poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `signals` or the targeted [`Signal`] callback are held elsewhere.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the `id` does not match any [`Signal`].
    pub fn try_set_signal_callback<F: Fn(&T, &T) + Send + Sync + 'static>(
        &self,
        id: SignalId,
        callback: F,
    ) -> Result<(), SignaledError> {
        let signals = self.signals.try_lock().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::Signals,
            },
        })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.try_set_callback(callback)
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the trigger condition for a [`Signal`] by `id`.
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`].
    /// * `trigger` - The new trigger function.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` or the targeted [`Signal`] `trigger` locks are poisoned.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the `id` does not match any [`Signal`].
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_set_signal_trigger`].
    pub fn set_signal_trigger<F: Fn(&T, &T) -> bool + Send + Sync + 'static>(
        &self,
        id: SignalId,
        trigger: F,
    ) -> Result<(), SignaledError> {
        let signals = self
            .signals
            .lock()
            .map_err(|_| SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_trigger(trigger)
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the trigger condition for a [`Signal`] by `id`.
    ///
    /// This function unlike [`Signaled::set_signal_trigger`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`].
    /// * `trigger` - The new trigger function.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` or the targeted [`Signal`] `trigger` locks are poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `signals` or the targeted [`Signal`] `trigger` locks are held elsewhere.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the `id` does not match any [`Signal`].
    pub fn try_set_signal_trigger<F: Fn(&T, &T) -> bool + Send + Sync + 'static>(
        &self,
        id: SignalId,
        trigger: F,
    ) -> Result<(), SignaledError> {
        let signals = self.signals.try_lock().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::Signals,
            },
        })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.try_set_trigger(trigger)
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the [`Signal`] `trigger` to always return true by `id`
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` or the targeted [`Signal`] `trigger` locks are poisoned.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the `id` does not match any [`Signal`].
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_remove_signal_trigger`].
    pub fn remove_signal_trigger(&self, id: SignalId) -> Result<(), SignaledError> {
        let signals = self
            .signals
            .lock()
            .map_err(|_| SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.remove_trigger()
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the [`Signal`] `trigger` to always return true by `id`
    ///
    /// This function unlike [`Signaled::remove_signal_trigger`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` or the targeted [`Signal`] `trigger` locks are poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `signals` or the targeted [`Signal`] `trigger` locks are held elsewhere.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the `id` does not match any [`Signal`].
    pub fn try_remove_signal_trigger(&self, id: SignalId) -> Result<(), SignaledError> {
        let signals = self.signals.try_lock().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::Signals,
            },
        })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.try_remove_trigger()
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the priority for a [`Signal`] by `id`.
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`].
    /// * `priority` - The new priority value (higher executes first).
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` lock is poisoned.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the `id` does not match any [`Signal`].
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_set_signal_priority`].
    pub fn set_signal_priority(&self, id: SignalId, priority: u64) -> Result<(), SignaledError> {
        let signals = self
            .signals
            .lock()
            .map_err(|_| SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_priority(priority);
            Ok(())
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the priority for a [`Signal`] by `id`.
    ///
    /// This function unlike [`Signaled::set_signal_priority`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`].
    /// * `priority` - The new priority value (higher executes first).
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` lock is poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `signals` lock is held elsewhere
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the `id` does not match any [`Signal`].
    pub fn try_set_signal_priority(
        &self,
        id: SignalId,
        priority: u64,
    ) -> Result<(), SignaledError> {
        let signals = self.signals.try_lock().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::Signals,
            },
        })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_priority(priority);
            Ok(())
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the `once` flag for a [`Signal`] by `id`.
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`].
    /// * `is_once` - Boolean that decides if the target [`Signal`] should be `once` or not.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` lock is poisoned.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the `id` does not match any [`Signal`].
    ///     
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_set_signal_once`].
    pub fn set_signal_once(&self, id: SignalId, is_once: bool) -> Result<(), SignaledError> {
        let signals = self
            .signals
            .lock()
            .map_err(|_| SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_once(is_once);
            Ok(())
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the `once` flag for a [`Signal`] by `id`.
    ///
    /// This function unlike [`Signaled::set_signal_once`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`].
    /// * `is_once` - Boolean that decides if the target [`Signal`] should be `once` or not.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` lock is poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `signals` lock is held elsewhere
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the `id` does not match any [`Signal`].
    pub fn try_set_signal_once(&self, id: SignalId, is_once: bool) -> Result<(), SignaledError> {
        let signals = self.signals.try_lock().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::Signals,
            },
        })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_once(is_once);
            Ok(())
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the `mute` flag for a [`Signal`] by `id`.
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`].
    /// * `is_mute` - Boolean that decides if the target [`Signal`] should be muted or not.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` lock is poisoned.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the `id` does not match any [`Signal`].
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_set_signal_mute`].
    pub fn set_signal_mute(&self, id: SignalId, is_mute: bool) -> Result<(), SignaledError> {
        let signals = self
            .signals
            .lock()
            .map_err(|_| SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_mute(is_mute);
            Ok(())
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the `mute` flag for a [`Signal`] by `id`.
    ///
    /// This function unlike [`Signaled::set_signal_mute`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`].
    /// * `is_mute` - Boolean that decides if the target [`Signal`] should be muted or not.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` lock is poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `signals` lock is held elsewhere
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the `id` does not match any [`Signal`].
    pub fn try_set_signal_mute(&self, id: SignalId, is_mute: bool) -> Result<(), SignaledError> {
        let signals = self.signals.try_lock().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::Signals,
            },
        })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_mute(is_mute);
            Ok(())
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Combines multiple [`Signal`]s by their `id` into a single [`Signal`].
    ///
    /// This function finds signals by their `id`, removes them from the `Signaled` instance,
    /// and then uses [`Signal::combine()`] to create a new, single [`Signal`] instance.
    /// This new combined signal is then added back to the `signals` collection and its new `SignalId` is returned.
    ///
    /// For more details about the combination process,
    /// see the documentation for [`Signal::combine()`].
    ///
    /// # Arguments
    ///
    /// * `signal_ids` A slice containing the `id`s of the [`Signal`]s that will be combined into a single [`Signal`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` or any target [`Signal`] `callback` or `trigger` locks are poisoned.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the any of the provided `SignalId` does not match any [`Signal`].
    ///
    /// # Examples
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use signaled::{signal_sync, sync::{Signal, Signaled}};
    ///
    /// let signaled = Signaled::new(0);
    ///
    /// let calls = Arc::new(Mutex::new(0));
    ///
    /// let calls_clone = Arc::clone(&calls);
    /// let signal_a = signal_sync!(move |_, _| {
    ///     let mut lock = calls_clone.lock().unwrap();
    ///     *lock = *lock + 1;
    /// });
    /// let signal_a_id = signaled.add_signal(signal_a).unwrap();
    ///
    /// let calls_clone = Arc::clone(&calls);
    /// let signal_b = signal_sync!(move |_, _| {
    ///     let mut lock = calls_clone.lock().unwrap();
    ///     *lock = *lock + 2;
    /// });
    /// let signal_b_id = signaled.add_signal(signal_b).unwrap();
    ///
    /// signaled.combine_signals(&[signal_a_id, signal_b_id]).unwrap();
    ///
    /// signaled.set(1).unwrap();
    /// assert_eq!(*calls.lock().unwrap(), 3); // `signal_a` increases calls by 1 and `signal_b` increases calls by 2 so the `combined` signal increases calls by 3
    /// ```
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_combine_signals`].
    pub fn combine_signals(&self, signal_ids: &[SignalId]) -> Result<SignalId, SignaledError> {
        let mut target_signals = Vec::new();
        let mut signals = self
            .signals
            .lock()
            .map_err(|_| SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            })?;

        for &id in signal_ids {
            if !signals.iter().any(|s| s.id == id) {
                return Err(SignaledError::InvalidSignalId { id });
            }
        }

        for &id in signal_ids {
            let index = signals
                .iter()
                .position(|s| s.id == id)
                .ok_or(SignaledError::InvalidSignalId { id })?;
            let signal = signals.remove(index);
            target_signals.push(signal);
        }

        let combined_signal = Signal::combine(&target_signals)?;
        let id = combined_signal.id;
        signals.push(combined_signal);
        Ok(id)
    }

    /// Combines multiple [`Signal`]s by their `id` into a single [`Signal`].
    ///
    /// This function unlike [`Signaled::combine_signals`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    ///
    /// This function finds signals by their `id`, removes them from the `Signaled` instance,
    /// and then uses [`Signal::try_combine()`] to create a new, single [`Signal`] instance.
    /// This new combined signal is then added back to the `signals` collection and its new `SignalId` is returned.
    ///
    /// For more details about the combination process,
    /// see the documentation for [`Signal::try_combine()`].
    ///
    /// # Arguments
    ///
    /// * `signal_ids` A slice containing the `id`s of the [`Signal`]s that will be combined into a single [`Signal`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if the `signals` or any target [`Signal`] `callback` or `trigger` locks are poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if the `signals` lock is held elsewhere.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the any of the provided `SignalId` does not match any [`Signal`].
    ///
    /// # Examples
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use signaled::{signal_sync, sync::{Signal, Signaled}};
    ///
    /// let signaled = Signaled::new(0);
    ///
    /// let calls = Arc::new(Mutex::new(0));
    ///
    /// let calls_clone = Arc::clone(&calls);
    /// let signal_a = signal_sync!(move |_, _| {
    ///     let mut lock = calls_clone.lock().unwrap();
    ///     *lock = *lock + 1;
    /// });
    /// let signal_a_id = signaled.add_signal(signal_a).unwrap();
    ///
    /// let calls_clone = Arc::clone(&calls);
    /// let signal_b = signal_sync!(move |_, _| {
    ///     let mut lock = calls_clone.lock().unwrap();
    ///     *lock = *lock + 2;
    /// });
    /// let signal_b_id = signaled.add_signal(signal_b).unwrap();
    ///
    /// signaled.try_combine_signals(&[signal_a_id, signal_b_id]).unwrap();
    ///
    /// signaled.set(1).unwrap();
    /// assert_eq!(*calls.lock().unwrap(), 3); // `signal_a` increases calls by 1 and `signal_b` increases calls by 2 so the `combined` signal increases calls by 3
    /// ```
    pub fn try_combine_signals(&self, signal_ids: &[SignalId]) -> Result<SignalId, SignaledError> {
        let mut target_signals = Vec::new();
        let mut signals = self.signals.try_lock().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::Signals,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::Signals,
            },
        })?;

        for &id in signal_ids {
            if !signals.iter().any(|s| s.id == id) {
                return Err(SignaledError::InvalidSignalId { id });
            }
        }

        for &id in signal_ids {
            let index = signals
                .iter()
                .position(|s| s.id == id)
                .ok_or(SignaledError::InvalidSignalId { id })?;
            let signal = signals.remove(index);
            target_signals.push(signal);
        }

        let combined_signal = Signal::try_combine(&target_signals)?;
        let id = combined_signal.id;
        signals.push(combined_signal);
        Ok(id)
    }
}

impl<T: Display + Send + Sync + 'static> Display for Signaled<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = self
            .val
            .try_read()
            .map(|v| v.to_string())
            .unwrap_or_else(|_| "<locked>".to_string());

        let signals_display = self
            .signals
            .try_lock()
            .map(|signals| {
                let signals_str = signals
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "signal_count: {}, signals: [{}]",
                    signals.len(),
                    signals_str
                )
            })
            .unwrap_or_else(|_| "signals: <locked>".to_string());

        write!(f, "Signaled {{ val: {}, {} }}", value, signals_display)
    }
}

impl<T: Debug + Send + Sync + 'static> Debug for Signaled<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("Signaled");
        match self.val.try_read() {
            Ok(val) => s.field("val", &*val),
            Err(_) => s.field("val", &"<locked>"),
        };
        match self.signals.try_lock() {
            Ok(signals) => s.field("signals", &*signals),
            Err(_) => s.field("signals", &"<locked>"),
        };
        s.finish()
    }
}

impl<T: Clone + Send + Sync + 'static> Signaled<T> {
    /// Sets a new value for `val` and emits all [`Signal`]s in a separated thread, returning a [`JoinHandle`] representing that thread.
    ///
    /// The returned [`JoinHandle`] will contain a [`SignaledError`] if any [`Signal`] emission failed.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if `val` or `signals` locks are poisoned.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{signal_sync, sync::{Signaled, Signal}};
    ///
    /// let signaled = Signaled::new(0);
    /// signaled.add_signal(signal_sync!(|old, new| println!("Old: {} | New: {}", old, new)));
    /// let handle = signaled.set_and_spawn(1).unwrap();
    /// handle.join();
    /// ```
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_set_and_spawn`].
    pub fn set_and_spawn(
        &self,
        new_value: T,
    ) -> Result<JoinHandle<Result<(), SignaledError>>, SignaledError> {
        let mut guard = self.val.write().map_err(|_| SignaledError::PoisonedLock {
            source: ErrorSource::Value,
        })?;
        let old_value = std::mem::replace(&mut *guard, new_value);
        let new_value_clone = (*guard).clone();
        drop(guard);

        let signals_clone = Arc::clone(&self.signals);
        let handle = thread::spawn(move || -> Result<(), SignaledError> {
            let mut signals = signals_clone
                .lock()
                .map_err(|_| SignaledError::PoisonedLock {
                    source: ErrorSource::Signals,
                })?;
            signals.sort_by(|a, b| {
                b.priority
                    .load(Ordering::Relaxed)
                    .cmp(&a.priority.load(Ordering::Relaxed))
            });
            for signal in signals.iter() {
                signal.emit(&old_value, &new_value_clone)?;
            }
            signals.retain(|s| {
                if !s.once.load(Ordering::Relaxed) {
                    return true;
                }
                match s.trigger.read() {
                    Ok(trigger) => !trigger(&old_value, &new_value_clone),
                    Err(_) => true,
                }
            });
            Ok(())
        });
        Ok(handle)
    }

    /// Sets a new value for `val` and emits all [`Signal`]s in a separated thread, returning a [`JoinHandle`] representing that thread.
    ///
    /// The returned [`JoinHandle`] will contain a [`SignaledError`] if any [`Signal`] emission failed.
    ///
    /// This function unlike [`Signaled::set_and_spawn`], is non-blocking so there are no re-entrant calls that block until `val` or `signals` lock can be acquired.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if `val` or `signals` locks are poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if `val` or `signals` locks are held elsewhere.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{signal_sync, sync::{Signaled, Signal}};
    ///
    /// let signaled = Signaled::new(0);
    /// signaled.add_signal(signal_sync!(|old, new| println!("Old: {} | New: {}", old, new)));
    /// let handle = signaled.try_set_and_spawn(1).unwrap();
    /// handle.join();
    /// ```
    pub fn try_set_and_spawn(
        &self,
        new_value: T,
    ) -> Result<JoinHandle<Result<(), SignaledError>>, SignaledError> {
        let mut guard = self.val.try_write().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::Value,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::Value,
            },
        })?;
        let old_value = std::mem::replace(&mut *guard, new_value);
        let new_value_clone = (*guard).clone();
        drop(guard);

        let signals_clone = Arc::clone(&self.signals);
        let handle = thread::spawn(move || -> Result<(), SignaledError> {
            let mut signals = signals_clone.try_lock().map_err(|e| match e {
                TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                    source: ErrorSource::Signals,
                },
                TryLockError::WouldBlock => SignaledError::WouldBlock {
                    source: ErrorSource::Signals,
                },
            })?;
            signals.sort_by(|a, b| {
                b.priority
                    .load(Ordering::Relaxed)
                    .cmp(&a.priority.load(Ordering::Relaxed))
            });
            for signal in signals.iter() {
                signal.try_emit(&old_value, &new_value_clone)?;
            }
            signals.retain(|s| {
                if !s.once.load(Ordering::Relaxed) {
                    return true;
                }
                match s.trigger.try_read() {
                    Ok(trigger) => !trigger(&old_value, &new_value_clone),
                    Err(_) => true,
                }
            });
            Ok(())
        });
        Ok(handle)
    }

    /// Sets a new value for `val` and emits all [`Signal`]s in a separated thread, returning a [`JoinHandle`] representing that thread,
    /// but only if enough time has passed since the previous throttled update.
    ///
    /// This method uses the configured [`throttle_duration`](Self::set_throttle_duration)
    /// and the internal `throttle_instant` to prevent signals from being emitted
    /// more frequently than allowed.
    ///
    /// The returned [`JoinHandle`] will contain a [`SignaledError`] if any [`Signal`] emission failed.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if any of the underlying locks for
    /// `throttle_instant`, `throttle_duration`, `val` or `signals` locks are poisoned.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{sync::{Signaled, Signal}, signal_sync};
    /// use std::sync::{Arc, Mutex};
    /// use std::time::Duration;
    ///
    /// let calls = Arc::new(Mutex::new(0));
    /// let signaled = Signaled::new(());
    /// signaled.set_throttle_duration(Duration::from_millis(500)).unwrap();
    ///
    /// let calls_clone = Arc::clone(&calls);
    /// signaled.add_signal(signal_sync!(move |_, _| {
    ///     let mut lock = calls_clone.lock().unwrap();
    ///     *lock += 1;
    /// })).unwrap();
    ///
    /// signaled.set_throttled_and_spawn(()).unwrap().join().unwrap().unwrap();
    /// assert_eq!(*calls.lock().unwrap(), 1);
    /// std::thread::sleep(Duration::from_millis(100));
    ///
    /// signaled.set_throttled_and_spawn(()).unwrap().join().unwrap().unwrap();
    /// assert_eq!(*calls.lock().unwrap(), 1);
    /// std::thread::sleep(Duration::from_millis(500));
    ///
    /// signaled.set_throttled_and_spawn(()).unwrap().join().unwrap().unwrap();
    /// assert_eq!(*calls.lock().unwrap(), 2);
    /// ```
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_set_throttled_and_spawn`].
    pub fn set_throttled_and_spawn(
        &self,
        new_value: T,
    ) -> Result<JoinHandle<Result<(), SignaledError>>, SignaledError> {
        let mut throttle_instant =
            self.throttle_instant
                .lock()
                .map_err(|_| SignaledError::PoisonedLock {
                    source: ErrorSource::ThrottleInstant,
                })?;
        if Instant::now() < *throttle_instant {
            return Ok(thread::spawn(|| Ok(())));
        }

        let mut guard = self.val.write().map_err(|_| SignaledError::PoisonedLock {
            source: ErrorSource::Value,
        })?;
        let old_value = std::mem::replace(&mut *guard, new_value);
        let new_value_clone = (*guard).clone();
        drop(guard);

        let signals_clone = Arc::clone(&self.signals);
        let handle = thread::spawn(move || -> Result<(), SignaledError> {
            let mut signals = signals_clone
                .lock()
                .map_err(|_| SignaledError::PoisonedLock {
                    source: ErrorSource::Signals,
                })?;
            signals.sort_by(|a, b| {
                b.priority
                    .load(Ordering::Relaxed)
                    .cmp(&a.priority.load(Ordering::Relaxed))
            });
            for signal in signals.iter() {
                signal.emit(&old_value, &new_value_clone)?;
            }
            signals.retain(|s| {
                if !s.once.load(Ordering::Relaxed) {
                    return true;
                }
                match s.trigger.read() {
                    Ok(trigger) => !trigger(&old_value, &new_value_clone),
                    Err(_) => true,
                }
            });
            Ok(())
        });
        *throttle_instant = Instant::now()
            + *self
                .throttle_duration
                .lock()
                .map_err(|_| SignaledError::PoisonedLock {
                    source: ErrorSource::ThrottleDuration,
                })?;
        Ok(handle)
    }

    /// Sets a new value for `val` and emits all [`Signal`]s in a separated thread, returning a [`JoinHandle`] representing that thread,
    /// but only if enough time has passed since the previous throttled update.
    ///
    /// This function unlike [`Signaled::set_throttled_and_spawn`], is non-blocking so there are no re-entrant calls that block until the lock can be acquired.
    ///
    /// This method uses the configured [`throttle_duration`](Self::set_throttle_duration)
    /// and the internal `throttle_instant` to prevent signals from being emitted
    /// more frequently than allowed.
    ///
    /// The returned [`JoinHandle`] will contain a [`SignaledError`] if any [`Signal`] emission failed.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if any of the underlying locks for
    /// `throttle_instant`, `throttle_duration`, `val` or `signals` locks are poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if any of the underlying locks for
    /// `throttle_instant`, `throttle_duration`, `val` or `signals` locks are held elsewhere.
    pub fn try_set_throttled_and_spawn(
        &self,
        new_value: T,
    ) -> Result<JoinHandle<Result<(), SignaledError>>, SignaledError> {
        let mut throttle_instant = self.throttle_instant.try_lock().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::ThrottleInstant,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::ThrottleInstant,
            },
        })?;
        if Instant::now() < *throttle_instant {
            return Ok(thread::spawn(|| Ok(())));
        }

        let mut guard = self.val.try_write().map_err(|e| match e {
            TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                source: ErrorSource::Value,
            },
            TryLockError::WouldBlock => SignaledError::WouldBlock {
                source: ErrorSource::Value,
            },
        })?;
        let old_value = std::mem::replace(&mut *guard, new_value);
        let new_value_clone = (*guard).clone();
        drop(guard);

        let signals_clone = Arc::clone(&self.signals);
        let handle = thread::spawn(move || -> Result<(), SignaledError> {
            let mut signals = signals_clone.try_lock().map_err(|e| match e {
                TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                    source: ErrorSource::Signals,
                },
                TryLockError::WouldBlock => SignaledError::WouldBlock {
                    source: ErrorSource::Signals,
                },
            })?;
            signals.sort_by(|a, b| {
                b.priority
                    .load(Ordering::Relaxed)
                    .cmp(&a.priority.load(Ordering::Relaxed))
            });
            for signal in signals.iter() {
                signal.try_emit(&old_value, &new_value_clone)?;
            }
            signals.retain(|s| {
                if !s.once.load(Ordering::Relaxed) {
                    return true;
                }
                match s.trigger.try_read() {
                    Ok(trigger) => !trigger(&old_value, &new_value_clone),
                    Err(_) => true,
                }
            });
            Ok(())
        });

        *throttle_instant = Instant::now()
            + *self.throttle_duration.try_lock().map_err(|e| match e {
                TryLockError::Poisoned(_) => SignaledError::PoisonedLock {
                    source: ErrorSource::ThrottleDuration,
                },
                TryLockError::WouldBlock => SignaledError::WouldBlock {
                    source: ErrorSource::ThrottleDuration,
                },
            })?;
        Ok(handle)
    }

    /// Returns a cloned copy of the current value.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if `val` lock is poisoned.
    ///
    /// # Warnings
    ///
    /// This function may result in a deadlock if used incorrectly.
    ///
    /// For a non-blocking alternative, see [`Signaled::try_get`].
    pub fn get(&self) -> Result<T, SignaledError> {
        match self.val.read() {
            Ok(r) => Ok(r.clone()),
            Err(_) => Err(SignaledError::PoisonedLock {
                source: ErrorSource::Value,
            }),
        }
    }

    /// Returns a cloned copy of the current value.
    ///
    /// This function unlike [`Signaled::get`], is non-blocking so there are no re-entrant calls that block until the lock is acquired.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::PoisonedLock`] if `val` lock is poisoned.
    ///
    /// Returns [`SignaledError::WouldBlock`] if `val` lock is held elsewhere.
    pub fn try_get(&self) -> Result<T, SignaledError> {
        match self.val.try_read() {
            Ok(r) => Ok(r.clone()),
            Err(TryLockError::WouldBlock) => Err(SignaledError::WouldBlock {
                source: ErrorSource::Value,
            }),
            Err(TryLockError::Poisoned(_)) => Err(SignaledError::PoisonedLock {
                source: ErrorSource::Value,
            }),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use core::panic;

    use super::*;

    #[test]
    fn test_signal() {
        let calls = Arc::new(Mutex::new(0));
        let calls_clone = Arc::clone(&calls);

        let signaled = Signaled::new(1);
        signaled
            .add_signal(signal_sync!(move |_, _| {
                let mut lock = calls_clone.lock().unwrap();
                *lock = *lock + 1;
            }))
            .unwrap();

        signaled.set(2).unwrap(); // Calls = 1
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn test_signal_trigger() {
        let calls = Arc::new(Mutex::new(0));
        let calls_clone = Arc::clone(&calls);

        let signal: Signal<i32> = signal_sync!(move |_, _| {
            let mut lock = calls_clone.lock().unwrap();
            *lock = *lock + 1;
        });
        signal.set_trigger(|_, new| *new > 5).unwrap(); // Signal will only invoke callback if the `new_value` is greater than 5.

        let signaled = Signaled::new(1);
        signaled.add_signal(signal).unwrap();

        signaled.set(4).unwrap(); // This does not meet the trigger condition (new_value < 5). Calls = 0
        assert_eq!(*calls.lock().unwrap(), 0);

        signaled.set(6).unwrap(); // This does meet the trigger condition (new_value > 5). Calls = 1
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn test_remove_signal() {
        let calls = Arc::new(Mutex::new(0));
        let calls_clone = Arc::clone(&calls);

        let signaled = Signaled::new(1);
        let signal_id = signaled
            .add_signal(signal_sync!(move |_, _| {
                let mut lock = calls_clone.lock().unwrap();
                *lock = *lock + 1;
            }))
            .unwrap();

        signaled.set(2).unwrap(); // Calls = 1
        assert_eq!(*calls.lock().unwrap(), 1);

        signaled.set(3).unwrap(); // Calls = 2
        assert_eq!(*calls.lock().unwrap(), 2);

        signaled.remove_signal(signal_id).unwrap(); // Signal removed so `calls` will not increase when `signaled` emits.

        signaled.set(4).unwrap(); // Calls = 2
        assert_eq!(*calls.lock().unwrap(), 2);
    }

    #[test]
    fn test_remove_trigger() {
        let calls = Arc::new(Mutex::new(0));
        let calls_clone = Arc::clone(&calls);

        let signal: Signal<i32> = signal_sync!(move |_, _| {
            let mut lock = calls_clone.lock().unwrap();
            *lock = *lock + 1;
        });

        signal.set_trigger(|_, new| *new > 5).unwrap(); // Signal will only invoke callback if the `new_value` is greater than 5.

        let signaled = Signaled::new(1);
        let signal_id = signaled.add_signal(signal).unwrap();

        signaled.set(4).unwrap(); // This does not meet the trigger condition (new_value < 5). Calls = 0
        assert_eq!(*calls.lock().unwrap(), 0);

        signaled.set(6).unwrap(); // This does meet the trigger condition (new_value > 5). Calls = 1
        assert_eq!(*calls.lock().unwrap(), 1);

        signaled.remove_signal_trigger(signal_id).unwrap(); // Trigger condition removed so `new_value` will always invoke callback.

        signaled.set(4).unwrap(); // Calls = 2
        assert_eq!(*calls.lock().unwrap(), 2);
    }

    #[test]
    fn test_change_signal() {
        let calls = Arc::new(Mutex::new(0));
        let calls_clone = Arc::clone(&calls);

        let signaled = Signaled::new(1);
        let signal_id = signaled
            .add_signal(signal_sync!(move |_, _| {
                let mut lock = calls_clone.lock().unwrap();
                *lock = *lock + 1;
            }))
            .unwrap();

        signaled.set(2).unwrap(); // Calls = 1
        assert_eq!(*calls.lock().unwrap(), 1);

        signaled.set(3).unwrap(); // Calls = 2
        assert_eq!(*calls.lock().unwrap(), 2);

        let calls_clone = Arc::clone(&calls);
        signaled
            .set_signal_callback(signal_id, move |_, _| {
                let mut lock = calls_clone.lock().unwrap();
                *lock = *lock + 2;
            })
            .unwrap(); // New callback increases call count by 2.

        signaled.set(4).unwrap(); // Calls = 4
        assert_eq!(*calls.lock().unwrap(), 4);
    }

    #[test]
    fn test_change_trigger() {
        let calls = Arc::new(Mutex::new(0));
        let calls_clone = Arc::clone(&calls);

        let signal: Signal<i32> = signal_sync!(move |_, _| {
            let mut lock = calls_clone.lock().unwrap();
            *lock = *lock + 1;
        });
        signal.set_trigger(|_, new| *new > 5).unwrap(); // Signal will only invoke callback if the `new_value` is greater than 5.

        let signaled = Signaled::new(1);
        let signal_id = signaled.add_signal(signal).unwrap();

        signaled.set(4).unwrap(); // This does not meet the trigger condition (new_value < 5). Calls = 0
        assert_eq!(*calls.lock().unwrap(), 0);

        signaled.set(6).unwrap(); // This does meet the trigger condition (new_value > 5). Calls = 1
        assert_eq!(*calls.lock().unwrap(), 1);

        signaled
            .set_signal_trigger(signal_id, |_, new| *new < 5)
            .unwrap(); // Trigger condition changed so `new_value` being < 5 will invoke callback.

        signaled.set(6).unwrap(); // This does not meet the new trigger condition (new_value > 5). Calls = 1
        assert_eq!(*calls.lock().unwrap(), 1);

        signaled.set(4).unwrap(); // This does meet the new trigger condition (new value < 5). Calls = 2
        assert_eq!(*calls.lock().unwrap(), 2);
    }

    #[test]
    fn test_priority() {
        let signaled = Signaled::new(0);

        let test_value = Arc::new(Mutex::new(' '));

        let test_value_clone = Arc::clone(&test_value);
        let signal_a: Signal<i32> = signal_sync!(move |_, _| {
            *test_value_clone.lock().unwrap() = 'a';
        });
        signal_a.set_priority(3);

        let test_value_clone = Arc::clone(&test_value);
        let signal_b: Signal<i32> = signal_sync!(move |_, _| {
            assert_eq!(*test_value_clone.lock().unwrap(), 'a');
        }); // Signal in the middle with a callback that checks that `signal_a` callback has been invoked first.
        signal_b.set_priority(2);

        let test_value_clone = Arc::clone(&test_value);
        let signal_c: Signal<i32> =
            signal_sync!(move |_, _| { *test_value_clone.lock().unwrap() = 'c' });
        signal_c.set_priority(1);

        let signal_a_id = signaled.add_signal(signal_a).unwrap();
        let signal_b_id = signaled.add_signal(signal_b).unwrap();
        let signal_c_id = signaled.add_signal(signal_c).unwrap();

        signaled.set(0).unwrap();
        assert_eq!(*test_value.lock().unwrap(), 'c'); // `signal_c` has the lowest priority (1) so after all callbacks are invoked the value will be 'c'.

        signaled.set_signal_priority(signal_a_id, 1).unwrap(); // Set priority to 1, `signal_a` will now be the last to execute.

        let test_value_clone = Arc::clone(&test_value);
        signaled
            .set_signal_callback(signal_b_id, move |_, _| {
                assert_eq!(*test_value_clone.lock().unwrap(), 'c')
            })
            .unwrap(); // Modify signal in the middle callback to check that `signal_c` callback has been invoked first.

        signaled.set_signal_priority(signal_c_id, 3).unwrap(); // Set priority to 3, `signal_c` will now be the first to execute.

        signaled.set(0).unwrap();
        assert_eq!(*test_value.lock().unwrap(), 'a'); // `signal_a` has now the lowest priority (1) so after all callbacks are invoked the valuew ill be 'a'.
    }

    #[test]
    fn test_once_signal() {
        let signaled = Signaled::new(5);
        let signal_a: Signal<i32> = signal_sync!(|_, _| {});
        let signal_b: Signal<i32> = signal_sync!(|_, _| {});
        signal_b.set_once(true);

        signaled.add_signal(signal_a).unwrap();
        signaled.add_signal(signal_b).unwrap();

        assert_eq!(signaled.signals.lock().unwrap().len(), 2); // Signal A and Signal B.
        signaled.set(6).unwrap(); // Signal B is dropped because it is `once`.
        assert_eq!(signaled.signals.lock().unwrap().len(), 1); // Signal A.
    }

    #[test]
    fn test_mute_signal() {
        let calls = Arc::new(Mutex::new(0));
        let signaled = Signaled::new(0);

        let calls_clone = Arc::clone(&calls);
        let signal: Signal<i32> = signal_sync!(move |_, _| {
            let mut lock = calls_clone.lock().unwrap();
            *lock = *lock + 1;
        });

        let signal_id = signaled.add_signal(signal).unwrap();

        signaled.set(6).unwrap(); // Calls = 1, Signal is unmuted.
        assert_eq!(*calls.lock().unwrap(), 1);

        signaled.set_signal_mute(signal_id, true).unwrap(); // Signal muted so next Signaled `set` won't invoke its callback.

        signaled.set(7).unwrap(); // Calls = 1, Signal is muted.
        assert_eq!(*calls.lock().unwrap(), 1);

        signaled.set_signal_mute(signal_id, false).unwrap(); // Signal unmuted so next Signaled `set` will invoke its callback.

        signaled.set(8).unwrap(); // Calls = 2, Signal is unmuted.
        assert_eq!(*calls.lock().unwrap(), 2);
    }

    #[test]
    fn test_getters() {
        let signaled = Signaled::new(0);
        assert_eq!(signaled.get().unwrap(), 0);
        assert_eq!(signaled.try_get().unwrap(), 0);
        assert_eq!(*signaled.get_lock().unwrap(), 0);
        assert_eq!(*signaled.try_get_lock().unwrap(), 0);

        for i in 0..10 {
            signaled.set(i).unwrap();
            assert_eq!(signaled.get().unwrap(), i);
            assert_eq!(signaled.try_get().unwrap(), i);
            assert_eq!(*signaled.get_lock().unwrap(), i);
            assert_eq!(*signaled.try_get_lock().unwrap(), i);
        }
    }

    #[test]
    fn test_old_new() {
        let signaled = Signaled::new(0);
        signaled
            .add_signal(signal_sync!(|old, new| assert!(*new == *old + 1)))
            .unwrap();

        for _ in 1..10 {
            signaled.set(signaled.get().unwrap() + 1).unwrap();
            signaled.try_set(signaled.try_get().unwrap() + 1).unwrap();
        }
    }

    #[test]
    fn test_multithread_set() {
        let calls = Arc::new(Mutex::new(0));

        let signaled = Arc::new(Mutex::new(Signaled::new(())));

        let calls_clone = Arc::clone(&calls);
        signaled
            .lock()
            .unwrap()
            .add_signal(signal_sync!(move |_, _| {
                let mut lock = calls_clone.lock().unwrap();
                *lock = *lock + 1;
            }))
            .unwrap();

        let mut handles = Vec::new();

        for _ in 0..5 {
            let signaled_clone = Arc::clone(&signaled);
            let handle = std::thread::spawn(move || {
                let lock = signaled_clone.lock().unwrap();
                for _ in 0..5 {
                    lock.set(()).unwrap();
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(*calls.lock().unwrap(), 25); // Spawned 5 threads each one increases called set 5 times so call count is 25.
    }

    #[test]
    fn test_multithread_once_signal() {
        let calls = Arc::new(Mutex::new(0));

        let signaled = Arc::new(Mutex::new(Signaled::new(())));

        let calls_clone = Arc::clone(&calls);
        let signal = signal_sync!(move |_, _| {
            let mut lock = calls_clone.lock().unwrap();
            *lock = *lock + 1;
        });
        signal.set_once(true);

        signaled.lock().unwrap().add_signal(signal).unwrap();

        let mut handles = Vec::new();
        for _ in 0..5 {
            let signaled_clone = Arc::clone(&signaled);
            let handle = std::thread::spawn(move || {
                signaled_clone.lock().unwrap().set(()).unwrap();
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(*calls.lock().unwrap(), 1); // Signal is `once` so the first thread that calls it removes it from the `signals` collection.
    }

    /////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

    macro_rules! test_signaled_would_block_error {
        ($test_name:ident, $lock:ident, $get_lock:ident, $method:ident $(, $args:expr)*; $error:expr) => {
            #[test]
            fn $test_name() {
                let signaled = Signaled::new(0);
                let _lock = signaled.$lock.$get_lock().unwrap();
                let err: SignaledError = $error;
                assert!(signaled.$method($($args),*).is_err_and(|e| e == err));
            }
        };
    }

    test_signaled_would_block_error!(test_would_block_source_value, val, read, try_set, 1; SignaledError::WouldBlock { source: ErrorSource::Value });
    test_signaled_would_block_error!(test_would_block_source_signals, signals, lock, try_set, 1; SignaledError::WouldBlock { source: ErrorSource::Signals });
    test_signaled_would_block_error!(test_would_block_source_throttle_instant, throttle_instant, lock, try_set_throttled, 1; SignaledError::WouldBlock { source: ErrorSource::ThrottleInstant });
    test_signaled_would_block_error!(test_would_block_source_throttle_duration, throttle_duration, lock, try_set_throttled, 1; SignaledError::WouldBlock { source: ErrorSource::ThrottleDuration });

    macro_rules! test_signal_would_block_error {
        ($test_name:ident, $lock:ident, $get_lock:ident, $method:ident $(, $args:expr)*; $error:expr) => {
            #[test]
            fn $test_name() {
                let signal: Signal<i32> = signal_sync!(|_, _| {});
                let _lock = signal.$lock.$get_lock().unwrap();
                let err: SignaledError = $error;
                assert!(signal.$method($($args),*).is_err_and(|e| e == err));
            }
        };
    }

    test_signal_would_block_error!(test_would_block_source_signal_callback, callback, write, try_emit, &1, &2; SignaledError::WouldBlock { source: ErrorSource::SignalCallback });
    test_signal_would_block_error!(test_would_block_source_signal_trigger, trigger, write, try_emit, &1, &2; SignaledError::WouldBlock { source: ErrorSource::SignalTrigger });

    /////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

    macro_rules! test_signaled_poisoned_lock {
        ($test_name:ident, $lock:ident, $get_lock:ident, $method:ident $(, $args:expr)*; $error:expr) => {
            #[test]
            fn $test_name() {
                let signaled = Arc::new(Signaled::new(0));
                let signaled_clone = Arc::clone(&signaled);
                let result = std::panic::catch_unwind(move || {
                    let _lock = signaled_clone.$lock.$get_lock().unwrap();
                    panic!();
                });
                assert!(result.is_err());
                let err: SignaledError = $error;
                assert!(signaled.$method($($args),*).is_err_and(|e| e == err));
            }
        };
    }
    test_signaled_poisoned_lock!(test_poisoned_lock_value, val, write, set, 1; SignaledError::PoisonedLock { source: ErrorSource::Value });
    test_signaled_poisoned_lock!(test_poisoned_lock_signals, signals, lock, set, 1; SignaledError::PoisonedLock { source: ErrorSource::Signals });
    test_signaled_poisoned_lock!(test_poisoned_lock_throttle_instant, throttle_instant, lock, set_throttled, 1; SignaledError::PoisonedLock { source: ErrorSource::ThrottleInstant });
    test_signaled_poisoned_lock!(test_poisoned_lock_throttle_duration, throttle_duration, lock, set_throttled, 1; SignaledError::PoisonedLock { source: ErrorSource::ThrottleDuration });

    /////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

    macro_rules! test_signal_poisoned_lock {
        ($test_name:ident, $lock:ident, $method:ident $(, $args:expr)*; $error:expr) => {
            #[test]
            fn $test_name() {
                let signal: Arc<Signal<i32>> = Arc::new(signal_sync!(|_, _| {}));
                let signal_clone = Arc::clone(&signal);
                let result = std::panic::catch_unwind(move || {
                    let _lock = signal_clone.$lock.write().unwrap();
                    panic!();
                });
                assert!(result.is_err());
                let err: SignaledError = $error;
                assert!(signal.$method($($args),*).is_err_and(|e| e == err));
            }
        };
    }
    test_signal_poisoned_lock!(test_poisoned_lock_signal_callback, callback, emit, &1, &2; SignaledError::PoisonedLock { source: ErrorSource::SignalCallback });
    test_signal_poisoned_lock!(test_poisoned_lock_signal_trigger, trigger, emit, &1, &2; SignaledError::PoisonedLock { source: ErrorSource::SignalTrigger });

    /////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

    #[test]
    fn test_invalid_id_error() {
        let signaled = Signaled::new(());
        let signal_id = signaled.add_signal(signal_sync!(|_, _| {})).unwrap();
        assert!(
            signaled
                .set_signal_callback(signal_id + 1, |_, _| {})
                .is_err_and(|e| e == SignaledError::InvalidSignalId { id: signal_id + 1 })
        )
    }

    /////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

    macro_rules! test_set_and_spawn {
        ($test_name:ident, $method:ident) => {
            #[test]
            fn $test_name() {
                let signaled = Signaled::new(0);
                let calls = Arc::new(Mutex::new(0));

                let calls_clone = Arc::clone(&calls);
                signaled
                    .add_signal(signal_sync!(
                        move |_, _| {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            let mut lock = calls_clone.lock().unwrap();
                            *lock = *lock + 1;
                        },
                        |_, _| true,
                        3,
                        true
                    ))
                    .unwrap();

                let calls_clone = Arc::clone(&calls);
                signaled
                    .add_signal(signal_sync!(
                        move |_, _| {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            let lock = calls_clone.lock().unwrap();
                            assert_eq!(*lock, 1)
                        },
                        |_, _| true,
                        2,
                        true
                    ))
                    .unwrap();

                let calls_clone = Arc::clone(&calls);
                signaled
                    .add_signal(signal_sync!(
                        move |_, _| {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            let mut lock = calls_clone.lock().unwrap();
                            *lock = *lock + 2;
                        },
                        |_, _| true,
                        1,
                        true
                    ))
                    .unwrap();

                assert_eq!(signaled.signals.lock().unwrap().len(), 3);
                let handle = signaled.$method(1).unwrap();
                assert_eq!(*calls.lock().unwrap(), 0);
                let _ = handle.join().unwrap();
                assert_eq!(signaled.signals.lock().unwrap().len(), 0);
                assert_eq!(*calls.lock().unwrap(), 3);
            }
        };
    }

    test_set_and_spawn!(test_set_and_spawn, set_and_spawn);
    test_set_and_spawn!(test_try_set_and_spawn, try_set_and_spawn);

    macro_rules! test_set_silent {
        ($test_name:ident, $method:ident) => {
            #[test]
            fn $test_name() {
                let calls = Arc::new(Mutex::new(0));
                let signaled = Signaled::new(0);

                let calls_clone = Arc::clone(&calls);
                signaled
                    .add_signal(signal_sync!(move |_, _| {
                        let mut lock = calls_clone.lock().unwrap();
                        *lock = *lock + 1;
                    }))
                    .unwrap();

                signaled.set(1).unwrap();
                assert_eq!(signaled.get().unwrap(), 1);
                assert_eq!(*calls.lock().unwrap(), 1);

                signaled.$method(2).unwrap();
                assert_eq!(signaled.get().unwrap(), 2);
                assert_eq!(*calls.lock().unwrap(), 1);
            }
        };
    }

    test_set_silent!(test_set_silent, set_silent);
    test_set_silent!(test_try_set_silent, try_set_silent);

    macro_rules! test_combine {
        ($test_name:ident, $method:ident) => {
            #[test]
            fn $test_name() {
                let calls = Arc::new(Mutex::new(0));
                let calls_clone = Arc::clone(&calls);
                let signal_a: Signal<i32> = Signal::new(
                    move |_, _| {
                        let mut lock = calls_clone.lock().unwrap();
                        *lock = *lock + 1;
                    },
                    |_, new| *new > 10,
                    100,
                    false,
                    false,
                );

                let calls_clone = Arc::clone(&calls);
                let signal_b: Signal<i32> = Signal::new(
                    move |_, _| {
                        let mut lock = calls_clone.lock().unwrap();
                        *lock = *lock + 2;
                    },
                    |old, _| *old > 10,
                    10,
                    false,
                    false,
                );

                signal_a.emit(&0, &9).unwrap(); // `new` < 10, Calls = 0
                assert_eq!(*calls.lock().unwrap(), 0);
                signal_a.emit(&0, &11).unwrap(); // `new` > 10, Calls = 1
                assert_eq!(*calls.lock().unwrap(), 1);

                signal_b.emit(&9, &0).unwrap(); // `old` < 10, Calls = 1
                assert_eq!(*calls.lock().unwrap(), 1);
                signal_b.emit(&11, &0).unwrap(); // `old` > 10, Calls = 3
                assert_eq!(*calls.lock().unwrap(), 3);

                let signal_c = Signal::$method(&[signal_a, signal_b]).unwrap();
                assert_eq!(signal_c.priority.load(Ordering::Relaxed), 100); // Keeps the highest priority after combining

                signal_c.emit(&9, &9).unwrap(); // Both triggers return false
                assert_eq!(*calls.lock().unwrap(), 3);
                signal_c.emit(&9, &11).unwrap(); // One triggers return false
                assert_eq!(*calls.lock().unwrap(), 3);
                signal_c.emit(&11, &11).unwrap(); // Both triggers return true, Calls = 6
                assert_eq!(*calls.lock().unwrap(), 6);
            }
        };
    }

    test_combine!(test_combine, combine);
    test_combine!(test_try_combine, try_combine);

    macro_rules! test_combine_signals {
        ($test_name:ident, $method:ident) => {
            #[test]
            fn $test_name() {
                let signaled = Signaled::new(0);
                let signal_a_id = signaled.add_signal(signal_sync!(|_, _| {})).unwrap();
                let signal_b_id = signaled.add_signal(signal_sync!(|_, _| {})).unwrap();
                let signal_c_id = signaled.add_signal(signal_sync!(|_, _| {})).unwrap();
                let signal_d_id = signaled.add_signal(signal_sync!(|_, _| {})).unwrap();
                assert_eq!(signaled.signals.lock().unwrap().len(), 4);

                signaled
                    .$method(&[signal_a_id, signal_b_id, signal_c_id, signal_d_id])
                    .unwrap();
                assert_eq!(signaled.signals.lock().unwrap().len(), 1);
            }
        };
    }

    test_combine_signals!(test_combine_signals, combine_signals);
    test_combine_signals!(test_try_combine_signals, try_combine_signals);

    #[test]
    fn test_once_retain() {
        let signaled = Signaled::new(1);
        signaled
            .add_signal(signal_sync!(
                |_, _| {},
                |old, new| *new > *old,
                1,
                true,
                false
            ))
            .unwrap();
        assert_eq!(signaled.signals.lock().unwrap().len(), 1);

        signaled.set(1).unwrap();
        assert_eq!(signaled.signals.lock().unwrap().len(), 1);

        signaled.set(2).unwrap();
        assert_eq!(signaled.signals.lock().unwrap().len(), 0);
    }

    macro_rules! test_set_throttled {
        ($test_name:ident, $set:ident, $set_duration:ident) => {
            #[test]
            fn $test_name() {
                let calls = Arc::new(Mutex::new(0));
                let signaled = Signaled::new(());
                signaled.$set_duration(Duration::from_millis(500)).unwrap();

                let calls_clone = Arc::clone(&calls);
                signaled
                    .add_signal(signal_sync!(move |_, _| {
                        let mut lock = calls_clone.lock().unwrap();
                        *lock = *lock + 1;
                    }))
                    .unwrap();

                signaled.$set(()).unwrap();
                assert_eq!(*calls.lock().unwrap(), 1);
                std::thread::sleep(Duration::from_millis(100));

                signaled.$set(()).unwrap();
                assert_eq!(*calls.lock().unwrap(), 1);
                std::thread::sleep(Duration::from_millis(500));

                signaled.$set(()).unwrap();
                assert_eq!(*calls.lock().unwrap(), 2);
            }
        };
    }

    test_set_throttled!(test_set_throttled, set_throttled, set_throttle_duration);
    test_set_throttled!(
        test_try_set_throttled,
        try_set_throttled,
        try_set_throttle_duration
    );

    macro_rules! test_set_throttled_and_spawn {
        ($test_name:ident, $set:ident, $set_duration:ident) => {
            #[test]
            fn $test_name() {
                let calls = Arc::new(Mutex::new(0));
                let signaled = Signaled::new(());
                signaled.$set_duration(Duration::from_millis(500)).unwrap();

                let calls_clone = Arc::clone(&calls);
                signaled
                    .add_signal(signal_sync!(move |_, _| {
                        let mut lock = calls_clone.lock().unwrap();
                        *lock = *lock + 1;
                    }))
                    .unwrap();

                signaled.$set(()).unwrap().join().unwrap().unwrap();
                assert_eq!(*calls.lock().unwrap(), 1);
                std::thread::sleep(Duration::from_millis(100));

                signaled.$set(()).unwrap().join().unwrap().unwrap();
                assert_eq!(*calls.lock().unwrap(), 1);
                std::thread::sleep(Duration::from_millis(500));

                signaled.$set(()).unwrap().join().unwrap().unwrap();
                assert_eq!(*calls.lock().unwrap(), 2);
            }
        };
    }

    test_set_throttled_and_spawn!(
        test_set_throttled_and_spawn,
        set_throttled_and_spawn,
        set_throttle_duration
    );
    test_set_throttled_and_spawn!(
        test_try_set_throttled_and_spawn,
        try_set_throttled_and_spawn,
        try_set_throttle_duration
    );

    #[test]
    fn test_reentrant_try_set() {
        let signaled = Arc::new(Signaled::new(0));
        let signaled_clone = Arc::clone(&signaled);

        signaled
            .add_signal(signal_sync!(move |_, _| {
                let err = signaled_clone.try_set(2).unwrap_err();
                assert_eq!(
                    err,
                    SignaledError::WouldBlock {
                        source: ErrorSource::Value
                    }
                );
            }))
            .unwrap();

        signaled.set(1).unwrap();
    }
}
