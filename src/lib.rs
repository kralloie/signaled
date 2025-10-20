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
//! use signaled::{Signaled, SignaledError, ErrorSource};
//!
//! let signaled = Signaled::new(0);
//! let _borrow = signaled.get_ref().unwrap();
//! let result = signaled.set(1);
//! assert!(matches!(
//!     result,
//!     Err(SignaledError::BorrowMutError { source }) if source == ErrorSource::Value
//! ));
//! ```

#![allow(dead_code)]
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;
use std::fmt::{Display};
use std::sync::atomic::{AtomicU64, Ordering};

pub mod sync;

////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum ErrorSource {
    Value,
    Signals,
    SignalCallback,
    SignalTrigger
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum SignaledError {
    BorrowError { source: ErrorSource },
    BorrowMutError { source: ErrorSource },
    InvalidSignalId { id: SignalId }
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
            .map_err(|_| SignaledError::BorrowError { source: ErrorSource::SignalTrigger })?;
        if trigger(old, new) {
            let callback = self.callback
                .try_borrow()
                .map_err(|_| SignaledError::BorrowError { source: ErrorSource::SignalCallback })?;
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
        *self.callback.try_borrow_mut().map_err(|_| SignaledError::BorrowMutError { source: ErrorSource::SignalCallback })? = Rc::new(callback);
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
        *self.trigger.try_borrow_mut().map_err(|_| SignaledError::BorrowMutError { source: ErrorSource::SignalTrigger })? = Rc::new(trigger);
        Ok(())
    }

    /// Removes the [`Signal`] trigger condition.
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError`] if the trigger is already borrowed.
    pub fn remove_trigger(&self) -> Result<(), SignaledError> {
        *self.trigger.try_borrow_mut().map_err(|_| SignaledError::BorrowMutError { source: ErrorSource::SignalTrigger })? = Rc::new(|_, _| true);
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
                    .map_err(|_| SignaledError::BorrowMutError { source: ErrorSource::Value })?,
            new_value
        );
        let new_value_ref = self.val.try_borrow().map_err(|_| SignaledError::BorrowError { source: ErrorSource::Value })?;
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
            Err(_) => Err(SignaledError::BorrowError { source: ErrorSource::Value })
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
            Err(_) => Err(SignaledError::BorrowMutError { source: ErrorSource::Signals })
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
            Err(_) => Err(SignaledError::BorrowMutError { source: ErrorSource::Signals })
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
                    .ok_or_else(|| SignaledError::InvalidSignalId { id: id })?;
                Ok(s.remove(index))
            }
            Err(_) => Err(SignaledError::BorrowMutError { source: ErrorSource::Signals })
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
            .map_err(|_| SignaledError::BorrowError { source: ErrorSource::Signals })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.set_callback(callback)
        } else {
            return Err(SignaledError::InvalidSignalId { id: id })
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
        let signals = self.signals.try_borrow().map_err(|_| SignaledError::BorrowError { source: ErrorSource::Signals })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.set_trigger(trigger)
        } else {
            return Err(SignaledError::InvalidSignalId { id: id })
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
        let signals = self.signals.try_borrow().map_err(|_| SignaledError::BorrowError { source: ErrorSource::Signals })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.remove_trigger()
        } else {
            return Err(SignaledError::InvalidSignalId { id: id })
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
            .map_err(|_| SignaledError::BorrowError { source: ErrorSource::Signals })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_priority(priority);
            return Ok(())
        } else {
            return Err(SignaledError::InvalidSignalId { id: id })
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
            .map_err(|_| SignaledError::BorrowError { source: ErrorSource::Signals })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_once(is_once);
            return Ok(())
        } else {
            return Err(SignaledError::InvalidSignalId { id: id })
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
            .map_err(|_| SignaledError::BorrowError { source: ErrorSource::Signals })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_mute(is_mute);
            return Ok(())
        } else {
            return Err(SignaledError::InvalidSignalId { id: id })
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

impl<T: Clone> Signaled<T> {
    /// Returns a cloned copy of the current value or an error if `val` is currently mutably borrowed.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError`] if `val` is already mutably borrowed.
    pub fn get(&self) -> Result<T, SignaledError> {
        match self.val.try_borrow() {
            Ok(r) => Ok(r.clone()),
            Err(_) => Err(SignaledError::BorrowError { source: ErrorSource::Value })
        }
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

/////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

    macro_rules! test_signaled_borrow_error {
        ($test_name:ident, $borrow:ident, $borrow_type:ident, $method:ident $(, $args:expr)*; $error:expr) => {
            #[test]
            fn $test_name() {
                let signaled = Signaled::new(0);
                let _borrow = signaled.$borrow.$borrow_type();
                let err: SignaledError = $error;
                assert!(signaled.$method($($args),*).is_err_and(|e| e == err));
            }
        };
        ($test_name:ident, $borrow:ident, $borrow_type:ident, $method:ident $(, $args:expr)*; $error:expr, $use_id:tt) => {
            #[test]
            fn $test_name() {
                let signaled = Signaled::new(0);
                let signal_id = signaled.add_signal(signal!(|_, _| {})).unwrap();
                let _borrow = signaled.$borrow.$borrow_type();
                let err: SignaledError = $error;
                assert!(signaled.$method(signal_id $(, $args),*).is_err_and(|e| e == err));
            }
        };
    }

    test_signaled_borrow_error!(test_set_borrow_error, val, borrow, set, 1; SignaledError::BorrowMutError { source: ErrorSource::Value });
    test_signaled_borrow_error!(test_get_ref_borrow_error, val, borrow_mut, get_ref; SignaledError::BorrowError { source: ErrorSource::Value });
    test_signaled_borrow_error!(test_get_borrow_error, val, borrow_mut, get; SignaledError::BorrowError { source: ErrorSource::Value });
    test_signaled_borrow_error!(test_emit_signals_borrow_error, signals, borrow, emit_signals, &1, &2; SignaledError::BorrowMutError { source: ErrorSource::Signals });
    test_signaled_borrow_error!(test_add_signal_borrow_error, signals, borrow, add_signal, signal!(|_, _| {}); SignaledError::BorrowMutError { source: ErrorSource::Signals });
    test_signaled_borrow_error!(test_remove_signal_borrow_error, signals, borrow, remove_signal; SignaledError::BorrowMutError { source: ErrorSource::Signals }, true);
    test_signaled_borrow_error!(test_set_signal_callback_borrow_error, signals, borrow_mut, set_signal_callback, |_, _| {}; SignaledError::BorrowError { source: ErrorSource::Signals }, true);
    test_signaled_borrow_error!(test_set_signal_trigger_borrow_error, signals, borrow_mut, set_signal_trigger, |_, _| true; SignaledError::BorrowError { source: ErrorSource::Signals }, true);
    test_signaled_borrow_error!(test_remove_signal_trigger_borrow_error, signals, borrow_mut, remove_signal_trigger; SignaledError::BorrowError { source: ErrorSource::Signals }, true);
    test_signaled_borrow_error!(test_set_signal_priority_borrow_error, signals, borrow_mut, set_signal_priority, 1; SignaledError::BorrowError { source: ErrorSource::Signals }, true);
    test_signaled_borrow_error!(test_set_signal_once_borrow_error, signals, borrow_mut, set_signal_once, true; SignaledError::BorrowError { source: ErrorSource::Signals }, true);
    test_signaled_borrow_error!(test_set_signal_mute_borrow_error, signals, borrow_mut, set_signal_mute, true; SignaledError::BorrowError { source: ErrorSource::Signals }, true);

    macro_rules! test_signal_borrow_error {
        ($test_name:ident, $borrow:ident, $borrow_type:ident, $method:ident $(, $args:expr)*; $error:expr) => {
            #[test]
            fn $test_name() {
                let signal: Signal<i32> = signal!(|_, _| {});
                let _borrow = signal.$borrow.$borrow_type();
                let err = $error;
                assert!(signal.$method($($args),*).is_err_and(|e| e == err));
            }
        };
    }

    test_signal_borrow_error!(test_emit_trigger_borrow_error, trigger, borrow_mut, emit, &1, &2; SignaledError::BorrowError { source: ErrorSource::SignalTrigger });
    test_signal_borrow_error!(test_emit_callback_borrow_error, callback, borrow_mut, emit, &1, &2; SignaledError::BorrowError { source: ErrorSource::SignalCallback });
    test_signal_borrow_error!(test_set_callback_borrow_error, callback, borrow, set_callback, |_, _| {}; SignaledError::BorrowMutError { source: ErrorSource::SignalCallback });
    test_signal_borrow_error!(test_set_trigger_borrow_error, trigger, borrow, set_trigger, |_, _| true; SignaledError::BorrowMutError { source: ErrorSource::SignalTrigger });
    test_signal_borrow_error!(test_remove_trigger_borrow_error, trigger, borrow, remove_trigger; SignaledError::BorrowMutError { source: ErrorSource::SignalTrigger });

/////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

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
                    Err(SignaledError::InvalidSignalId { id }) if id == invalid_id)
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

/////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

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