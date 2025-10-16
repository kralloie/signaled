//! # Signaled (Sync)
//!
//! A lightweight reactive programming library for Rust, providing a signal-slot mechanism.
//! `Signaled<T>` holds a value of type `T` and a collection of `Signal<T>` instances, which are
//! callbacks triggered when the value changes. Signals support priorities and conditional triggers,
//! making it ideal for reactive UI updates, event handling, or state management.
//!
//! ## Features
//!
//! - **Reactive Updates**: Update a value and automatically emit signals to registered callbacks.
//! - **Priority-Based Signals**: Signals are executed in descending priority order.
//! - **Conditional Triggers**: Signals can have trigger functions to control callback execution.
//! - **One-Time Signals**: Signals can be flagged as `once` making them only be called once and then removed from the Signaled Signal collection.
//! - **Safe Mutability**: Uses `Mutex` for interior mutability.
//! - **Error Handling**: Returns `Result` with `SignaledError` for poisoned locks and invalid signal IDs.
//!
//! ## Limitations
//!
//! - Recursive calls to `set` or `emit` in signal callbacks may result in a deadlock.
//! - Re-entrant calls (e.g. calling `set` from within the callback of a `Signal<T>`) may result in a deadlock.
//!
//! ## Examples
//!
//! Basic usage to create a `Signaled<i32>`, add a signal, and emit changes:
//!
//! ```
//! use signaled::{signal_sync, sync::{Signal, SignaledError, Signaled}};
//!
//! let signaled = Signaled::new(0);
//! let signal = signal_sync!(|old: &i32, new: &i32| println!("Old: {} | New: {}", old, new));
//! signaled.add_signal(signal).unwrap();
//! signaled.set(42).unwrap(); // Prints "Old: 0 | New: 42"
//! ```
//!
//! Using priorities and triggers:
//!
//! ```
//! use signaled::{signal_sync, sync::{Signal, SignaledError, Signaled}};
//!
//! let signaled = Signaled::new(0);
//! let high_priority = signal_sync!(|old: &i32, new: &i32| println!("High: Old: {}, New: {}", old, new));
//! high_priority.set_priority(10);
//! let conditional = signal_sync!(|old: &i32, new: &i32| println!("Conditional: Old: {}, New: {}", old, new));
//! conditional.set_trigger(|old: &i32, new: &i32| *new > *old + 5).unwrap();
//! signaled.add_signal(high_priority).unwrap();
//! signaled.add_signal(conditional).unwrap();
//! signaled.set(10).unwrap(); // Prints "High: Old: 0, New: 10" and "Conditional: Old: 0, New: 10"
//! signaled.set(3).unwrap(); // Prints only "High: Old: 10, New: 3"
//! ```
//!
//! ## Error Handling
//!
//! Methods like `set`, `emit`, and `remove_signal` return `Result` with `SignaledError` for:
//! - `PoisonedLock`: Attempted to acquire a poisoned lock from `Mutex` or `RwLock`.
//! - `InvalidSignalId`: Provided a `Signal` ID that does not exist.
//! - `WouldBlock`: Attempted to acquire a lock that is held elsewhere.
//! 
//! ## Deadlock risk
//!
//! ```
//! use signaled::sync::{SignaledError, Signaled};
//!
//! let signaled = Signaled::new(0);
//! let read_lock = signaled.get_lock().unwrap();
//! // let result = signaled.set(1).unwrap(); Since we hold the lock in the `read_lock` variable, calling `set` will try to acquire the lock resulting in a deadlock.
//! ```

#![allow(dead_code)]
use std::fmt::{Display};
use std::sync::{RwLock, RwLockReadGuard, TryLockError};
use std::sync::{Arc, Mutex, atomic::{AtomicU64, AtomicBool, Ordering}};

////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

/// Source of an error in the [`Signaled`] or [`Signal`] structs.
#[derive(Debug)]
pub enum ErrorSource {
    /// Error related to the `val` field of [`Signaled`].
    Value,
    /// Error related to the [`Signal`] vector of [`Signaled`].
    Signals,
    /// Error related to a [`Signal`]'s `callback`.
    SignalCallback,
    /// Error related to a [`Signal`]'s `trigger`.
    SignalTrigger
}

/// Type of error when manipulating [`Signaled`] or [`Signal`] structs. 
#[derive(Debug)] 
pub enum ErrorType {
    /// Attempted to acquire a poisoned [`Mutex`] or [`RwLock`] lock.
    PoisonedLock { source: ErrorSource },
    /// Attempted to acquire a lock that is already held.
    WouldBlock { source: ErrorSource },
    /// Provided a `SignalId` that does not match any [`Signal`].
    InvalidSignalId { id: SignalId }
}

/// Error type returned by [`Signaled`] and [`Signal`] functions.
/// 
/// Contains a descriptive message indicating the source and type of the error, such as borrow conflicts or invalid [`Signal`] `id`.
#[derive(Debug)]
pub struct SignaledError {
    /// Descriptive error message.
    pub message: String
}

/// Constructs a [`SignaledError`] from an [`ErrorType`].
/// 
/// # Arguments
/// 
/// * `err_type` - [`ErrorType`] that represents the type of the error and contains a field representing the source of the error.
pub fn signaled_error(err_type: ErrorType) -> SignaledError {
    let err_msg = match err_type {
        ErrorType::PoisonedLock { source } => {
            match source {
                ErrorSource::SignalCallback => "Cannot access Signal callback: lock is poisoned due to a previous panic".to_string(),
                ErrorSource::SignalTrigger => "Cannot access Signal trigger: lock is poisoned due to a previous panic".to_string(),
                ErrorSource::Signals => "Cannot access Signaled signals: lock is poisoned due to a previous panic".to_string(),
                ErrorSource::Value => "Cannot access Signaled value: lock is poisoned due to a previous panic".to_string()
            }
        }
        ErrorType::WouldBlock { source } => {
            match source {
                ErrorSource::SignalCallback => "Cannot access Signal callback: lock is already held elsewhere".to_string(),
                ErrorSource::SignalTrigger => "Cannot access Signal trigger: lock is already held elsewhere".to_string(),
                ErrorSource::Signals => "Cannot access Signaled signals: lock is already held elsewhere".to_string(),
                ErrorSource::Value => "Cannot access Signaled value: lock is already held elsewhere".to_string()
            }
        }
        ErrorType::InvalidSignalId { id } => {
            format!("Signal ID '{}' does not match any Signal", id)
        }
    };

    SignaledError { message: err_msg }
}

type SignalId = u64;

