#![allow(dead_code)]
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;
use std::fmt::Display;
use std::sync::atomic::{AtomicU64, Ordering};

////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

/// Source of an error in the `Signaled` or `Signal` structs.
#[derive(Debug)]
pub enum ErrorSource {
    /// Error related to the `value` field of `Signaled`.
    Value,
    /// Error related to the `signals` vector of `Signaled`.
    Signals,
    /// Error related to a `Signal`'s callback.
    SignalCallback,
    /// Error related to a `Signal`'s trigger.
    SignalTrigger
}

/// Type of error when manipulating `Signaled` or `Signal` structs. 
#[derive(Debug)] 
pub enum ErrorType {
    /// Attempted to immutably borrow a `RefCell` wrapped field that is already borrowed.
    BorrowError { source: ErrorSource },
    /// Attempted to mutably borrow a `RefCell` wrapped field that is already borrowed.
    BorrowMutError { source: ErrorSource },
    /// Provided a `SignalId` that does not match any `Signal`.
    InvalidSignalId { id: SignalId }
}

/// Error type returned by `Signaled` and `Signal` functions.
/// 
/// Contains a descriptive message indicating the source and type of the error, such as borrow conflicts or invalid signal IDs.
#[derive(Debug)]
pub struct SignaledError {
    /// Descriptive error message.
    message: String
}

/// Constructs a `SignaledError` from an `ErrorType`.
/// 
/// # Arguments
/// 
/// * `err_type` - `ErrorType` that represents the type of the error and contains a field representing the source of the error.
pub fn signaled_error(err_type: ErrorType) -> SignaledError {
    let err_msg = match err_type {
        ErrorType::BorrowError { source } => {
            match source {
                ErrorSource::SignalCallback => "Cannot borrow Signal callback, it is already borrowed".to_string(),
                ErrorSource::SignalTrigger => "Cannot borrow Signal trigger, it is already borrowed".to_string(),
                ErrorSource::Value => "Cannot borrow Signaled value, it is already borrowed".to_string(),
                ErrorSource::Signals => "Cannot borrow Signaled signals, they are already borrowed".to_string()
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

/// Generates a new `SignalId` for a `Signal` using the global ID counter
/// 
/// When a `Signal` is `cloned` the ID is reutilized for easier identification since `callback` and `trigger` cannot be visualized.
/// 
/// # Panics 
/// 
/// Panics in debug builds if the ID counter overflows (`u64::MAX`).
pub fn new_signal_id() -> SignalId {
    let id = SIGNAL_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    debug_assert!(id != u64::MAX, "SignalId counter overflow");
    id
}
/// A signal that executes a callback when a `Signaled` value changes, if its trigger condition is met.
///
/// Signals have a unique ID, a priority for ordering execution, and a trigger function to
/// conditionally execute the callback.
///
/// # Examples
///
/// ```
/// use signaled::{Signal, Signaled};
///
/// let signal = Signal::new(|v: &i32| println!("Value: {}", v));
/// let signaled = Signaled::new(0);
/// signaled.add_signal(signal).unwrap();
/// signaled.set(42).unwrap(); // Prints "Value: 42"
/// ```
pub struct Signal<T> {
    /// Function to run when the signal is emitted and the trigger condition is met.
    callback: RefCell<Rc<dyn Fn(&T)>>,
    /// Function that decides if the callback will be invoked or not when the signal is emitted.
    trigger: RefCell<Rc<dyn Fn(&T) -> bool>>,
    /// Identifier for the signal.
    id: u64,
    /// Number used in the Signaled struct to decide the order of execution of the signals.
    priority: Cell<u64>
}

impl<T> Signal<T> {
    /// Creates a new `Signal` with the given callback, default trigger (always `true`), and priority 1.
    ///
    /// # Arguments
    ///
    /// * `callback` - The function to execute when the signal is emitted.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::Signal;
    ///
    /// let signal = Signal::new(|v: &i32| println!("Value: {}", v));
    /// let signaled = Signaled::new(5);
    /// signaled.add_signal(signal).unwrap();
    /// signaled.set(6).unwrap(); // Prints "Value: 6"
    /// ```
    pub fn new<F: Fn(&T) + 'static>(callback: F) -> Self {
        Signal {
            callback: RefCell::new(Rc::new(callback)),
            trigger: RefCell::new(Rc::new(|_| true)),
            id: new_signal_id(),
            priority: Cell::new(1)
        }
    }

    /// Emits the signal, executing the callback if the trigger condition is met.
    ///
    /// # Errors
    ///
    /// Returns `SignaledError` if the callback or trigger is already borrowed.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{Signal};
    ///
    /// let signal = Signal::new(|v: &i32| println!("Value: {}", v));
    /// signal.set_trigger(|v| *v > 5).unwrap();
    /// signal.emit(&4).unwrap() // Doesn't print because trigger condition is not met.
    /// signal.emit(&6).unwrap() // Prints "Value: 6"
    /// ```
    pub fn emit(&self, value: &T) -> Result<(), SignaledError>{
        let trigger = self.trigger
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::SignalTrigger }))?;
        if trigger(value) {
            let callback = self.callback
                .try_borrow()
                .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::SignalCallback }))?;
            callback(value);
        }
        Ok(())
    }

    /// Sets a new callback for the signal.
    /// 
    /// # Arguments
    /// 
    /// * `callback` - The new callback function.
    /// 
    /// # Errors
    /// 
    /// Returns `SignaledError` if the callback is already borrowed.
    pub fn set_callback<F: Fn(&T) + 'static>(&self, callback: F) -> Result<(), SignaledError> {
        *self.callback.try_borrow_mut().map_err(|_| signaled_error(ErrorType::BorrowMutError { source: ErrorSource::SignalCallback }))? = Rc::new(callback);
        Ok(())
    }

    /// Sets a new trigger for the signal.
    /// 
    /// Default trigger always returns `true`.
    /// 
    /// # Arguments
    /// 
    /// * `trigger` - The new trigger function.
    /// 
    /// # Errors
    /// 
    /// Returns `SignaledError` if the trigger is already borrowed.
    pub fn set_trigger<F: Fn(&T) -> bool + 'static>(&self, trigger: F) -> Result<(), SignaledError> {
        *self.trigger.try_borrow_mut().map_err(|_| signaled_error(ErrorType::BorrowMutError { source: ErrorSource::SignalTrigger }))? = Rc::new(trigger);
        Ok(())
    }

    /// Sets the execution priority of the signal.
    /// 
    /// Default priority is 1.
    /// 
    /// # Arguments
    /// 
    /// * `priority` The new priority number, bigger number means higher priority.
    pub fn set_priority(&self, priority: u64) {
        self.priority.set(priority);
    }
}

