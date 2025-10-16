//! # Signaled
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
//! - **Safe Mutability**: Uses `RefCell` for interior mutability with runtime borrow checking.
//! - **Error Handling**: Returns `Result` with `SignaledError` for borrow conflicts and invalid signal IDs.
//! - **Multi-threading**: `signaled::sync` inner module with thread-safe versions of `Signaled` and `Signal`.
//!
//! ## Limitations
//!
//! - Recursive calls to `set` or `emit` in signal callbacks may cause borrow errors.
//! - Re-entrant calls (e.g. calling `set` from within the callback of a `Signal<T>`) may panic due borrow errors.
//!
//! ## Examples
//!
//! Basic usage to create a `Signaled<i32>`, add a signal, and emit changes:
//!
//! ```
//! use signaled::{Signaled, Signal, SignaledError, signal};
//!
//! let signaled = Signaled::new(0);
//! let signal = signal!(|old: &i32, new: &i32| println!("Old: {} | New: {}", old, new));
//! signaled.add_signal(signal).unwrap();
//! signaled.set(42).unwrap(); // Prints "Old: 0 | New: 42"
//! ```
//!
//! Using priorities and triggers:
//!
//! ```
//! use signaled::{Signaled, Signal, SignaledError, signal};
//!
//! let signaled = Signaled::new(0);
//! let high_priority = signal!(|old: &i32, new: &i32| println!("High: Old: {}, New: {}", old, new));
//! high_priority.set_priority(10);
//! let conditional = signal!(|old: &i32, new: &i32| println!("Conditional: Old: {}, New: {}", old, new));
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
//! - `BorrowError`: Attempted to immutably borrow a value already mutably borrowed.
//! - `BorrowMutError`: Attempted to mutably borrow a value already borrowed.
//! - `InvalidSignalId`: Provided a `Signal` ID that does not exist.
//!
//! ```
//! use signaled::{Signaled, SignaledError, ErrorType, ErrorSource};
//!
//! let signaled = Signaled::new(0);
//! let _borrow = signaled.get_ref().unwrap();
//! let result = signaled.set(1);
//! assert!(matches!(
//!     result,
//!     Err(SignaledError { message }) if message == "Cannot mutably borrow Signaled value, it is already borrowed"
//! ));
//! ```

#![allow(dead_code)]
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;
use std::fmt::{Display};
use std::sync::atomic::{AtomicU64, Ordering};