static SIGNAL_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generates a new [`SignalId`] for a [`Signal`] using the global identifier counter
/// 
/// # Panics 
/// 
/// Panics in debug builds if the identifier counter overflows (`u64::MAX`).
pub fn new_signal_id() -> SignalId {
    let id = SIGNAL_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    debug_assert!(id != u64::MAX, "SignalId counter overflow");
    id
}
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
    callback: Arc<Mutex<Box<dyn Fn(&T, &T) + Send + Sync + 'static>>>,
    /// Function that decides if the callback will be invoked or not when the [`Signal`] is emitted.
    trigger: Arc<Mutex<Box<dyn Fn(&T, &T) -> bool + Send + Sync + 'static>>>,
    /// Identifier for the [`Signal`].
    id: u64,
    /// Number used in the [`Signaled`] struct to decide the order of execution of the signals.
    priority: AtomicU64,
    /// Boolean representing if the [`Signal`] should be removed from the Signaled after being called once.
    once: AtomicBool,
    /// Boolean representing if the [`Signal`] should not invoke the callback when emitted.
    mute: AtomicBool
}

impl<T: Send + Sync + 'static> Signal<T> {
    /// Creates a new [`Signal`] instance.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function to execute when the signal is emitted.
    /// * `trigger` -  Function that returns a boolean representing if the `callback` will be invoked or not.
    /// * `priority` - Number representing the priority in which the [`Signal`] will be emitted from the parent [`Signaled`].
    /// * `once` - Boolean representing if the [`Signal`] will be removed from the parent [`Signaled`] `signals` after being emitted once.
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
    pub fn new<F, G> (callback: F, trigger: G, priority: u64, once: bool, mute: bool) -> Self
    where
        F: Fn(&T, &T) + Send + Sync + 'static,
        G: Fn(&T, &T) -> bool + Send + Sync + 'static,
    {
        Signal {
            callback: Arc::new(Mutex::new(Box::new(callback))),
            trigger: Arc::new(Mutex::new(Box::new(trigger))),
            id: new_signal_id(),
            priority: AtomicU64::new(priority),
            once: AtomicBool::new(once),
            mute: AtomicBool::new(mute)
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
    /// Returns [`SignaledError`] if the `callback` or `trigger` locks are poisoned.
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
    /// For a non-blocking alternative, see [`try_emit`].
    pub fn emit(&self, old: &T, new: &T) -> Result<(), SignaledError> {
        if self.mute.load(Ordering::Relaxed) {
            return Ok(())
        }
        let trigger = self.trigger.lock()
            .map_err(|_| signaled_error(ErrorType::PoisonedLock { source: ErrorSource::SignalTrigger }))?;
        if trigger(old, new) {
            let callback = self.callback.lock()
                .map_err(|_| signaled_error(ErrorType::PoisonedLock { source: ErrorSource::SignalCallback }))?;
            callback(old, new);
        }
        Ok(())
    }

    /// Emits the signal, executing the callback if the trigger condition is met.
    /// 
    /// This function unlike [`emit`], is non-blocking so there are no re-entrant calls that block until the `trigger` and `callback` locks can be acquired.
    ///
    /// # Arguments
    ///
    /// * `old` - The old `val` of the [`Signaled`].
    /// * `new` - The new `val` of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError`] if the `callback` or `trigger` locks are poisoned or held elsewhere.
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
            return Ok(())
        }
        let trigger = self.trigger.try_lock()
            .map_err(|e| {
                match e {
                    TryLockError::Poisoned(_) => signaled_error(ErrorType::PoisonedLock { source: ErrorSource::SignalTrigger }),
                    TryLockError::WouldBlock => signaled_error(ErrorType::WouldBlock { source: ErrorSource::SignalTrigger })
                }
            })?;
        if trigger(old, new) {
            let callback = self.callback.try_lock()
                .map_err(|e| {
                    match e {
                        TryLockError::Poisoned(_) => signaled_error(ErrorType::PoisonedLock { source: ErrorSource::SignalCallback }),
                        TryLockError::WouldBlock => signaled_error(ErrorType::WouldBlock { source: ErrorSource::SignalCallback })
                    }
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
    /// Returns [`SignaledError`] if the `callback` lock is poisoned.
    ///     
    /// # Warnings
    /// 
    /// This function may result in a deadlock if used incorrectly.
    /// 
    /// For a non-blocking alternative, see [`try_set_callback`].
    pub fn set_callback<F: Fn(&T, &T) + Send + Sync + 'static>(&self, callback: F) -> Result<(), SignaledError> {
        let mut lock = self.callback.lock().map_err(|_| signaled_error(ErrorType::PoisonedLock { source: ErrorSource::SignalCallback }))?;
        *lock = Box::new(callback);
        Ok(())
    }
    
    /// Sets a new callback for the [`Signal`].
    /// 
    /// This function unlike [`set_callback`], is non-blocking so there are no re-entrant calls that block until the `callback` lock can be acquired.
    /// 
    /// # Arguments
    /// 
    /// * `callback` - The new callback function.
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if the `callback` lock is poisoned or held elsewhere.
    pub fn try_set_callback<F: Fn(&T, &T) + Send + Sync + 'static>(&self, callback: F) -> Result<(), SignaledError> {
        let mut lock = self.callback.try_lock().map_err(|e| {
            match e {
                TryLockError::Poisoned(_) => signaled_error(ErrorType::PoisonedLock { source: ErrorSource::SignalCallback }),
                TryLockError::WouldBlock => signaled_error(ErrorType::WouldBlock { source: ErrorSource::SignalCallback })
            }
        })?;
        *lock = Box::new(callback);
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
    /// Returns [`SignaledError`] if the `trigger` lock is poisoned.
    /// 
    /// # Warnings
    /// 
    /// This function may result in a deadlock if used incorrectly.
    /// 
    /// For a non-blocking alternative, see [`try_set_trigger`]
    pub fn set_trigger<F: Fn(&T, &T) -> bool + Send + Sync + 'static>(&self, trigger: F) -> Result<(), SignaledError> {
        let mut lock = self.trigger.lock().map_err(|_| signaled_error(ErrorType::PoisonedLock { source: ErrorSource::SignalTrigger }))?;
        *lock = Box::new(trigger);
        Ok(())
    }

    /// Sets a new trigger for the [`Signal`].
    /// 
    /// This function unlike [`set_trigger`], is non-blocking so there are no re-entrant calls that block until the `trigger` lock can be acquired.
    /// 
    /// Default trigger always returns `true`.
    /// 
    /// # Arguments
    /// 
    /// * `trigger` - The new trigger function.
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if the `trigger` lock is poisoned or held elsewhere.
    pub fn try_set_trigger<F: Fn(&T, &T) -> bool + Send + Sync + 'static>(&self, trigger: F) -> Result<(), SignaledError> {
        let mut lock = self.trigger.try_lock().map_err(|e| {
            match e {
                TryLockError::Poisoned(_) => signaled_error(ErrorType::PoisonedLock { source: ErrorSource::SignalTrigger }),
                TryLockError::WouldBlock => signaled_error(ErrorType::WouldBlock { source: ErrorSource::SignalTrigger })
            }
        })?;
        *lock = Box::new(trigger);
        Ok(())
    }

    /// Removes the [`Signal`] trigger condition.
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if the `trigger` lock is poisoned.
    /// 
    /// # Warnings
    /// 
    /// This function may result in a deadlock if used incorrectly.
    /// 
    /// For a non-blocking alternative see [`try_remove_trigger`]
    pub fn remove_trigger(&self) -> Result<(), SignaledError> {
        let mut lock = self.trigger.lock().map_err(|_| signaled_error(ErrorType::PoisonedLock { source: ErrorSource::SignalTrigger }))?;
        *lock = Box::new(|_, _| true);
        Ok(())
    }

    /// Removes the [`Signal`] trigger condition.
    /// 
    /// This function unlike [`remove_trigger`], is non-blocking so there are no re-entrant calls that block until the `trigger` lock can be acquired.
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if the `trigger` lock is poisoned or held elsewhere.
    pub fn try_remove_trigger(&self) -> Result<(), SignaledError> {
        let mut lock = self.trigger.try_lock().map_err(|e| {
            match e {
                TryLockError::Poisoned(_) => signaled_error(ErrorType::PoisonedLock { source: ErrorSource::SignalTrigger }),
                TryLockError::WouldBlock => signaled_error(ErrorType::WouldBlock { source: ErrorSource::SignalTrigger })
            }
        })?;
        *lock = Box::new(|_, _| true);
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
}

impl<T: Send + Sync + 'static> Display for Signal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Signal {{ id: {}, priority: {} }}", self.id, self.priority.load(Ordering::Relaxed))
    }
}