impl<T> Display for Signal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Signal {{ id: {}, priority: {} }}", self.id, self.priority.get())
    }
}

impl<T> Clone for Signal<T> {
    /// Creates a clone of the signal, retaining the same ID.
    /// 
    /// # Panics
    /// 
    /// Panics if the callback or trigger is already borrowed.
    /// 
    /// # Notes
    /// 
    /// Cloned signals share the same callback and trigger, (`Rc` pointers).
    /// Modifying them through `set_callback` or `set_trigger` will only affect the targeted Signal since those methods drop the `Rc` wrapped inside the `RefCell` and create a new one
    fn clone(&self) -> Self {
        Signal {
            callback: self.callback.clone(),
            trigger: self.trigger.clone(),
            id: self.id.clone(),
            priority: self.priority.clone()
        }
    }
}

/// A reactive container that holds a value and emits signals when the value changes.
///
/// `Signaled<T>` manages a value of type `T` and a collection of `Signal<T>` instances.
/// When the value is updated via `set`, all signals are emitted in order of descending priority,
/// provided their trigger conditions are met.
///
/// # Examples
///
/// ```
/// use signaled::{Signaled, Signal, SignaledError};
///
/// let signaled = Signaled::new(0);
/// signaled.add_signal(Signal::new(|v| println!("Value: {}", v))).unwrap();
/// signaled.set(42).unwrap(); // Prints "Value: 42"
/// ```
pub struct Signaled<T> {
    /// Reactive value, the mutation of this value through `set` will emit all `Signal<T>` inside `signals`.
    value: RefCell<T>,
    /// Collection of signals that will be emitted when `value` is changed through `set`.
    signals: RefCell<Vec<Signal<T>>>
}

impl<T> Signaled<T> {
    /// Creates a new instance of `Signaled` with the given initial value.
    ///
    /// # Arguments
    ///
    /// * `value` - The initial value.
    pub fn new(value: T) -> Self {
        Self {
            value: RefCell::new(value),
            signals: RefCell::new(Vec::new())
        }
    }

    /// Sets a new value and emits all signals.
    ///
    /// # Arguments
    ///
    /// * `value` - The new value to set.
    ///
    /// # Errors
    ///
    /// Returns `SignaledError` if the value or signals are already borrowed.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{Signaled};
    ///
    /// let signaled = Signaled::new(0);
    /// signaled.add_signal(Signal::new(|v| println!("Value: {}", v)));
    /// signaled.set(1); /// Prints "Value: 1"
    /// ```
    pub fn set(&self, value: T) -> Result<(), SignaledError> {
        *self.value.try_borrow_mut().map_err(|_| signaled_error(ErrorType::BorrowMutError { source: ErrorSource::Value }))? = value;
        self.emit()
    }