pub mod sync;

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
    /// Attempted to immutably borrow a [`RefCell`] wrapped field that is already mutably borrowed.
    BorrowError { source: ErrorSource },
    /// Attempted to mutably borrow a [`RefCell`] wrapped field that is already borrowed.
    BorrowMutError { source: ErrorSource },
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
        ErrorType::BorrowError { source } => {
            match source {
                ErrorSource::SignalCallback => "Cannot borrow Signal callback, it is already mutably borrowed".to_string(),
                ErrorSource::SignalTrigger => "Cannot borrow Signal trigger, it is already mutably borrowed".to_string(),
                ErrorSource::Value => "Cannot borrow Signaled value, it is already mutably borrowed".to_string(),
                ErrorSource::Signals => "Cannot borrow Signaled signals, they are already mutably borrowed".to_string()
            }
        }
        ErrorType::BorrowMutError { source } => {
            match source {
                ErrorSource::SignalCallback => "Cannot mutably borrow Signal callback, it is already borrowed".to_string(),
                ErrorSource::SignalTrigger => "Cannot mutably borrow Signal trigger, it is already borrowed".to_string(),
                ErrorSource::Value => "Cannot mutably borrow Signaled value, it is already borrowed".to_string(),
                ErrorSource::Signals => "Cannot mutably borrow Signaled signals, they are already borrowed".to_string()
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
/// use signaled::{Signal, Signaled, signal};
///
/// let signal = signal!(|old: &i32, new: &i32| println!("Old: {} | New: {}", old, new));
/// let signaled = Signaled::new(0);
/// signaled.add_signal(signal).unwrap();
/// signaled.set(42).unwrap(); // Prints "Old: 0 | New: 42"
/// ```
pub struct Signal<T> {
    /// Function to run when the [`Signal`] is emitted and the trigger condition is met.
    callback: RefCell<Rc<dyn Fn(&T, &T)>>,
    /// Function that decides if the callback will be invoked or not when the [`Signal`] is emitted.
    trigger: RefCell<Rc<dyn Fn(&T, &T) -> bool>>,
    /// Identifier for the [`Signal`].
    id: u64,
    /// Number used in the [`Signaled`] struct to decide the order of execution of the signals.
    priority: Cell<u64>,
    /// Boolean representing if the [`Signal`] should be removed from the Signaled after being called once.
    once: Cell<bool>,
    /// Boolean representing if the [`Signal`] should not invoke the callback when emitted.
    mute: Cell<bool>
}

impl<T> Signal<T> {
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
    /// use signaled::{Signaled, Signal, signal};
    ///
    /// let signal = signal!(|old: &i32, new: &i32| println!("Old: {} | New: {}", old, new));
    /// let signaled = Signaled::new(5);
    /// signaled.add_signal(signal).unwrap();
    /// signaled.set(6).unwrap(); // Prints "Old: 5 | New: 6"
    /// ```
    pub fn new<F: Fn(&T, &T) + 'static, G: Fn(&T, &T) -> bool + 'static> (callback: F, trigger: G, priority: u64, once: bool, mute: bool) -> Self {
        Signal {
            callback: RefCell::new(Rc::new(callback)),
            trigger: RefCell::new(Rc::new(trigger)),
            id: new_signal_id(),
            priority: Cell::new(priority),
            once: Cell::new(once),
            mute: Cell::new(mute)
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
    /// Returns [`SignaledError`] if the callback or trigger are already borrowed.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{Signal, signal};
    ///
    /// let signal = signal!(|old: &i32, new: &i32| println!("Old: {}, New: {}", old, new));
    /// signal.set_trigger(|old: &i32, new: &i32| *new > *old + 5).unwrap();
    /// signal.emit(&4, &6).unwrap(); // Doesn't print because trigger condition is not met
    /// signal.emit(&4, &10).unwrap(); // Prints "Old: 4, New: 10"
    /// ```
    pub fn emit(&self, old: &T, new: &T) -> Result<(), SignaledError>{
        if self.mute.get() {
            return Ok(())
        }
        let trigger = self.trigger
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::SignalTrigger }))?;
        if trigger(old, new) {
            let callback = self.callback
                .try_borrow()
                .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::SignalCallback }))?;
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
    /// Returns [`SignaledError`] if the callback is already borrowed.
    pub fn set_callback<F: Fn(&T, &T) + 'static>(&self, callback: F) -> Result<(), SignaledError> {
        *self.callback.try_borrow_mut().map_err(|_| signaled_error(ErrorType::BorrowMutError { source: ErrorSource::SignalCallback }))? = Rc::new(callback);
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
    /// Returns [`SignaledError`] if the trigger is already borrowed.
    pub fn set_trigger<F: Fn(&T, &T) -> bool + 'static>(&self, trigger: F) -> Result<(), SignaledError> {
        *self.trigger.try_borrow_mut().map_err(|_| signaled_error(ErrorType::BorrowMutError { source: ErrorSource::SignalTrigger }))? = Rc::new(trigger);
        Ok(())
    }

    /// Removes the [`Signal`] trigger condition.
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if the trigger is already borrowed.
    pub fn remove_trigger(&self) -> Result<(), SignaledError> {
        *self.trigger.try_borrow_mut().map_err(|_| signaled_error(ErrorType::BorrowMutError { source: ErrorSource::SignalTrigger }))? = Rc::new(|_, _| true);
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
        self.priority.set(priority);
    }

    /// Sets the `once` flag of the [`Signal`].
    pub fn set_once(&self, is_once: bool) {
        self.once.set(is_once);
    }

    /// Sets the `mute` flag of the [`Signal`].
    pub fn set_mute(&self, is_mute: bool) {
        self.mute.set(is_mute);
    } 
}

impl<T> Display for Signal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Signal {{ id: {}, priority: {} }}", self.id, self.priority.get())
    }
}

impl<T> Clone for Signal<T> {
    /// Creates a clone of the [`Signal`], retaining the same `id`.
    /// 
    /// When a [`Signal`] is cloned the `id` is reutilized for easier identification since `callback` and `trigger` cannot be visualized.
    /// 
    /// # Panics
    /// 
    /// Panics if the callback or trigger are already mutably borrowed.
    /// 
    /// # Notes
    /// 
    /// Cloned signals share the same callback and trigger pointer.
    /// Modifying them through `set_callback` or `set_trigger` will only affect the targeted [`Signal`] since those methods drop the [`Rc`] wrapped inside the [`RefCell`] and create a new one
    fn clone(&self) -> Self {
        Signal {
            callback: self.callback.clone(),
            trigger: self.trigger.clone(),
            id: self.id.clone(),
            priority: self.priority.clone(),
            once: Cell::new(false),
            mute: Cell::new(false)
        }
    }
}