impl<T: Send + Sync + 'static> Default for Signal<T> {
    fn default() -> Self {
        Self {
            callback: Arc::new(Mutex::new(Box::new(|_, _| {}))),
            trigger: Arc::new(Mutex::new(Box::new(|_, _| true))),
            id: new_signal_id(),
            priority: AtomicU64::new(0),
            once: AtomicBool::new(false),
            mute: AtomicBool::new(false)
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
    signals: Arc<Mutex<Vec<Signal<T>>>>
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
            signals: Arc::new(Mutex::new(Vec::new()))
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
    /// Returns [`SignaledError`] if `val` or `signals` locks are poisoned.
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
    /// For a non-blocking alternative, see [`try_set`].
    pub fn set(&self, new_value: T) -> Result<(), SignaledError> {
        let mut guard = self.val
            .write()
            .map_err(|_| signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Value }))?;
        let old_value = std::mem::replace(&mut *guard, new_value);
        self.emit_signals(&old_value, &*guard)
    }

    /// Sets a new value for `val` and emits all [`Signal`]s.
    /// 
    /// This function unlike [`set`], is non-blocking so there are no re-entrant calls that block until the lock for `val` is acquired.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of the [`Signaled`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError`] if `val` or `signals` locks are poisoned or held elsewhere.
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
        let mut guard = self.val
            .try_write()
            .map_err(|e| {
                match e {
                    TryLockError::Poisoned(_) => signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Value }),
                    TryLockError::WouldBlock => signaled_error(ErrorType::WouldBlock { source: ErrorSource::Value })
                }
            })?;
        let old_value = std::mem::replace(&mut *guard, new_value);
        self.try_emit_signals(&old_value, &*guard)
    }

    /// Returns the lock of the current value.
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if `val` lock is poisoned.
    /// 
    /// # Warnings
    /// 
    /// This function may result in a deadlock if used incorrectly.
    /// 
    /// For a non-blocking alternative, see [`try_get_lock`].
    pub fn get_lock(&self) -> Result<RwLockReadGuard<'_, T>, SignaledError> {
        match self.val.read() {
            Ok(r) => Ok(r),
            Err(_) => Err(signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Value })) 
        }
    }

    /// Returns the lock of the current value.
    /// 
    /// This function unlike [`get_lock`], is non-blocking so there are no re-entrant calls that block until the lock is acquired.
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if `val` lock is poisoned or held elsewhere.
    pub fn try_get_lock(&self) -> Result<RwLockReadGuard<'_, T>, SignaledError> {
        match self.val.try_read() {
            Ok(r) => Ok(r),
            Err(TryLockError::WouldBlock) => Err(signaled_error(ErrorType::WouldBlock { source: ErrorSource::Value })),
            Err(TryLockError::Poisoned(_)) => Err(signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Value }))
        }
    }

    /// Emits all [`Signal`]s in descending priority order, invoking their callbacks if their trigger condition is met.
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if the [`Signal`] collection lock or [`Signal`]s inside `signals` locks are poisoned.
    /// 
    /// # Warnings
    /// 
    /// This function may result in a deadlock if used incorrectly.
    /// 
    /// For a non-blocking alternative, see [`try_emit_signals`].
    fn emit_signals(&self, old: &T, new: &T) -> Result<(), SignaledError> {
        match self.signals.lock() {
            Ok(mut signals) => {
                signals.sort_by(|a, b| { 
                    b.priority.load(Ordering::Relaxed).cmp(&a.priority.load(Ordering::Relaxed))
                });
                for signal in signals.iter() {
                    signal.emit(old, new)?
                }
                signals.retain(|s| !s.once.load(Ordering::Relaxed));
                Ok(())
            }
            Err(_) => Err(signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals }))
        }
    }

    /// Emits all [`Signal`]s in descending priority order, invoking their callbacks if their trigger condition is met.
    /// 
    /// This function unlike [`emit_signals`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if the [`Signal`] collection lock or [`Signal`]s inside `signals` locks are poisoned or held elsewhere.
    fn try_emit_signals(&self, old: &T, new: &T) -> Result<(), SignaledError> {
        match self.signals.try_lock() {
            Ok(mut signals) => {
                signals.sort_by(|a, b| { 
                    b.priority.load(Ordering::Relaxed).cmp(&a.priority.load(Ordering::Relaxed))
                });
                for signal in signals.iter() {
                    signal.try_emit(old, new)?
                }
                signals.retain(|s| !s.once.load(Ordering::Relaxed));
                Ok(())
            }
            Err(TryLockError::Poisoned(_)) => Err(signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals })),
            Err(TryLockError::WouldBlock) => Err(signaled_error(ErrorType::WouldBlock { source: ErrorSource::Signals }))
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
    /// Returns [`SignaledError`] if the [`Signal`] collection lock is poisoned.
    /// 
    /// # Warnings
    /// 
    /// This function may result in a deadlock if used incorrectly.
    /// 
    /// For a non-blocking alternative, see [`try_add_signal`].
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
            Err(_) => Err(signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals }))
        }
    }

    /// Adds a signal to the collection, returning its [`SignalId`].
    /// 
    /// If a [`Signal`] with the same `id` is already in the collection, returns the existing `id` without adding the [`Signal`] to the collection.
    /// 
    /// This function unlike [`add_signal`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    /// 
    /// # Arguments
    /// * `signal` - The [`Signal`] to add.
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if the [`Signal`] collection lock is poisoned or held elsewhere.
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
            Err(TryLockError::Poisoned(_)) => Err(signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals })),
            Err(TryLockError::WouldBlock) => Err(signaled_error(ErrorType::WouldBlock { source: ErrorSource::Signals }))
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
    /// Returns [`SignaledError`] if the [`Signal`] collection lock is poisoned or if the `id` is invalid.
    /// 
    /// # Warnings 
    /// 
    /// This function may result in a deadlock if used incorrectly.
    /// 
    /// For a non-blocking alternative, see [`try_remove_signal`].
    pub fn remove_signal(&self, id: SignalId) -> Result<Signal<T>, SignaledError> {
        match self.signals.lock() {
            Ok(mut s) => {
                let index = s
                    .iter()
                    .position(|s| s.id == id)
                    .ok_or_else(|| signaled_error(ErrorType::InvalidSignalId { id: id }))?;
                Ok(s.remove(index))
            }
            Err(_) => Err(signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals }))
        }
    }

    /// Removes a [`Signal`] by `id`, returning the removed [`Signal`].
    /// 
    /// This function unlike [`remove_signal`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`] to remove.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError`] if the [`Signal`] collection lock is poisoned or held elsewhere, or if the `id` is invalid.
    pub fn try_remove_signal(&self, id: SignalId) -> Result<Signal<T>, SignaledError> {
        match self.signals.try_lock() {
            Ok(mut s) => {
                let index = s
                    .iter()
                    .position(|s| s.id == id)
                    .ok_or_else(|| signaled_error(ErrorType::InvalidSignalId { id: id }))?;
                Ok(s.remove(index))
            }
            Err(TryLockError::Poisoned(_)) => Err(signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals })),
            Err(TryLockError::WouldBlock) => Err(signaled_error(ErrorType::WouldBlock { source: ErrorSource::Signals }))
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
    /// Returns [`SignaledError`] if the [`Signal`] collection or [`Signal`] callback locks are poisoned or if the provided [`SignalId`] does not match any [`Signal`].
    /// 
    /// # Warnings
    /// 
    /// This function may result in a deadlock if used incorrectly.
    /// 
    /// For a non-blocking alternative, see [`try_set_signal_callback`].
    pub fn set_signal_callback<F: Fn(&T, &T) + Send + Sync + 'static>(&self, id: SignalId, callback: F) -> Result<(), SignaledError> {
        let signals = self.signals
            .lock()
            .map_err(|_| signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.set_callback(callback)
        } else {
            return Err(signaled_error(ErrorType::InvalidSignalId { id: id }))
        }
    }

    /// Sets the callback for a [`Signal`] by `id`.
    /// 
    /// This function unlike [`set_signal_callback`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    ///
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`].
    /// * `callback` - The new callback function.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError`] if the [`Signal`] collection or [`Signal`] callback locks are poisoned or held elsewhere, or if the provided [`SignalId`] does not match any [`Signal`].
    pub fn try_set_signal_callback<F: Fn(&T, &T) + Send + Sync + 'static>(&self, id: SignalId, callback: F) -> Result<(), SignaledError> {
        let signals = self.signals
            .try_lock()
            .map_err(|e| {
                match e {
                    TryLockError::Poisoned(_) => signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals }),
                    TryLockError::WouldBlock => signaled_error(ErrorType::WouldBlock { source: ErrorSource::Signals })
                }
            })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.try_set_callback(callback)
        } else {
            return Err(signaled_error(ErrorType::InvalidSignalId { id: id }))
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
    /// Returns [`SignaledError`] if the [`Signal`] collection or [`Signal`] trigger locks are poisoned or if the provided [`SignalId`] does not match any [`Signal`].
    /// 
    /// # Warnings
    /// 
    /// This function may result in a deadlock if used incorrectly.
    /// 
    /// For a non-blocking alternative, see [`try_set_signal_trigger`].
    pub fn set_signal_trigger<F: Fn(&T, &T) -> bool + Send + Sync + 'static>(&self, id: SignalId, trigger: F) -> Result<(), SignaledError> {
        let signals = self.signals.lock().map_err(|_| signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.set_trigger(trigger)
        } else {
            return Err(signaled_error(ErrorType::InvalidSignalId { id: id }))
        }
    }

    /// Sets the trigger condition for a [`Signal`] by `id`.
    ///
    /// This function unlike [`set_signal_trigger`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    /// 
    /// # Arguments
    ///
    /// * `id` - The `id` of the [`Signal`].
    /// * `trigger` - The new trigger function.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError`] if the [`Signal`] collection or [`Signal`] trigger locks are poisoned or held elsewhere, or if the provided [`SignalId`] does not match any [`Signal`].
    pub fn try_set_signal_trigger<F: Fn(&T, &T) -> bool + Send + Sync + 'static>(&self, id: SignalId, trigger: F) -> Result<(), SignaledError> {
        let signals = self.signals.try_lock().map_err(|e| {
            match e {
                TryLockError::Poisoned(_) => signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals }),
                TryLockError::WouldBlock => signaled_error(ErrorType::WouldBlock { source: ErrorSource::Signals })       
            }
        })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.try_set_trigger(trigger)
        } else {
            return Err(signaled_error(ErrorType::InvalidSignalId { id: id }))
        }
    }

    /// Removes the trigger condition for a [`Signal`] by `id`.
    /// 
    /// # Arguments
    /// 
    /// * `id` - The `id` of the [`Signal`].
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if the [`Signal`] collection or [`Signal`] trigger locks are poisoned or if the provided [`SignalId`] does not match any [`Signal`].
    /// 
    /// # Warnings
    /// 
    /// This function may result in a deadlock if used incorrectly.
    /// 
    /// For a non-blocking alternative, see [`try_remove_signal_trigger`].
    pub fn remove_signal_trigger(&self, id: SignalId) -> Result<(), SignaledError> {
        let signals = self.signals.lock().map_err(|_| signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.remove_trigger()
        } else {
            return Err(signaled_error(ErrorType::InvalidSignalId { id: id }))
        }
    }

    /// Removes the trigger condition for a [`Signal`] by `id`.
    /// 
    /// This function unlike [`remove_signal_trigger`], is non-blocking so there are no re-entrant calls that block until the `signals` lock can be acquired.
    /// 
    /// # Arguments
    /// 
    /// * `id` - The `id` of the [`Signal`].
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if the [`Signal`] collection or [`Signal`] trigger locks are poisoned or held elsewhere, or if the provided [`SignalId`] does not match any [`Signal`].
    pub fn try_remove_signal_trigger(&self, id: SignalId) -> Result<(), SignaledError> {
        let signals = self.signals.try_lock().map_err(|e| {
            match e {
                TryLockError::Poisoned(_) => signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals }),
                TryLockError::WouldBlock => signaled_error(ErrorType::WouldBlock { source: ErrorSource::Signals })
            }
        })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.try_remove_trigger()
        } else {
            return Err(signaled_error(ErrorType::InvalidSignalId { id: id }))
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
    /// Returns [`SignaledError`] if the [`Signal`] collection lock is poisoned or if the provided [`SignalId`] does not match any [`Signal`].
    pub fn set_signal_priority(&self, id: SignalId, priority: u64) -> Result<(), SignaledError> {
        let signals = self.signals
            .lock()
            .map_err(|_| signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_priority(priority);
            return Ok(())
        } else {
            return Err(signaled_error(ErrorType::InvalidSignalId { id: id }))
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
    /// Returns [`SignaledError`] if the [`Signal`] collection lock is poisoned or if the provided [`SignalId`] does not match any [`Signal`].
    pub fn set_signal_once(&self, id: SignalId, is_once: bool) -> Result<(), SignaledError> {
        let signals = self.signals
            .lock()
            .map_err(|_| signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_once(is_once);
            return Ok(())
        } else {
            return Err(signaled_error(ErrorType::InvalidSignalId { id: id }))
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
    /// Returns [`SignaledError`] if the [`Signal`] collection lock is poisoned or if the provided [`SignalId`] does not match any [`Signal`].
    pub fn set_signal_mute(&self, id: SignalId, is_mute: bool) -> Result<(), SignaledError> {
        let signals = self.signals
            .lock()
            .map_err(|_| signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_mute(is_mute);
            return Ok(())
        } else {
            return Err(signaled_error(ErrorType::InvalidSignalId { id: id }))
        }
    }

}

impl<T: Display + Send + Sync + 'static> Display for Signaled<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = self.val.read()
            .map(|v| format!("{}", *v))
            .unwrap_or_else(|_| "<poisoned>".to_string());
        let mut signals_len = 0;
        let signals_string: String = self.signals.lock().map(|s| {
            signals_len = s.len();
            s.iter()
                .map(|signal| format!("{}", signal))
                .collect::<Vec<_>>()
                .join(", ")
        }).unwrap_or_else(|_| "<poisoned>".to_string());
        write!(f, "Signaled {{ val: {}, signal_count: {}, signals: [{}] }}", value, signals_len, signals_string)
    }
}


impl<T: Clone + Send + Sync + 'static> Signaled<T> {
    /// Returns a cloned copy of the current value.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError`] if `val` lock is poisoned.
    /// 
    /// # Warnings
    /// 
    /// This function may result in a deadlock if used incorrectly.
    /// 
    /// For a deadlock-safe version, see [`try_get`].
    pub fn get(&self) -> Result<T, SignaledError> {
        match self.val.read() {
            Ok(r) => Ok(r.clone()),
            Err(_) => Err(signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Value }))
        }
    }

    /// Returns a cloned copy of the current value.
    /// 
    /// This function unlike [`get`], is non-blocking so there are no re-entrant calls that block until the lock is acquired.
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if `val` lock is poisoned or held elsewhere.
    pub fn try_get(&self) -> Result<T, SignaledError> {
        match self.val.try_read() {
            Ok(r) => Ok(r.clone()),
            Err(TryLockError::WouldBlock) => Err(signaled_error(ErrorType::WouldBlock { source: ErrorSource::Value })),
            Err(TryLockError::Poisoned(_)) => Err(signaled_error(ErrorType::PoisonedLock { source: ErrorSource::Value }))
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
        signaled.add_signal(signal_sync!(move |_, _| { 
            let mut lock = calls_clone.lock().unwrap();
            *lock = *lock + 1;
        })).unwrap();

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
        let signal_id = signaled.add_signal(signal_sync!(move |_, _| { 
            let mut lock = calls_clone.lock().unwrap();
            *lock = *lock + 1;
        })).unwrap();
        
        signaled.set(2).unwrap(); // Calls = 1
        assert_eq!(*calls.lock().unwrap(), 1);

        signaled.set(3).unwrap(); // Calls = 2
        assert_eq!(*calls.lock().unwrap(), 2);

        signaled.remove_signal(signal_id).unwrap(); // Signal removed so `calls` will not increase when `signaled` emits.

        signaled.set(4).unwrap();// Calls = 2
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
        let signal_id = signaled.add_signal(signal_sync!(move |_, _| { 
            let mut lock = calls_clone.lock().unwrap();
            *lock = *lock + 1;
        })).unwrap();
        
        signaled.set(2).unwrap(); // Calls = 1
        assert_eq!(*calls.lock().unwrap(), 1);

        signaled.set(3).unwrap(); // Calls = 2
        assert_eq!(*calls.lock().unwrap(), 2);

        let calls_clone = Arc::clone(&calls);
        signaled.set_signal_callback(signal_id,  move |_, _| {
            let mut lock = calls_clone.lock().unwrap();
            *lock = *lock + 2;
        }).unwrap(); // New callback increases call count by 2.

        signaled.set(4).unwrap();// Calls = 4
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

        signaled.set_signal_trigger(signal_id, |_, new| { *new < 5 }).unwrap(); // Trigger condition changed so `new_value` being < 5 will invoke callback.

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
        let signal_b: Signal<i32> = signal_sync!(move |_, _| { assert_eq!(*test_value_clone.lock().unwrap(), 'a'); }); // Signal in the middle with a callback that checks that `signal_a` callback has been invoked first.
        signal_b.set_priority(2);

        let test_value_clone = Arc::clone(&test_value);
        let signal_c: Signal<i32> = signal_sync!(move |_, _| {
            *test_value_clone.lock().unwrap() = 'c'
        });
        signal_c.set_priority(1);

        let signal_a_id = signaled.add_signal(signal_a).unwrap();
        let signal_b_id = signaled.add_signal(signal_b).unwrap();
        let signal_c_id = signaled.add_signal(signal_c).unwrap();

        signaled.set(0).unwrap();
        assert_eq!(*test_value.lock().unwrap(), 'c'); // `signal_c` has the lowest priority (1) so after all callbacks are invoked the value will be 'c'.

        signaled.set_signal_priority(signal_a_id, 1).unwrap(); // Set priority to 1, `signal_a` will now be the last to execute.

        let test_value_clone = Arc::clone(&test_value);
        signaled.set_signal_callback(signal_b_id, move |_, _| { assert_eq!(*test_value_clone.lock().unwrap(), 'c') }).unwrap(); // Modify signal in the middle callback to check that `signal_c` callback has been invoked first.

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
    fn test_get_lock() {
        let signaled = Signaled::new(0);
        assert_eq!(*signaled.get_lock().unwrap(), 0);

        for i in 0..10 {
            signaled.set(i).unwrap();
            assert_eq!(*signaled.get_lock().unwrap(), i);
        }
    }

    #[test]
    fn test_get() {
        let signaled = Signaled::new(0);
        assert_eq!(signaled.get().unwrap(), 0);

        for i in 0..10 {
            signaled.set(i).unwrap();
            assert_eq!(signaled.get().unwrap(), i);
        }
    }

    #[test]
    fn test_old_new() {
        {
            let signaled = Signaled::new(0);
            signaled.add_signal(signal_sync!(|old, new| assert!(*new == *old + 1))).unwrap();
            
            for i in 1..10 {
                signaled.set(i).unwrap();
            }
        }
        {
            let signaled = Signaled::new(0);
            signaled.add_signal(signal_sync!(|old, new| assert!(*new == *old + 1))).unwrap();
            
            for i in 1..10 {
                signaled.try_set(i).unwrap();
            }
        }
    }

    #[test]
    fn test_try_get_lock() {
        let signaled = Signaled::new(0);
        assert_eq!(*signaled.try_get_lock().unwrap(), 0);

        for i in 0..10 {
            signaled.set(i).unwrap();
            assert_eq!(*signaled.try_get_lock().unwrap(), i);
        }
    }

    #[test]
    fn test_try_get() {
        let signaled = Signaled::new(0);
        assert_eq!(signaled.try_get().unwrap(), 0);
        for i in 0..10 {
            signaled.set(i).unwrap();
            assert_eq!(signaled.try_get().unwrap(), i)
        }
    }

    #[test]
    fn test_try_set() {
        let calls = Arc::new(Mutex::new(0));
        let signaled = Signaled::new(0);

        let calls_clone = Arc::clone(&calls);
        signaled.add_signal(signal_sync!(move |_, _| {
            let mut lock = calls_clone.lock().unwrap();
            *lock = *lock + 1;
        })).unwrap();

        for i in 0..10 {
            signaled.try_set(i).unwrap();
            assert_eq!(signaled.get().unwrap(), i);
        }

        assert_eq!(*calls.lock().unwrap(), 10);
    }

    #[test]
    fn test_multithread_set() {
        let calls = Arc::new(Mutex::new(0));

        let signaled = Arc::new(Mutex::new(Signaled::new(())));

        let calls_clone = Arc::clone(&calls);
        signaled.lock().unwrap().add_signal(signal_sync!(move |_, _| {
            let mut lock = calls_clone.lock().unwrap();
            *lock = *lock + 1;
        })).unwrap();

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

    #[test] 
    fn test_try_set_would_block_error() {
        let signaled = Signaled::new(0);
        let _read_lock = signaled.get_lock().unwrap();
        assert!(signaled.try_set(1).is_err_and(|e| e.message == "Cannot access Signaled value: lock is already held elsewhere"));
    }

    #[test]
    fn test_try_get_would_block_error() {
        let signaled = Signaled::new(0);
        let _write_lock = signaled.val.write().unwrap();
        assert!(signaled.try_get().is_err_and(|e| e.message == "Cannot access Signaled value: lock is already held elsewhere"));
    }

    #[test]
    fn test_try_get_lock_would_block_error() {
        let signaled = Signaled::new(0);
        let _write_lock = signaled.val.write().unwrap();
        assert!(signaled.try_get_lock().is_err_and(|e| e.message == "Cannot access Signaled value: lock is already held elsewhere"));
    }

    #[test]
    fn test_try_emit_signals_would_block_error() {
        let signaled = Signaled::new(0);
        signaled.add_signal(signal_sync!(|_, _| {})).unwrap();
        let _lock = signaled.signals.lock().unwrap();
        assert!(signaled.try_emit_signals(&1, &2).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is already held elsewhere"));
    }

    #[test]
    fn test_try_add_signal_would_block_error() {
        let signaled = Signaled::new(0);
        let _lock = signaled.signals.lock().unwrap();
        assert!(signaled.try_add_signal(signal_sync!(|_, _| {})).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is already held elsewhere"));
    }

    #[test]
    fn test_try_remove_signal_would_block_error() {
        let signaled = Signaled::new(0);
        let signal_id = signaled.add_signal(signal_sync!(|_, _| {})).unwrap();
        let _lock = signaled.signals.lock().unwrap();
        assert!(signaled.try_remove_signal(signal_id).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is already held elsewhere"));
    }

    #[test]
    fn test_try_set_signal_callback_would_block_error() {
        let signaled = Signaled::new(0);
        let signal_id = signaled.add_signal(signal_sync!(|_, _| {})).unwrap();
        let _lock = signaled.signals.lock().unwrap();
        assert!(signaled.try_set_signal_callback(signal_id, |_, _| {}).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is already held elsewhere"));
    }

    #[test]
    fn test_try_set_signal_trigger_would_block_error() {
        let signaled = Signaled::new(0);
        let signal_id = signaled.add_signal(signal_sync!(|_, _| {})).unwrap();
        let _lock = signaled.signals.lock().unwrap();
        assert!(signaled.try_set_signal_trigger(signal_id, |_, _| true).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is already held elsewhere"));
    }
    
    #[test]
    fn test_try_remove_signal_trigger_would_block_error() {
        let signaled = Signaled::new(0);
        let signal_id = signaled.add_signal(signal_sync!(|_, _| {})).unwrap();
        let _lock = signaled.signals.lock().unwrap();
        assert!(signaled.try_remove_signal_trigger(signal_id).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is already held elsewhere"));
    }

    #[test]
    fn test_try_emit_would_block_error() {
        {
            let signal: Signal<i32> = signal_sync!(|_, _| {});
            let _lock = signal.trigger.lock().unwrap();
            assert!(signal.try_emit(&1, &2).is_err_and(|e| e.message == "Cannot access Signal trigger: lock is already held elsewhere"));
        }
        {
            let signal: Signal<i32> = signal_sync!(|_, _| {});
            let _lock = signal.callback.lock().unwrap();
            assert!(signal.try_emit(&1, &2).is_err_and(|e| e.message == "Cannot access Signal callback: lock is already held elsewhere"));
        }
    }

    #[test]
    fn test_try_set_callback_would_block_error() {
        let signal: Signal<i32> = signal_sync!(|_, _| {});
        let _lock = signal.callback.lock().unwrap();
        assert!(signal.try_set_callback(|_, _| {}).is_err_and(|e| e.message == "Cannot access Signal callback: lock is already held elsewhere"));
    }

    #[test]
    fn test_try_set_trigger_would_block_error() {
        let signal: Signal<i32> = signal_sync!(|_, _| {});
        let _lock = signal.trigger.lock().unwrap();
        assert!(signal.try_set_trigger(|_, _| true).is_err_and(|e| e.message == "Cannot access Signal trigger: lock is already held elsewhere"));
    }

    #[test]
    fn test_try_remove_trigger_would_block_error() {
        let signal: Signal<i32> = signal_sync!(|_, _| {});
        let _lock = signal.trigger.lock().unwrap();
        assert!(signal.try_remove_trigger().is_err_and(|e| e.message == "Cannot access Signal trigger: lock is already held elsewhere"));
    }

    fn create_poisoned_signaled() -> (Arc<Signaled<i32>>, SignalId) {
        let signaled = Arc::new(Signaled::new(0));
        let signal_id = signaled.add_signal(signal_sync!(|_, _| panic!())).unwrap();

        let signaled_clone = Arc::clone(&signaled);
        let result = std::panic::catch_unwind(move || {
            let _ = signaled_clone.set(1);
        });
        assert!(result.is_err());
        (signaled, signal_id)
    }

    #[test]
    fn test_set_poisoned_lock_error() {
        let (signaled, _) = create_poisoned_signaled();
        assert!(signaled.set(1).is_err_and(|e| e.message == "Cannot access Signaled value: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_try_set_poisoned_lock_error() {
        let (signaled, _) = create_poisoned_signaled();
        assert!(signaled.try_set(1).is_err_and(|e| e.message == "Cannot access Signaled value: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_get_poisoned_lock_error() {
        let (signaled, _) = create_poisoned_signaled();
        assert!(signaled.get().is_err_and(|e| e.message == "Cannot access Signaled value: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_try_get_poisoned_lock_error() {
        let (signaled, _) = create_poisoned_signaled();
        assert!(signaled.try_get().is_err_and(|e| e.message == "Cannot access Signaled value: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_get_lock_poisoned_lock_error() {
        let (signaled, _) = create_poisoned_signaled();
        assert!(signaled.get_lock().is_err_and(|e| e.message == "Cannot access Signaled value: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_try_get_lock_poisoned_lock_error() {
        let (signaled, _) = create_poisoned_signaled();
        assert!(signaled.try_get_lock().is_err_and(|e| e.message == "Cannot access Signaled value: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_emit_signals_poisoned_lock_error() {
        let (signaled, _) = create_poisoned_signaled();
        assert!(signaled.emit_signals(&1, &2).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_try_emit_signals_poisoned_lock_error() {
        let (signaled, _) = create_poisoned_signaled();
        assert!(signaled.try_emit_signals(&1, &2).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_add_signal_poisoned_lock_error() {
        let (signaled, _) = create_poisoned_signaled();
        assert!(signaled.add_signal(signal_sync!(|_, _| {})).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_try_add_signal_poisoned_lock_error() {
        let (signaled, _) = create_poisoned_signaled();
        assert!(signaled.try_add_signal(signal_sync!(|_, _| {})).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_remove_signal_poisoned_lock_error() {
        let (signaled, signal_id) = create_poisoned_signaled();
        assert!(signaled.remove_signal(signal_id).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_try_remove_signal_poisoned_lock_error() {
        let (signaled, signal_id) = create_poisoned_signaled();
        assert!(signaled.try_remove_signal(signal_id).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_set_signal_callback_poisoned_lock_error() {
        let (signaled, signal_id) = create_poisoned_signaled();
        assert!(signaled.set_signal_callback(signal_id, |_, _| {}).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_try_set_signal_callback_poisoned_lock_error() {
        let (signaled, signal_id) = create_poisoned_signaled();
        assert!(signaled.try_set_signal_callback(signal_id, |_, _| {}).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_set_signal_trigger_poisoned_lock_error() {
        let (signaled, signal_id) = create_poisoned_signaled();
        assert!(signaled.set_signal_trigger(signal_id, |_, _| true).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_try_set_signal_trigger_poisoned_lock_error() {
        let (signaled, signal_id) = create_poisoned_signaled();
        assert!(signaled.try_set_signal_trigger(signal_id, |_, _| true).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_remove_signal_trigger_poisoned_lock_error() {
        let (signaled, signal_id) = create_poisoned_signaled();
        assert!(signaled.remove_signal_trigger(signal_id).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_try_remove_signal_trigger_poisoned_lock_error() {
        let (signaled, signal_id) = create_poisoned_signaled();
        assert!(signaled.remove_signal_trigger(signal_id).is_err_and(|e| e.message == "Cannot access Signaled signals: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_emit_poisoned_lock_error() {
        {
            let signal = Arc::new(signal_sync!(|_, _| {}));
            let signal_clone = Arc::clone(&signal);
            let result = std::panic::catch_unwind(move || {
                let _lock = signal_clone.callback.lock().unwrap();
                panic!();
            });

            assert!(result.is_err());
            assert!(signal.emit(&1, &2).is_err_and(|e| e.message == "Cannot access Signal callback: lock is poisoned due to a previous panic"));
        }
        {
            let signal = Arc::new(signal_sync!(|_, _| {}));
            let signal_clone = Arc::clone(&signal);
            let result = std::panic::catch_unwind(move || {
                let _lock = signal_clone.trigger.lock().unwrap();
                panic!();
            });

            assert!(result.is_err());
            assert!(signal.emit(&1, &2).is_err_and(|e| e.message == "Cannot access Signal trigger: lock is poisoned due to a previous panic"));
        }
    }

    #[test]
    fn test_try_emit_poisoned_lock_error() {
        {
            let signal = Arc::new(signal_sync!(|_, _| {}));
            let signal_clone = Arc::clone(&signal);
            let result = std::panic::catch_unwind(move || {
                let _lock = signal_clone.callback.lock().unwrap();
                panic!();
            });

            assert!(result.is_err());
            assert!(signal.try_emit(&1, &2).is_err_and(|e| e.message == "Cannot access Signal callback: lock is poisoned due to a previous panic"));
        }
        {
            let signal = Arc::new(signal_sync!(|_, _| {}));
            let signal_clone = Arc::clone(&signal);
            let result = std::panic::catch_unwind(move || {
                let _lock = signal_clone.trigger.lock().unwrap();
                panic!();
            });

            assert!(result.is_err());
            assert!(signal.try_emit(&1, &2).is_err_and(|e| e.message == "Cannot access Signal trigger: lock is poisoned due to a previous panic"));
        }
    }

    #[test]
    fn test_set_callback_poisoned_lock_error() {
        let signal: Arc<Signal<i32>> = Arc::new(signal_sync!(|_, _| {}));
        let signal_clone = Arc::clone(&signal);
        let result = std::panic::catch_unwind(move || {
            let _lock = signal_clone.callback.lock().unwrap();
            panic!();
        });

        assert!(result.is_err());
        assert!(signal.set_callback(|_, _| {}).is_err_and(|e| e.message == "Cannot access Signal callback: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_try_set_callback_poisoned_lock_error() {
        let signal: Arc<Signal<i32>> = Arc::new(signal_sync!(|_, _| {}));
        let signal_clone = Arc::clone(&signal);
        let result = std::panic::catch_unwind(move || {
            let _lock = signal_clone.callback.lock().unwrap();
            panic!();
        });

        assert!(result.is_err());
        assert!(signal.try_set_callback(|_, _| {}).is_err_and(|e| e.message == "Cannot access Signal callback: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_set_trigger_poisoned_lock_error() {
        let signal: Arc<Signal<i32>> = Arc::new(signal_sync!(|_, _| {}));
        let signal_clone = Arc::clone(&signal);
        let result = std::panic::catch_unwind(move || {
            let _lock = signal_clone.trigger.lock().unwrap();
            panic!();
        });

        assert!(result.is_err());
        assert!(signal.set_trigger(|_, _| true).is_err_and(|e| e.message == "Cannot access Signal trigger: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_try_set_trigger_poisoned_lock_error() {
        let signal: Arc<Signal<i32>> = Arc::new(signal_sync!(|_, _| {}));
        let signal_clone = Arc::clone(&signal);
        let result = std::panic::catch_unwind(move || {
            let _lock = signal_clone.trigger.lock().unwrap();
            panic!();
        });

        assert!(result.is_err());
        assert!(signal.try_set_trigger(|_, _| true).is_err_and(|e| e.message == "Cannot access Signal trigger: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_remove_trigger_poisoned_lock_error() {
        let signal: Arc<Signal<i32>> = Arc::new(signal_sync!(|_, _| {}));
        let signal_clone = Arc::clone(&signal);
        let result = std::panic::catch_unwind(move || {
            let _lock = signal_clone.trigger.lock().unwrap();
            panic!();
        });

        assert!(result.is_err());
        assert!(signal.remove_trigger().is_err_and(|e| e.message == "Cannot access Signal trigger: lock is poisoned due to a previous panic"));
    }

    #[test]
    fn test_try_remove_trigger_poisoned_lock_error() {
        let signal: Arc<Signal<i32>> = Arc::new(signal_sync!(|_, _| {}));
        let signal_clone = Arc::clone(&signal);
        let result = std::panic::catch_unwind(move || {
            let _lock = signal_clone.trigger.lock().unwrap();
            panic!();
        });

        assert!(result.is_err());
        assert!(signal.try_remove_trigger().is_err_and(|e| e.message == "Cannot access Signal trigger: lock is poisoned due to a previous panic"));
    }

    fn create_signaled_with_invalid_id() -> (Signaled<i32>, SignalId) {
        let signaled = Signaled::new(0);
        let signal_id = signaled.add_signal(signal_sync!(|_, _| {})).unwrap();
        let invalid_id = signal_id + 1;
        (signaled, invalid_id)
    }

    macro_rules! test_invalid_id_error {
        ($test_name:ident, $method:ident, $($args:expr),*) => {
            #[test]
            fn $test_name() {
                let (signaled, invalid_id) = create_signaled_with_invalid_id();
                assert!(matches!(
                    signaled.$method(invalid_id, $($args),*),
                    Err(SignaledError { message }) if message == format!("Signal ID '{}' does not match any Signal", invalid_id))
                );
            }
        };
    }
    test_invalid_id_error!(test_remove_signal_invalid_signal_id_error, remove_signal,);
    test_invalid_id_error!(test_try_remove_signal_invalid_signal_id_error, try_remove_signal,);
    test_invalid_id_error!(test_set_signal_callback_invalid_signal_id_error, set_signal_callback, |_, _| {});
    test_invalid_id_error!(test_try_set_signal_callback_invalid_signal_id_error, try_set_signal_callback, |_, _| {});
    test_invalid_id_error!(test_set_signal_trigger_invalid_signal_id_error, set_signal_trigger, |_, _| true);
    test_invalid_id_error!(test_try_set_signal_trigger_invalid_signal_id_error, try_set_signal_trigger, |_, _| true);
    test_invalid_id_error!(test_remove_signal_trigger_invalid_signal_id_error, remove_signal_trigger,);
    test_invalid_id_error!(test_try_remove_signal_trigger_invalid_signal_id_error, try_remove_signal_trigger,);
    test_invalid_id_error!(test_set_signal_priority_invalid_signal_id_error, set_signal_priority, 1);
    test_invalid_id_error!(test_set_signal_once_invalid_signal_id_error, set_signal_once, true);
    test_invalid_id_error!(test_set_signal_mute_invalid_signal_id_error, set_signal_mute, true);

}