    /// Returns a reference to the current value or an error if the value is currently borrowed.
    /// 
    /// # Errors
    /// 
    /// Returns `SignaledError` if the value is already borrowed.
    pub fn get_ref(&self) -> Result<Ref<'_, T>, SignaledError> {
        match self.value.try_borrow() {
            Ok(r) => Ok(r),
            Err(_) => Err(signaled_error(ErrorType::BorrowError { source: ErrorSource::Value })) 
        }
    }

    /// Emits all signals in descending priority order, invoking their callbacks if their trigger condition is met.
    /// 
    /// # Errors
    /// 
    /// Returns `SignaledError` if the value or any of the signals are already borrowed.
    pub fn emit(&self) -> Result<(), SignaledError> {
        match self.signals.try_borrow_mut() {
            Ok(mut signals) => {
                signals.sort_by(|a, b| b.priority.get().cmp(&a.priority.get()));
                for signal in signals.iter() {
                    let value = self.value
                        .try_borrow()
                        .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Value }))?;
                    signal.emit(&value)?
                }
                Ok(())
            }
            Err(_) => Err(signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))
        }
    }

    /// Adds a signal to the collection, returning its SignalId.
    /// 
    /// If a signal with the same ID is already in the collection, returns the existing ID without adding the signal to the collection.
    /// 
    /// # Arguments
    /// * `signal` - The signal to add.
    /// 
    /// # Errors
    /// 
    /// Returns `SignaledError` if the signals collection is already borrowed.
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

    /// Removes a signal by ID, returning the removed signal.
    ///
    /// # Arguments
    ///
    /// * `id` - The ID of the signal to remove.
    ///
    /// # Errors
    ///
    /// Returns `SignaledError` if the signals collection is already borrowed or the ID is invalid.
    pub fn remove_signal(&self, id: SignalId) -> Result<Signal<T>, SignaledError>{
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
 
    /// Sets the callback for a signal by ID.
    ///
    /// # Arguments
    ///
    /// * `id` - The ID of the signal.
    /// * `callback` - The new callback function.
    ///
    /// # Errors
    ///
    /// Returns `SignaledError` if the signals collection or signal callback is already borrowed.
    ///
    /// # Notes
    ///
    /// If the ID does not exist, the operation is a no-op and returns `Ok(())`.
    pub fn set_signal_callback<F: Fn(&T) + 'static>(&self, id: SignalId, callback: F) -> Result<(), SignaledError> {
        let signals = self.signals
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.set_callback(callback)
        }
        Ok(())
    }

    /// Sets the trigger condition for a signal by ID.
    ///
    /// # Arguments
    ///
    /// * `id` - The ID of the signal.
    /// * `trigger` - The new trigger function.
    ///
    /// # Errors
    ///
    /// Returns `SignaledError` if the signals collection or signal trigger is already borrowed.
    ///
    /// # Notes
    ///
    /// If the ID does not exist, the operation is a no-op and returns `Ok(())`.
    pub fn set_signal_trigger<F: Fn(&T) -> bool + 'static>(&self, id: SignalId, trigger: F) -> Result<(), SignaledError> {
        let signals = self.signals.try_borrow().map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.set_trigger(trigger)
        }
        Ok(())
    }

    /// Sets the priority for a signal by ID.
    ///
    /// # Arguments
    ///
    /// * `id` - The ID of the signal.
    /// * `priority` - The new priority value (higher executes first).
    ///
    /// # Errors
    ///
    /// Returns `SignaledError` if the signals collection is already borrowed.
    ///
    /// # Notes
    ///
    /// If the ID does not exist, the operation is a no-op and returns `Ok(())`.
    pub fn set_signal_priority(&self, id: SignalId, priority: u64) -> Result<(), SignaledError> {
        let signals = self.signals
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_priority(priority);
        }
        Ok(())
    }

}

impl<T: Display> Display for Signaled<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let signals = self.signals.borrow();
        let value = self.value.borrow();
        let signals_len = signals.len();
        let signals: String = signals
            .iter()
            .map(|s| format!("{}", s))
            .collect::<Vec<_>>()
            .join(", ");
        write!(f, "Signaled {{ value: {}, signal_count: {}, signals: [{}] }}", value, signals_len, signals)
    }
}

impl<T: Clone> Clone for Signaled<T> {
    /// Creates a deep copy of the `Signaled`, cloning the value and signals.
    ///
    /// # Panics
    ///
    /// Panics if the value or signals are borrowed at the time of cloning.
    /// Use `try_clone` for a non-panicking alternative.
    fn clone(&self) -> Self {
        Signaled {
            value: self.value.clone(),
            signals: self.signals.clone()
        }
    }
}

impl<T: Clone> Signaled<T> {
    /// Returns a cloned copy of the current value.
    ///
    /// # Errors
    ///
    /// Returns `SignaledError` if the value is already borrowed.
    pub fn get(&self) -> Result<T, SignaledError> {
        match self.value.try_borrow() {
            Ok(r) => Ok(r.clone()),
            Err(_) => Err(signaled_error(ErrorType::BorrowError { source: ErrorSource::Value }))
        }
    }

    /// Creates a deep copy of the `Signaled`, cloning the value and signals.
    ///
    /// # Errors
    ///
    /// Returns `SignaledError` if the value or signals are already borrowed.
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
        let value_clone = self.value
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Value }))?
            .clone();
        Ok(
            Self {
                value: RefCell::new(value_clone),
                signals: RefCell::new(signals_clone)
            }
        )
    }
}

////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
fn main() {
}

#[cfg(test)]
mod tests {
    use super::*;
}