impl<T> Default for Signal<T> {
    fn default() -> Self {
        Self {
            callback: RefCell::new(Rc::new(|_, _| {})),
            trigger: RefCell::new(Rc::new(|_, _| true)),
            id: new_signal_id(),
            priority: Cell::new(0),
            once: Cell::new(false),
            mute: Cell::new(false)
        }
    }
}

/// Creates a [`Signal`] with any number of parameters.
///
/// # Examples
/// ```
/// use signaled::{Signal, signal};
/// let s1: Signal<i32> = signal!();
/// let s2: Signal<i32> = signal!(|_, _| println!("changed"));
/// let s3: Signal<i32> = signal!(|_, _| {}, |_, _| true, 1, false, true);
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
macro_rules! signal {
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
/// use signaled::{Signaled, Signal, SignaledError, signal};
///
/// let signaled = Signaled::new(0);
/// signaled.add_signal(signal!(|old, new| println!("Old: {} | New: {}", old, new))).unwrap();
/// signaled.set(42).unwrap(); // Prints "Old: 0 | New: 42"
/// ```
pub struct Signaled<T> {
    /// Reactive value, the mutation of this value through `set` will emit all [`Signal`] inside `signals`.
    val: RefCell<T>,
    /// Collection of [`Signal`]s that will be emitted when `val` is changed through `set`.
    signals: RefCell<Vec<Signal<T>>>
}

impl<T> Signaled<T> {
    /// Creates a new instance of [`Signaled`] with the given initial value.
    ///
    /// # Arguments
    ///
    /// * `val` - The initial value.
    pub fn new(val: T) -> Self {
        Self {
            val: RefCell::new(val),
            signals: RefCell::new(Vec::new())
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
    /// Returns [`SignaledError`] if `val` or `signals` are already borrowed.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{Signaled, Signal, signal};
    ///
    /// let signaled = Signaled::new(0);
    /// signaled.add_signal(signal!(|old, new| println!("Old: {} | New: {}", old, new)));
    /// signaled.set(1).unwrap(); /// Prints "Old: 0 | New: 1"
    /// ```
    pub fn set(&self, new_value: T) -> Result<(), SignaledError> {
        let old_value = std::mem::replace(
            &mut *self.val
                    .try_borrow_mut()
                    .map_err(|_| signaled_error(ErrorType::BorrowMutError { source: ErrorSource::Value }))?,
            new_value
        );
        let new_value_ref = self.val.try_borrow().map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Value }))?;
        self.emit_signals(&old_value, &new_value_ref)
    }

    /// Returns a reference to the current value.
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if `val` is mutably borrowed.
    pub fn get_ref(&self) -> Result<Ref<'_, T>, SignaledError> {
        match self.val.try_borrow() {
            Ok(r) => Ok(r),
            Err(_) => Err(signaled_error(ErrorType::BorrowError { source: ErrorSource::Value })) 
        }
    }

    /// Emits all [`Signal`]s in descending priority order, invoking their callbacks if their trigger condition is met.
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if any of the [`Signal`]s is already borrowed.
    fn emit_signals(&self, old: &T, new: &T) -> Result<(), SignaledError> {
        match self.signals.try_borrow_mut() {
            Ok(mut signals) => {
                signals.sort_by(|a, b| b.priority.get().cmp(&a.priority.get()));
                for signal in signals.iter() {
                    signal.emit(old, new)?
                }
                signals.retain(|s| !s.once.get());
                Ok(())
            }
            Err(_) => Err(signaled_error(ErrorType::BorrowMutError { source: ErrorSource::Signals }))
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
    /// Returns [`SignaledError`] if the [`Signal`] collection is already borrowed.
    pub fn add_signal(&self, signal: Signal<T>) -> Result<SignalId, SignaledError> {
        match self.signals.try_borrow_mut() {
            Ok(mut s) => {
                let id = signal.id;
                if s.iter().any(|sig| sig.id == id) {
                    return Ok(id);
                }
                s.push(signal);
                Ok(id)
            }
            Err(_) => Err(signaled_error(ErrorType::BorrowMutError { source: ErrorSource::Signals }))
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
    /// Returns [`SignaledError`] if the [`Signal`] collection is already borrowed or the `id` is invalid.
    pub fn remove_signal(&self, id: SignalId) -> Result<Signal<T>, SignaledError> {
        match self.signals.try_borrow_mut() {
            Ok(mut s) => {
                let index = s
                    .iter()
                    .position(|s| s.id == id)
                    .ok_or_else(|| signaled_error(ErrorType::InvalidSignalId { id: id }))?;
                Ok(s.remove(index))
            }
            Err(_) => Err(signaled_error(ErrorType::BorrowMutError { source: ErrorSource::Signals }))
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
    /// Returns [`SignaledError`] if the [`Signal`] collection or [`Signal`] callback are already borrowed or if the provided [`SignalId`] does not match any [`Signal`].
    pub fn set_signal_callback<F: Fn(&T, &T) + 'static>(&self, id: SignalId, callback: F) -> Result<(), SignaledError> {
        let signals = self.signals
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.set_callback(callback)
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
    /// Returns [`SignaledError`] if the [`Signal`] collection or [`Signal`] trigger are already borrowed or if the provided [`SignalId`] does not match any [`Signal`].
    pub fn set_signal_trigger<F: Fn(&T, &T) -> bool + 'static>(&self, id: SignalId, trigger: F) -> Result<(), SignaledError> {
        let signals = self.signals.try_borrow().map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.set_trigger(trigger)
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
    /// Returns [`SignaledError`] if the [`Signal`] collection or [`Signal`] trigger are already borrowed or if the provided [`SignalId`] does not match any [`Signal`].
    pub fn remove_signal_trigger(&self, id: SignalId) -> Result<(), SignaledError> {
        let signals = self.signals.try_borrow().map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.remove_trigger()
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
    /// Returns [`SignaledError`] if the [`Signal`] collection is already mutably borrowed or if the provided [`SignalId`] does not match any [`Signal`].
    pub fn set_signal_priority(&self, id: SignalId, priority: u64) -> Result<(), SignaledError> {
        let signals = self.signals
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))?;
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
    /// Returns [`SignaledError`] if the [`Signal`] collection is already mutably borrowed or if the provided [`SignalId`] does not match any [`Signal`].
    pub fn set_signal_once(&self, id: SignalId, is_once: bool) -> Result<(), SignaledError> {
        let signals = self.signals
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))?;
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
    /// Returns [`SignaledError`] if the [`Signal`] collection is already mutably borrowed or if the provided [`SignalId`] does not match any [`Signal`].
    pub fn set_signal_mute(&self, id: SignalId, is_mute: bool) -> Result<(), SignaledError> {
        let signals = self.signals
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_mute(is_mute);
            return Ok(())
        } else {
            return Err(signaled_error(ErrorType::InvalidSignalId { id: id }))
        }
    }

}

impl<T: Display> Display for Signaled<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = self.val.try_borrow()
            .map(|v| format!("{}", *v))
            .unwrap_or_else(|_| "<borrowed>".to_string());
        let mut signals_len = 0;
        let signals_string: String = self.signals.try_borrow().map(|s| {
            signals_len = s.len();
            s.iter()
                .map(|signal| format!("{}", signal))
                .collect::<Vec<_>>()
                .join(", ")
        }).unwrap_or_else(|_| "<borrowed>".to_string());
        write!(f, "Signaled {{ val: {}, signal_count: {}, signals: [{}] }}", value, signals_len, signals_string)
    }
}

impl<T: Clone> Clone for Signaled<T> {
    /// Clones the [`Signaled`] instance.
    /// 
    /// The contained value `val` is deep-cloned.
    /// 
    /// The list of signals is also cloned, but individual [`Signal`]s will initially share their underlying callback and trigger functions with the original. 
    /// 
    /// See [`Signal::clone`] for more details.
    /// 
    /// Because they also share the same `id`, only one clone of a particular signal can be added to a [`Signaled`] `signals` collection.
    /// However it is possible to create 2 identical [`Signal`]s by repeating the same creating process with [`Signal::new`] or the [`signal!`] macro.
    ///
    /// # Panics
    ///
    /// Panics if `val` or `signals` are borrowed at the time of cloning.
    /// Use `try_clone` for a non-panicking alternative.
    fn clone(&self) -> Self {
        Signaled {
            val: self.val.clone(),
            signals: self.signals.clone()
        }
    }
}

impl<T: Clone> Signaled<T> {
    /// Returns a cloned copy of the current value or an error if `val` is currently mutably borrowed.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError`] if `val` is already mutably borrowed.
    pub fn get(&self) -> Result<T, SignaledError> {
        match self.val.try_borrow() {
            Ok(r) => Ok(r.clone()),
            Err(_) => Err(signaled_error(ErrorType::BorrowError { source: ErrorSource::Value }))
        }
    }

    /// Creates a copy of the [`Signaled`], cloning `val` and `signals`.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError`] if `val` or `signals` are already borrowed.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{Signaled, SignaledError, ErrorType, ErrorSource};
    ///
    /// let s1 = Signaled::new(0);
    /// let s2 = s1.try_clone().unwrap();
    /// s1.set(1).unwrap();
    /// assert_eq!(s2.get().unwrap(), 0); // Independent copy
    /// ```
    pub fn try_clone(&self) -> Result<Self, SignaledError> {
        let signals_clone = self.signals
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))?
            .clone();
        let val_clone = self.val
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Value }))?
            .clone();
        Ok(
            Self {
                val: RefCell::new(val_clone),
                signals: RefCell::new(signals_clone)
            }
        )
    }
}

////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal() {
        let calls = Rc::new(Cell::new(0));
        let calls_clone = Rc::clone(&calls);

        let signaled = Signaled::new(1);
        signaled.add_signal(signal!(move |_, _| { calls_clone.set(calls_clone.get() + 1) })).unwrap();

        signaled.set(2).unwrap(); // Calls = 1
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn test_signal_trigger() {
        let calls = Rc::new(Cell::new(0));
        let calls_clone = Rc::clone(&calls);

        let signal: Signal<i32> = signal!(move |_, _| { calls_clone.set(calls_clone.get() + 1) });
        signal.set_trigger(|_, new| *new > 5).unwrap(); // Signal will only invoke callback if the `new_value` is greater than 5.

        let signaled = Signaled::new(1);
        signaled.add_signal(signal).unwrap();

        signaled.set(4).unwrap(); // This does not meet the trigger condition (new_value < 5). Calls = 0
        assert_eq!(calls.get(), 0);
        
        signaled.set(6).unwrap(); // This does meet the trigger condition (new_value > 5). Calls = 1
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn test_remove_signal() {
        let calls = Rc::new(Cell::new(0));
        let calls_clone = Rc::clone(&calls);

        let signaled = Signaled::new(1);
        let signal_id = signaled.add_signal(signal!(move |_, _| { calls_clone.set(calls_clone.get() + 1) })).unwrap();
        
        signaled.set(2).unwrap(); // Calls = 1
        assert_eq!(calls.get(), 1);

        signaled.set(3).unwrap(); // Calls = 2
        assert_eq!(calls.get(), 2);

        signaled.remove_signal(signal_id).unwrap(); // Signal removed so `calls` will not increase when `signaled` emits.

        signaled.set(4).unwrap();// Calls = 2
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn test_remove_trigger() {
        let calls = Rc::new(Cell::new(0));
        let calls_clone = Rc::clone(&calls);

        let signal: Signal<i32> = signal!(move |_, _| { calls_clone.set(calls_clone.get() + 1) });
        signal.set_trigger(|_, new| *new > 5).unwrap(); // Signal will only invoke callback if the `new_value` is greater than 5.

        let signaled = Signaled::new(1);
        let signal_id = signaled.add_signal(signal).unwrap();

        signaled.set(4).unwrap(); // This does not meet the trigger condition (new_value < 5). Calls = 0
        assert_eq!(calls.get(), 0);

        signaled.set(6).unwrap(); // This does meet the trigger condition (new_value > 5). Calls = 1
        assert_eq!(calls.get(), 1);

        signaled.remove_signal_trigger(signal_id).unwrap(); // Trigger condition removed so `new_value` will always invoke callback.

        signaled.set(4).unwrap(); // Calls = 2
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn test_change_signal() {
        let calls = Rc::new(Cell::new(0));
        let calls_clone = Rc::clone(&calls);

        let signaled = Signaled::new(1);
        let signal_id = signaled.add_signal(signal!(move |_, _| { calls_clone.set(calls_clone.get() + 1) })).unwrap();
        
        signaled.set(2).unwrap(); // Calls = 1
        assert_eq!(calls.get(), 1);

        signaled.set(3).unwrap(); // Calls = 2
        assert_eq!(calls.get(), 2);

        let calls_clone = Rc::clone(&calls);
        signaled.set_signal_callback(signal_id,  move |_, _| { calls_clone.set(calls_clone.get() + 2); }).unwrap(); // New callback increases call count by 2.

        signaled.set(4).unwrap();// Calls = 4
        assert_eq!(calls.get(), 4);
    }

    #[test]
    fn test_change_trigger() {
        let calls = Rc::new(Cell::new(0));
        let calls_clone = Rc::clone(&calls);

        let signal: Signal<i32> = signal!(move |_, _| { calls_clone.set(calls_clone.get() + 1) });
        signal.set_trigger(|_, new| *new > 5).unwrap(); // Signal will only invoke callback if the `new_value` is greater than 5.

        let signaled = Signaled::new(1);
        let signal_id = signaled.add_signal(signal).unwrap();

        signaled.set(4).unwrap(); // This does not meet the trigger condition (new_value < 5). Calls = 0
        assert_eq!(calls.get(), 0);

        signaled.set(6).unwrap(); // This does meet the trigger condition (new_value > 5). Calls = 1
        assert_eq!(calls.get(), 1);

        signaled.set_signal_trigger(signal_id, |_, new| { *new < 5 }).unwrap(); // Trigger condition changed so `new_value` being < 5 will invoke callback.

        signaled.set(6).unwrap(); // This does not meet the new trigger condition (new_value > 5). Calls = 1
        assert_eq!(calls.get(), 1);

        signaled.set(4).unwrap(); // This does meet the new trigger condition (new value < 5). Calls = 2
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn test_priority() {
        let signaled = Signaled::new(0);

        let test_value = Rc::new(Cell::new(' '));
        
        let test_value_clone = Rc::clone(&test_value);
        let signal_a: Signal<i32> = signal!(move |_, _| { test_value_clone.set('a'); });
        signal_a.set_priority(3);

        let test_value_clone = Rc::clone(&test_value);
        let signal_b: Signal<i32> = signal!(move |_, _| { assert_eq!(test_value_clone.get(), 'a'); }); // Signal in the middle with a callback that checks that `signal_a` callback has been invoked first.
        signal_b.set_priority(2);

        let test_value_clone = Rc::clone(&test_value);
        let signal_c: Signal<i32> = signal!(move |_, _| { test_value_clone.set('c'); });
        signal_c.set_priority(1);

        let signal_a_id = signaled.add_signal(signal_a).unwrap();
        let signal_b_id = signaled.add_signal(signal_b).unwrap();
        let signal_c_id = signaled.add_signal(signal_c).unwrap();

        signaled.set(0).unwrap();
        assert_eq!(test_value.get(), 'c'); // `signal_c` has the lowest priority (1) so after all callbacks are invoked the value will be 'c'.

        signaled.set_signal_priority(signal_a_id, 1).unwrap(); // Set priority to 1, `signal_a` will now be the last to execute.

        let test_value_clone = Rc::clone(&test_value);
        signaled.set_signal_callback(signal_b_id, move |_, _| { assert_eq!(test_value_clone.get(), 'c') }).unwrap(); // Modify signal in the middle callback to check that `signal_c` callback has been invoked first.

        signaled.set_signal_priority(signal_c_id, 3).unwrap(); // Set priority to 3, `signal_c` will now be the first to execute.

        signaled.set(0).unwrap();
        assert_eq!(test_value.get(), 'a'); // `signal_a` has now the lowest priority (1) so after all callbacks are invoked the valuew ill be 'a'.
    }

    #[test]
    fn test_set_borrow_error() {
        let signaled = Signaled::new(0);
        let _borrow = signaled.get_ref().unwrap();
        assert!(signaled.set(1).is_err_and(|e| e.message == "Cannot mutably borrow Signaled value, it is already borrowed"));
    }

    #[test]
    fn test_get_ref_borrow_error() {
        let signaled = Signaled::new(0);
        let _borrow = signaled.val.borrow_mut();
        assert!(signaled.get_ref().is_err_and(|e| e.message == "Cannot borrow Signaled value, it is already mutably borrowed"));
    }

    #[test]
    fn test_get_borrow_error() {
        let signaled = Signaled::new(0);
        let _borrow = signaled.val.borrow_mut();
        assert!(signaled.get().is_err_and(|e| e.message == "Cannot borrow Signaled value, it is already mutably borrowed"));
    }

    #[test]
    fn test_try_clone_borrow_error() {
        {
            let signaled = Signaled::new(0);
            let _borrow = signaled.val.borrow_mut();
            assert!(signaled.try_clone().is_err_and(|e| e.message == "Cannot borrow Signaled value, it is already mutably borrowed"));
        }
        {
            let signaled = Signaled::new(0);
            let _borrow = signaled.signals.borrow_mut();
            assert!(signaled.try_clone().is_err_and(|e| e.message == "Cannot borrow Signaled signals, they are already mutably borrowed"));
        }
    }

    #[test]
    #[should_panic]
    fn test_clone_borrow_panic() {
        let signaled_a = Signaled::new(0);
        let _borrow = signaled_a.val.borrow_mut();
        let _signaled_b = signaled_a.clone();
    }

    #[test]
    fn test_emit_signals_borrow_error() {
        let signaled = Signaled::new(0);
        signaled.add_signal(signal!(|_, _|  {})).unwrap();
        let _borrow = signaled.signals.borrow();
        assert!(signaled.emit_signals(&1, &2).is_err_and(|e| e.message == "Cannot mutably borrow Signaled signals, they are already borrowed"));
    }

    #[test]
    fn test_add_signal_borrow_error() {
        let signaled = Signaled::new(0);
        let _borrow = signaled.signals.borrow();
        assert!(signaled.add_signal(signal!(|_, _| {})).is_err_and(|e| e.message == "Cannot mutably borrow Signaled signals, they are already borrowed"));
    }

    #[test]
    fn test_remove_signal_borrow_error() {
        let signaled = Signaled::new(0);
        let signal_id = signaled.add_signal(signal!(|_, _| {})).unwrap();
        let _borrow = signaled.signals.borrow();
        assert!(signaled.remove_signal(signal_id).is_err_and(|e| e.message == "Cannot mutably borrow Signaled signals, they are already borrowed"));
    }

    #[test]
    fn test_set_signal_callback_borrow_error() {
        let signaled = Signaled::new(0);
        let signal_id = signaled.add_signal(signal!(|_, _| {})).unwrap();
        let _borrow = signaled.signals.borrow_mut();
        assert!(signaled.set_signal_callback(signal_id, |_, _| {}).is_err_and(|e| e.message == "Cannot borrow Signaled signals, they are already mutably borrowed"));
    }

    #[test]
    fn test_set_signal_trigger_borrow_error() {
        let signaled = Signaled::new(0);
        let signal_id = signaled.add_signal(signal!(|_, _| {})).unwrap();
        let _borrow = signaled.signals.borrow_mut();
        assert!(signaled.set_signal_trigger(signal_id, |_, _| true).is_err_and(|e| e.message == "Cannot borrow Signaled signals, they are already mutably borrowed"));
    }

    #[test]
    fn test_remove_signal_trigger_borrow_error() {
        let signaled = Signaled::new(0);
        let signal_id = signaled.add_signal(signal!(|_, _| {})).unwrap();
        let _borrow = signaled.signals.borrow_mut();
        assert!(signaled.remove_signal_trigger(signal_id).is_err_and(|e| e.message == "Cannot borrow Signaled signals, they are already mutably borrowed"));
    }

    #[test]
    fn test_set_signal_priority_borrow_error() {
        let signaled = Signaled::new(0);
        let signal_id = signaled.add_signal(signal!(|_, _| {})).unwrap();
        let _borrow = signaled.signals.borrow_mut();
        assert!(signaled.set_signal_priority(signal_id, 1).is_err_and(|e| e.message == "Cannot borrow Signaled signals, they are already mutably borrowed"));
    }

    #[test]
    fn test_set_signal_once_borrow_error() {
        let signaled = Signaled::new(0);
        let signal_id = signaled.add_signal(signal!(|_, _| {})).unwrap();
        let _borrow = signaled.signals.borrow_mut();
        assert!(signaled.set_signal_once(signal_id, true).is_err_and(|e| e.message == "Cannot borrow Signaled signals, they are already mutably borrowed"));
    }

    #[test]
    fn test_set_signal_mute_borrow_error() {
        let signaled = Signaled::new(0);
        let signal_id = signaled.add_signal(signal!(|_, _| {})).unwrap();
        let _borrow = signaled.signals.borrow_mut();
        assert!(signaled.set_signal_mute(signal_id, true).is_err_and(|e| e.message == "Cannot borrow Signaled signals, they are already mutably borrowed"));
    }

    #[test]
    fn test_emit_borrow_error() {
        {
            let signal: Signal<i32> = signal!(|_, _| {});
            let _borrow = signal.trigger.borrow_mut();
            assert!(signal.emit(&1, &2).is_err_and(|e| e.message == "Cannot borrow Signal trigger, it is already mutably borrowed"));
        }
        {
            let signal: Signal<i32> = signal!(|_, _| {});
            let _borrow = signal.callback.borrow_mut();
            assert!(signal.emit(&1, &2).is_err_and(|e| e.message == "Cannot borrow Signal callback, it is already mutably borrowed"));
        }
    }

    #[test] 
    fn test_set_callback_borrow_error() {
        let signal: Signal<i32> = signal!(|_, _| {});
        let _borrow = signal.callback.borrow();
        assert!(signal.set_callback(|_, _| {}).is_err_and(|e| e.message == "Cannot mutably borrow Signal callback, it is already borrowed"));
    }

    #[test]
    fn test_set_trigger_borrow_error() {
        let signal: Signal<i32> = signal!(|_, _| {});
        let _borrow = signal.trigger.borrow();
        assert!(signal.set_trigger(|_, _| true).is_err_and(|e| e.message == "Cannot mutably borrow Signal trigger, it is already borrowed"));
    }

    #[test]
    fn test_remove_trigger_borrow_error() {
        let signal: Signal<i32> = signal!(|_, _| {});
        let _borrow = signal.trigger.borrow();
        assert!(signal.remove_trigger().is_err_and(|e| e.message == "Cannot mutably borrow Signal trigger, it is already borrowed"));
    }

    fn create_signaled_with_invalid_id() -> (Signaled<i32>, SignalId) {
        let signaled = Signaled::new(0);
        let signal_id = signaled.add_signal(signal!(|_, _| {})).unwrap();
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
    test_invalid_id_error!(test_set_signal_callback_invalid_signal_id_error, set_signal_callback, |_, _| {});
    test_invalid_id_error!(test_set_signal_trigger_invalid_signal_id_error, set_signal_trigger, |_, _| true);
    test_invalid_id_error!(test_remove_signal_trigger_invalid_signal_id_error, remove_signal_trigger,);
    test_invalid_id_error!(test_set_signal_priority_invalid_signal_id_error, set_signal_priority, 1);
    test_invalid_id_error!(test_set_signal_once_invalid_signal_id_error, set_signal_once, true);
    test_invalid_id_error!(test_set_signal_mute_invalid_signal_id_error, set_signal_mute, true);

    #[test]
    fn test_once_signal() {
        let signaled = Signaled::new(5);
        let signal_a: Signal<i32> = signal!(|_, _| {});
        let signal_b: Signal<i32> = signal!(|_, _| {});
        signal_b.set_once(true);
        
        signaled.add_signal(signal_a).unwrap();
        signaled.add_signal(signal_b).unwrap();

        assert_eq!(signaled.signals.borrow().len(), 2); // Signal A and Signal B.
        signaled.set(6).unwrap(); // Signal B is dropped because it is `once`.
        assert_eq!(signaled.signals.borrow().len(), 1); // Signal A.
    }

    #[test]
    fn test_mute_signal() {
        let calls = Rc::new(Cell::new(0));
        let signaled = Signaled::new(0);

        let calls_clone = Rc::clone(&calls);
        let signal: Signal<i32> = signal!(move |_, _| calls_clone.set(calls_clone.get() + 1));

        let signal_id = signaled.add_signal(signal).unwrap();

        signaled.set(6).unwrap(); // Calls = 1, Signal is unmuted.
        assert_eq!(calls.get(), 1);

        signaled.set_signal_mute(signal_id, true).unwrap(); // Signal muted so next Signaled `set` won't invoke its callback.

        signaled.set(7).unwrap(); // Calls = 1, Signal is muted.
        assert_eq!(calls.get(), 1);

        signaled.set_signal_mute(signal_id, false).unwrap(); // Signal unmuted so next Signaled `set` will invoke its callback.

        signaled.set(8).unwrap(); // Calls = 2, Signal is unmuted.
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn test_get_ref() {
        let signaled = Signaled::new(0);
        assert_eq!(*signaled.get_ref().unwrap(), 0);

        for i in 0..10 {
            signaled.set(i).unwrap();
            assert_eq!(*signaled.get_ref().unwrap(), i);
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
        let signaled = Signaled::new(0);
        signaled.add_signal(signal!(|old, new| assert!(*new == *old + 1))).unwrap();
        
        for i in 1..10 {
            signaled.set(i).unwrap();
        }
    }
}