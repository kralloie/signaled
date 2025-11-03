#![doc = include_str!("../README.md")]
use std::cell::{Cell, Ref, RefCell};
use std::fmt::{Debug, Display};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub mod sync;

////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum ErrorSource {
    /// Error coming from [`Signaled::val`].
    Value,
    /// Error coming from [`Signaled::signals`].
    Signals,
    /// Error coming from [`Signal::callback`].
    SignalCallback,
    /// Error coming from [`Signal::trigger`].
    SignalTrigger,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum SignaledError {
    /// Attempted to borrow a value that was already mutably borrowed.
    BorrowError { source: ErrorSource },
    /// Attempted to mutably borrow a value that was already borrowed.
    BorrowMutError { source: ErrorSource },
    /// Attempted to target a [`Signal`] with a [`SignalId`] that does not match any [`Signal`].
    InvalidSignalId { id: SignalId },
}

pub type SignalId = u64;

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

/// [`Signal`] callback signature.
pub type Callback<T> = dyn Fn(&T, &T) + 'static;
/// [`Signal`] callback container.
pub type SignalCallback<T> = RefCell<Rc<Callback<T>>>;
/// [`Signal`] trigger signature.
pub type Trigger<T> = dyn Fn(&T, &T) -> bool + 'static;
/// [`Signal`] trigger container.
pub type SignalTrigger<T> = RefCell<Rc<Trigger<T>>>;

/// A signal that executes a its [`Signal::callback`] when a [`Signaled::val`] changes through [`Signaled::set()`] or similar methods, if its [`Signal::trigger`] condition is met.
///
/// ## *Signal* has the following properties:
/// 
/// - [`Signal::callback`]: [`SignalCallback`] that is invoked when the [`Signal`] is emitted.
/// 
/// - [`Signal::trigger`]: [`SignalTrigger`] that returns a [`bool`] representing if the [`Signal::callback`] will be invoked or not.
/// 
/// - [`Signal::id`]: Unique [`SignalId`] that serves as identifier for the [`Signal`].
/// 
/// - [`Signal::priority`]: [`Cell<u64>`] that the parent [`Signaled`] uses to decide the order of execution of the [`Signaled::signals`].
/// 
/// - [`Signal::once`]: [`Cell<bool>`] that if set to `true` will make the [`Signaled`] parent remove this [`Signal`] from [`Signaled::signals`] after the [`Signal::callback`] is called once.
/// 
/// - [`Signal::mute`]: [`Cell<bool>`] that if set to `true` will prevent this [`Signal`] from being emitted.
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
pub struct Signal<T: 'static> {
    /// Function to run when the [`Signal`] is emitted and the [`Signal::trigger`] condition is met.
    callback: SignalCallback<T>,
    /// Function that decides if the [`Signal::callback`] will be invoked or not when the [`Signal`] is emitted.
    trigger: SignalTrigger<T>,
    /// Identifier for the [`Signal`].
    pub id: u64,
    /// Number used in the [`Signaled`] struct to decide the order of execution of the signals.
    priority: Cell<u64>,
    /// Boolean representing if the [`Signal`] should be removed from the [`Signaled`] after its [`Signal::callback`] is successfully invoked once.
    /// The removal only occurs if the [`Signal::trigger`] condition is met during emission, resulting in the [`Signal::callback`] being invoked.
    once: Cell<bool>,
    /// Boolean representing if the [`Signal`] should not invoke the [`Signal::callback`] when emitted.
    mute: Cell<bool>,
}

/// A struct containing the information of each field of [`Signal`]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalInfo {
    /// Unique [`SignalId`] identifier of the [`Signal`].
    pub id: u64,
    /// [`Signal::priority`] of the [`Signal`] that decides in which order it will be executed.
    pub priority: u64,
    /// [`Signal::once`] Flag of the [`Signal`] that decides if it should be removed from the [`Signaled::signals`] collection after executed.
    pub once: bool,
    /// [`Signal::mute`] Flag of the [`Signal`] that decides if it should be emitted when the parent [`Signaled`] calls [`Signaled::emit_signals()`]
    pub mute: bool,
}

impl<T> Signal<T> {
    /// Creates a new [`Signal`] instance.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function to execute when the [`Signal`] is emitted.
    /// * `trigger` -  Function that returns a boolean representing if the [`Signal::callback`] will be invoked or not.
    /// * `priority` - Number representing the priority in which the [`Signal`] will be emitted from the parent [`Signaled`].
    /// * `once` - Boolean representing if the [`Signal`] will be removed from the parent [`Signaled::signals`] after being emitted once. The removal only occurs if the [`Signal::trigger`] condition is met during emission.
    /// * `mute` - Boolean representing if the [`Signal::callback`] will be invoked or not.
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
    pub fn new<F: Fn(&T, &T) + 'static, G: Fn(&T, &T) -> bool + 'static>(
        callback: F,
        trigger: G,
        priority: u64,
        once: bool,
        mute: bool,
    ) -> Self {
        Signal {
            callback: RefCell::new(Rc::new(callback)),
            trigger: RefCell::new(Rc::new(trigger)),
            id: new_signal_id(),
            priority: Cell::new(priority),
            once: Cell::new(once),
            mute: Cell::new(mute),
        }
    }

    /// Emits the [`Signal`], executing the [`Signal::callback`] function if the [`Signal::trigger`] condition is met.
    ///
    /// # Arguments
    ///
    /// * `old` - The old value of [`Signaled::val`].
    /// * `new` - The new value of [`Signaled::val`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowError`] if the [`Signal::callback`] or [`Signal::trigger`] are already mutably borrowed.
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
    pub fn emit(&self, old: &T, new: &T) -> Result<(), SignaledError> {
        if self.mute.get() {
            return Ok(());
        }
        let trigger = self
            .trigger
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError {
                source: ErrorSource::SignalTrigger,
            })?;
        if trigger(old, new) {
            let callback = self
                .callback
                .try_borrow()
                .map_err(|_| SignaledError::BorrowError {
                    source: ErrorSource::SignalCallback,
                })?;
            callback(old, new);
        }
        Ok(())
    }

    /// Sets a new [`Signal::callback`] for the [`Signal`].
    ///
    /// # Arguments
    ///
    /// * `callback` - The new [`Signal::callback`] function.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowMutError`] if the [`Signal::callback`] is already borrowed.
    pub fn set_callback<F: Fn(&T, &T) + 'static>(&self, callback: F) -> Result<(), SignaledError> {
        *self
            .callback
            .try_borrow_mut()
            .map_err(|_| SignaledError::BorrowMutError {
                source: ErrorSource::SignalCallback,
            })? = Rc::new(callback);
        Ok(())
    }

    /// Sets a new [`Signal::trigger`] for the [`Signal`].
    ///
    /// Default [`Signal::trigger`] always returns `true`.
    ///
    /// # Arguments
    ///
    /// * `trigger` - The new [`Signal::trigger`] function.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowMutError`] if the [`Signal::trigger`] is already borrowed.
    pub fn set_trigger<F: Fn(&T, &T) -> bool + 'static>(
        &self,
        trigger: F,
    ) -> Result<(), SignaledError> {
        *self
            .trigger
            .try_borrow_mut()
            .map_err(|_| SignaledError::BorrowMutError {
                source: ErrorSource::SignalTrigger,
            })? = Rc::new(trigger);
        Ok(())
    }

    /// Sets the [`Signal::trigger`] to always return true.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowMutError`] if the [`Signal::trigger`] is already borrowed.
    pub fn remove_trigger(&self) -> Result<(), SignaledError> {
        *self
            .trigger
            .try_borrow_mut()
            .map_err(|_| SignaledError::BorrowMutError {
                source: ErrorSource::SignalTrigger,
            })? = Rc::new(|_, _| true);
        Ok(())
    }

    /// Sets the execution priority of the [`Signal`].
    ///
    /// Default [`Signal::priority`] is 0.
    ///
    /// # Arguments
    ///
    /// * `priority` The new [`Signal::priority`] number, bigger number means higher priority.
    pub fn set_priority(&self, priority: u64) {
        self.priority.set(priority);
    }

    /// Sets the [`Signal::once`] flag of the [`Signal`].
    pub fn set_once(&self, is_once: bool) {
        self.once.set(is_once);
    }

    /// Sets the [`Signal::mute`] flag of the [`Signal`].
    pub fn set_mute(&self, is_mute: bool) {
        self.mute.set(is_mute);
    }

    /// Combines `N` amount of [`Signal`]s returning a single combined [`Signal`] instance and consuming the provided [`Signal`] instances.
    ///
    /// The order in which each [`Signal::callback`] will be called depends on the order that the [`Signal`]s are passed into the argument's slice.
    ///
    /// * `callback` will invoke all callbacks from the combined [`Signal`]s.
    ///
    /// * `trigger` will combine all triggers only returning `true` if every [`Signal::trigger`] does so.
    ///
    /// * `priority` will be the highest [`Signal::priority`] of the provided [`Signal`]s.
    ///
    /// * [`Signal::mute`] and [`Signal::once`] will be `false` by default.
    ///
    /// * [`Signal::id`] will be a new unique [`SignalId`].
    ///
    /// # Arguments
    ///
    /// * `signals` A slice containing the [`Signal`]s that will be combined into a single [`Signal`] instance.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowError`] if any of the provided [`Signal`] instances [`Signal::callback`] or [`Signal::trigger`] are mutably borrowed elsewhere.
    ///
    /// # Examples
    /// ```
    /// use std::cell::Cell;
    /// use std::rc::Rc;
    /// use signaled::{Signal, signal};
    ///
    /// let calls = Rc::new(Cell::new(0));
    ///
    /// let calls_clone = Rc::clone(&calls);
    /// let signal_a = signal!(move |_, _| calls_clone.set(calls_clone.get() + 1));
    ///
    /// let calls_clone = Rc::clone(&calls);
    /// let signal_b = signal!(move |_, _| calls_clone.set(calls_clone.get() + 2));
    ///
    /// let signal_c = Signal::combine(&[signal_a, signal_b]).unwrap();
    ///
    /// signal_c.emit(&(), &()).unwrap();
    /// assert_eq!(calls.get(), 3); // `signal_a` increases calls by 1 and `signal_b` increases calls by 2 so the `combined` signal increases calls by 3
    /// ```
    pub fn combine(signals: &[Signal<T>]) -> Result<Self, SignaledError> {
        let callbacks: Vec<Rc<Callback<T>>> = signals
            .iter()
            .map(|signal| {
                signal
                    .callback
                    .try_borrow()
                    .map(|rc| rc.clone())
                    .map_err(|_| SignaledError::BorrowError {
                        source: ErrorSource::SignalCallback,
                    })
            })
            .collect::<Result<_, _>>()?;

        let triggers: Vec<Rc<Trigger<T>>> = signals
            .iter()
            .map(|signal| {
                signal
                    .trigger
                    .try_borrow()
                    .map(|rc| rc.clone())
                    .map_err(|_| SignaledError::BorrowError {
                        source: ErrorSource::SignalTrigger,
                    })
            })
            .collect::<Result<_, _>>()?;

        let priority = signals
            .iter()
            .max_by_key(|s| s.priority.get())
            .map(|s| s.priority.get())
            .unwrap_or(0);

        Ok(Signal {
            callback: RefCell::new(Rc::new(move |old, new| {
                for callback in &callbacks {
                    callback(old, new);
                }
            })),
            trigger: RefCell::new(Rc::new(move |old, new| {
                triggers.iter().all(|tr| tr(old, new))
            })),
            id: new_signal_id(),
            priority: Cell::new(priority),
            once: Cell::new(false),
            mute: Cell::new(false),
        })
    }

    /// Returns [`SignalInfo`] with the data related to each field of [`Signal`]
    pub fn get_info(&self) -> SignalInfo {
        SignalInfo {
            id: self.id,
            priority: self.priority.get(),
            once: self.once.get(),
            mute: self.mute.get()
        }
    }
}

impl<T> Display for Signal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Signal {{ id: {}, priority: {}, once: {}, mute: {} }}",
            self.id,
            self.priority.get(),
            self.once.get(),
            self.mute.get()
        )
    }
}

impl<T> Debug for Signal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signal")
            .field("id", &self.id)
            .field("priority", &self.priority.get())
            .field("once", &self.once.get())
            .field("mute", &self.mute.get())
            .field("callback", &"<function>")
            .field("trigger", &"<function>")
            .finish()
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
            mute: Cell::new(false),
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
/// 1. `callback` - [`Signal::callback`]  
/// 2. `trigger` - [`Signal::trigger`]  
/// 3. `priority` - [`Signal::priority`]
/// 4. `once` - [`Signal::once`]
/// 5. `mute` - [`Signal::mute`]
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
/// When the value is updated via [`Signaled::set`], all [`Signal`]s in [`Signaled::signals`] are emitted in order of descending priority,
/// provided their trigger conditions are met.
/// 
/// ## *Signaled* has the following properties:
/// 
/// - [`Signaled::val`]: [`RefCell<T>`] that contains the wrapped value that emits [`Signal`]s when changed through [`Signaled::set()`] and similar methods.
/// 
/// - [`Signaled::signals`]: [`RefCell<Vec<Signal<T>>>`] The collection of [`Signal`]s that are emitted through [`Signaled::emit_signals()`] or [`Signaled::set()`] similar methods.
/// 
/// - [`Signaled::throttle_instant`]: [`Cell<Instant>`] used as reference for throttling.
/// 
/// - [`Signaled::throttle_duration`]: [`Cell<Duration>`] the duration of the throttle configured through [`Signaled::set_throttle_duration()`].
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
pub struct Signaled<T: 'static> {
    /// Reactive value, the mutation of this value through [`Signaled::set`] will emit all [`Signal`] inside [`Signaled::signals`].
    val: RefCell<T>,
    /// Collection of [`Signal`]s that will be emitted when [`Signaled::val`] is changed through [`Signaled::set`].
    signals: RefCell<Vec<Signal<T>>>,

    /// Instant to use as reference for throttling.
    throttle_instant: Cell<Instant>,
    /// Duration of the throttling.
    throttle_duration: Cell<Duration>,
}

impl<T> Signaled<T> {
    /// Creates a new instance of [`Signaled`] with the given initial value.
    ///
    /// # Arguments
    ///
    /// * `val` - The initial value for [`Signaled::val`].
    pub fn new(val: T) -> Self {
        Self {
            val: RefCell::new(val),
            signals: RefCell::new(Vec::new()),
            throttle_instant: Cell::new(Instant::now()),
            throttle_duration: Cell::new(Duration::ZERO),
        }
    }

    /// Sets a new value for [`Signaled::val`] and emits all [`Signal`]s.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of [`Signaled::val`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowMutError`] if [`Signaled::val`] or [`Signaled::signals`] are already borrowed.
    ///
    /// Returns [`SignaledError::BorrowError`] propagated from [`Signal::emit()`] if an individual [`Signal::callback`] or [`Signal::trigger`] is already mutably borrowed.
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
            &mut *self
                .val
                .try_borrow_mut()
                .map_err(|_| SignaledError::BorrowMutError {
                    source: ErrorSource::Value,
                })?,
            new_value,
        );
        let new_value_ref = self
            .val
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError {
                source: ErrorSource::Value,
            })?;
        self.emit_signals(&old_value, &new_value_ref)
    }

    /// Sets a new value for [`Signaled::val`] without emitting [`Signaled::signals`].
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of [`Signaled::val`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowMutError`] if [`Signaled::val`] is already borrowed.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{Signaled, Signal, signal};
    ///
    /// let signaled = Signaled::new(0);
    /// signaled.add_signal(signal!(|_, _| { println!("do something")})).unwrap();
    /// signaled.set_silent(1).unwrap(); // This does not emit the signal so "do something" is not printed.
    /// assert_eq!(signaled.get().unwrap(), 1);
    /// ```
    pub fn set_silent(&self, new_value: T) -> Result<(), SignaledError> {
        *self
            .val
            .try_borrow_mut()
            .map_err(|_| SignaledError::BorrowMutError {
                source: ErrorSource::Value,
            })? = new_value;
        Ok(())
    }

    /// Sets the minimum [`Duration`] that must elapse between consecutive calls to
    /// [`Signaled::set_throttled`].
    ///
    /// # Arguments
    ///
    /// * `duration` - The time interval to wait before another throttled update is allowed.
    pub fn set_throttle_duration(&self, duration: Duration) {
        self.throttle_duration.set(duration);
    }

    /// Sets a new value for [`Signaled::val`] and emits all [`Signal`]s, but only if enough time
    /// has passed since the previous throttled update.
    ///
    /// This method uses the [`Signaled::throttle_duration`] configured through [`Signaled::set_throttle_duration`]
    /// and the internal [`Signaled::throttle_instant`] to prevent signals from being emitted
    /// more frequently than allowed.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of [`Signaled::val`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowMutError`] if [`Signaled::val`] or [`Signaled::signals`] are already borrowed.
    ///
    /// Returns [`SignaledError::BorrowError`] propagated from [`Signal::emit()`] if an individual [`Signal::callback`] or [`Signal::trigger`] is already mutably borrowed.
    ///
    /// # Examples
    ///
    /// ```
    /// use signaled::{Signaled, Signal, signal};
    /// use std::time::Duration;
    /// use std::cell::Cell;
    /// use std::rc::Rc;
    ///
    /// let calls = Rc::new(Cell::new(0));
    /// let signaled = Signaled::new(());
    /// signaled.set_throttle_duration(Duration::from_millis(500));
    ///
    /// let calls_clone = Rc::clone(&calls);
    /// signaled.add_signal(signal!(move |_, _| calls_clone.set(calls_clone.get() + 1))).unwrap();
    ///
    /// signaled.set_throttled(()).unwrap();
    /// assert_eq!(calls.get(), 1);
    /// std::thread::sleep(Duration::from_millis(100));
    ///
    /// signaled.set_throttled(()).unwrap();
    /// assert_eq!(calls.get(), 1);
    /// std::thread::sleep(Duration::from_millis(500));
    ///
    /// signaled.set_throttled(()).unwrap();
    /// assert_eq!(calls.get(), 2);
    /// ```
    pub fn set_throttled(&self, new_value: T) -> Result<(), SignaledError> {
        if Instant::now() < self.throttle_instant.get() {
            return Ok(());
        }

        self.set(new_value)?;
        self.throttle_instant
            .set(Instant::now() + self.throttle_duration.get());
        Ok(())
    }

    /// Sets a new value for [`Signaled::val`] without emitting [`Signaled::signals`], but only if enough time
    /// has passed since the previous throttled update.
    ///
    /// # Arguments
    ///
    /// * `new_value` - The new value of [`Signaled::val`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowMutError`] if [`Signaled::val`] is already borrowed.
    pub fn set_silent_throttled(&self, new_value: T) -> Result<(), SignaledError> {
        if Instant::now() < self.throttle_instant.get() {
            return Ok(());
        }

        self.set_silent(new_value)?;
        self.throttle_instant
            .set(Instant::now() + self.throttle_duration.get());
        Ok(())
    }

    /// Returns a reference to the current [`Signaled::val`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowError`] if [`Signaled::val`] is mutably borrowed.
    pub fn get_ref(&self) -> Result<Ref<'_, T>, SignaledError> {
        match self.val.try_borrow() {
            Ok(r) => Ok(r),
            Err(_) => Err(SignaledError::BorrowError {
                source: ErrorSource::Value,
            }),
        }
    }

    /// Emits all [`Signal`]s in [`Signaled::signals`] in descending priority order, invoking their callbacks if their trigger condition is met.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowMutError`] if [`Signaled::signals`] is already borrowed.
    ///
    /// Returns [`SignaledError::BorrowError`] propagated from [`Signal::emit()`] if an individual [`Signal::callback`] or [`Signal::trigger`] is already mutably borrowed.
    fn emit_signals(&self, old: &T, new: &T) -> Result<(), SignaledError> {
        match self.signals.try_borrow_mut() {
            Ok(mut signals) => {
                signals.sort_by(|a, b| b.priority.get().cmp(&a.priority.get()));
                for signal in signals.iter() {
                    signal.emit(old, new)?
                }
                signals
                    .retain(|s| !s.once.get() || (s.once.get() && !(s.trigger.borrow())(old, new)));
                Ok(())
            }
            Err(_) => Err(SignaledError::BorrowMutError {
                source: ErrorSource::Signals,
            }),
        }
    }

    /// Adds a [`Signal`] to the [`Signaled::signals`] collection, returning its [`SignalId`].
    ///
    /// If a [`Signal`] with the same [`SignalId`] is already in the collection, returns the existing [`SignalId`] without adding the [`Signal`] to the collection.
    ///
    /// # Arguments
    /// * `signal` - The [`Signal`] to add.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowMutError`] if [`Signaled::signals`] is already borrowed.
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
            Err(_) => Err(SignaledError::BorrowMutError {
                source: ErrorSource::Signals,
            }),
        }
    }

    /// Removes a [`Signal`] by [`SignalId`] from [`Signaled::signals`], returning the removed [`Signal`].
    ///
    /// # Arguments
    ///
    /// * `id` - The [`SignalId`] of the [`Signal`] to remove.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowMutError`] if [`Signaled::signals`] is already borrowed.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the [`SignalId`] does not match any [`Signal::id`].
    pub fn remove_signal(&self, id: SignalId) -> Result<Signal<T>, SignaledError> {
        match self.signals.try_borrow_mut() {
            Ok(mut s) => {
                let index = s
                    .iter()
                    .position(|s| s.id == id)
                    .ok_or(SignaledError::InvalidSignalId { id })?;
                Ok(s.remove(index))
            }
            Err(_) => Err(SignaledError::BorrowMutError {
                source: ErrorSource::Signals,
            }),
        }
    }

    /// Sets the [`Signal::callback`] of a [`Signal`] by [`SignalId`].
    ///
    /// # Arguments
    ///
    /// * `id` - The [`Signal`] of the [`Signal`].
    /// * `callback` - The new [`Signal::callback`] function.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowError`] if the [`Signaled::signals`] is already mutably borrowed.
    ///
    /// Returns [`SignaledError::BorrowMutError`] if the targeted [`Signal::callback`] is already borrowed.
    /// 
    /// Returns [`SignaledError::InvalidSignalId`] if the [`SignalId`] does not match any [`Signal::id`].
    pub fn set_signal_callback<F: Fn(&T, &T) + 'static>(
        &self,
        id: SignalId,
        callback: F,
    ) -> Result<(), SignaledError> {
        let signals = self
            .signals
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError {
                source: ErrorSource::Signals,
            })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_callback(callback)
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the [`Signal::trigger`] condition for a [`Signal`] by [`SignalId`].
    ///
    /// # Arguments
    ///
    /// * `id` - The [`SignalId`] of the [`Signal`].
    /// * `trigger` - The new [`Signal::trigger`] function.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowError`] if the [`Signaled::signals`] is already mutably borrowed.
    ///
    /// Returns [`SignaledError::BorrowMutError`] if the targeted [`Signal::trigger`] is already borrowed.
    /// 
    /// Returns [`SignaledError::InvalidSignalId`] if the [`SignalId`] does not match any [`Signal::id`].
    pub fn set_signal_trigger<F: Fn(&T, &T) -> bool + 'static>(
        &self,
        id: SignalId,
        trigger: F,
    ) -> Result<(), SignaledError> {
        let signals = self
            .signals
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError {
                source: ErrorSource::Signals,
            })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_trigger(trigger)
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the [`Signal::trigger`] to always return `true` by [`SignalId`]
    ///
    /// # Arguments
    ///
    /// * `id` - The [`SignalId`] of the [`Signal`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowError`] if the [`Signaled::signals`] is already mutably borrowed.
    ///
    /// Returns [`SignaledError::BorrowMutError`] if the targeted [`Signal::trigger`] is already borrowed.
    /// 
    /// Returns [`SignaledError::InvalidSignalId`] if the [`SignalId`] does not match any [`Signal::id`].
    pub fn remove_signal_trigger(&self, id: SignalId) -> Result<(), SignaledError> {
        let signals = self
            .signals
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError {
                source: ErrorSource::Signals,
            })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.remove_trigger()
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the [`Signal::priority`] for a [`Signal`] by [`SignalId`].
    ///
    /// # Arguments
    ///
    /// * `id` - The [`SignalId`] of the [`Signal`].
    /// * `priority` - The new [`Signal::priority`] value (higher executes first).
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowError`] if the [`Signaled::signals`] is already mutably borrowed.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if [`SignalId`] does not match any [`Signal::id`].
    pub fn set_signal_priority(&self, id: SignalId, priority: u64) -> Result<(), SignaledError> {
        let signals = self
            .signals
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError {
                source: ErrorSource::Signals,
            })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_priority(priority);
            Ok(())
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the [`Signal::once`] flag for a [`Signal`] by [`SignalId`].
    ///
    /// # Arguments
    ///
    /// * `id` - The [`SignalId`] of the [`Signal`].
    /// * `is_once` - Boolean that represents the new [`Signaled::once`] value.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowError`] if the [`Signaled::signals`] is already mutably borrowed.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if [`SignalId`] does not match any [`Signal::id`].
    pub fn set_signal_once(&self, id: SignalId, is_once: bool) -> Result<(), SignaledError> {
        let signals = self
            .signals
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError {
                source: ErrorSource::Signals,
            })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_once(is_once);
            Ok(())
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Sets the [`Signal::mute`] flag for a [`Signal`] by [`SignalId`].
    ///
    /// # Arguments
    ///
    /// * `id` - The [`SignalId`] of the [`Signal`].
    /// * `is_mute` - Boolean that represents the new [`Signal::mute`]
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowError`] if the [`Signaled::signals`] is already mutably borrowed.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if [`SignalId`] does not match any [`Signal::id`].
    pub fn set_signal_mute(&self, id: SignalId, is_mute: bool) -> Result<(), SignaledError> {
        let signals = self
            .signals
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError {
                source: ErrorSource::Signals,
            })?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_mute(is_mute);
            Ok(())
        } else {
            Err(SignaledError::InvalidSignalId { id })
        }
    }

    /// Combines multiple [`Signal`]s by their [`SignalId`] into a single [`Signal`] consuming the [`Signal`] instances associated with the provided [`SignalId`]s.
    ///
    /// This function finds signals by their [`Signal::id`], removes them from the [`Signaled::signals`] instance,
    /// and then uses [`Signal::combine()`] to create a new, single [`Signal`] instance.
    /// This new combined signal is then added back to the [`Signaled::signals`] collection and its new [`SignalId`] is returned.
    ///
    /// For more details about the combination process,
    /// see the documentation for [`Signal::combine()`].
    ///
    /// # Arguments
    ///
    /// * `signal_ids` A slice containing the [`SignalId`]s of the [`Signal`]s that will be combined into a single [`Signal`].
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowMutError`] if [`Signaled::signals`] is already borrowed.
    ///
    /// Returns [`SignaledError::BorrowError`] if any of the targeted [`Signal`] instances [`Signal::callback`] or [`Signal::trigger`] are mutably borrowed elsewhere.
    ///
    /// Returns [`SignaledError::InvalidSignalId`] if the any of the provided [`SignalId`] does not match any [`Signal::id`].
    /// # Examples
    /// ```
    /// use std::cell::Cell;
    /// use std::rc::Rc;
    /// use signaled::{Signal, Signaled, signal};
    ///
    /// let signaled = Signaled::new(0);
    ///
    /// let calls = Rc::new(Cell::new(0));
    ///
    /// let calls_clone = Rc::clone(&calls);
    /// let signal_a = signal!(move |_, _| calls_clone.set(calls_clone.get() + 1));
    /// let signal_a_id = signaled.add_signal(signal_a).unwrap();
    ///
    /// let calls_clone = Rc::clone(&calls);
    /// let signal_b = signal!(move |_, _| calls_clone.set(calls_clone.get() + 2));
    /// let signal_b_id = signaled.add_signal(signal_b).unwrap();
    ///
    /// signaled.combine_signals(&[signal_a_id, signal_b_id]).unwrap();
    ///
    /// signaled.set(1).unwrap();
    /// assert_eq!(calls.get(), 3); // `signal_a` increases calls by 1 and `signal_b` increases calls by 2 so the `combined` signal increases calls by 3
    /// ```
    pub fn combine_signals(&self, signal_ids: &[SignalId]) -> Result<SignalId, SignaledError> {
        let mut target_signals = Vec::new();
        let mut signals =
            self.signals
                .try_borrow_mut()
                .map_err(|_| SignaledError::BorrowMutError {
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

    /// Returns a [`Vec<SignalInfo>`] containing a [`SignalInfo`] for each [`Signal`] present in [`Signaled::signals`] (private field).
    /// 
    /// # Errors 
    ///    
    /// Returns [`SignaledError::BorrowError`] if the [`Signaled::signals`] is already mutably borrowed.
    pub fn get_signals_info(&self) -> Result<Vec<SignalInfo>, SignaledError> {
        let signals = self
            .signals
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError {
                source: ErrorSource::Signals,
            })?
            .iter()
            .map(|s| s.get_info())
            .collect();
        Ok(signals)
    }

    /// Returns the [`SignalInfo`] belonging to the [`Signal`] that matches the specified [`SignalId`].
    /// 
    /// # Errors
    /// 
    /// Returns [`SignaledError::BorrowError`] if the [`Signaled::signals`] is already mutably borrowed.
    /// 
    /// Returns [`SignaledError::InvalidSignalId`] if the provided [`SignalId`] does not match any [`Signal`].
    pub fn get_signal_info(&self, id: SignalId) -> Result<SignalInfo, SignaledError> {
        let signals = self
            .signals
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError {
                source: ErrorSource::Signals,
            })?;
        let signal_info = signals
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.get_info())
            .ok_or(SignaledError::InvalidSignalId { id })?;
        Ok(signal_info)
    }

    /// Returns the [`Signaled::throttle_duration`] (private field) that is configured through [`Signaled::set_throttle_duration`].
    pub fn get_throttle_duration(&self) -> Duration {
        self.throttle_duration.get()
    }

}

impl<T: Display> Display for Signaled<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = self
            .val
            .try_borrow()
            .map(|v| v.to_string())
            .unwrap_or_else(|_| "<borrowed>".to_string());

        let signals_display = self
            .signals
            .try_borrow()
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
            .unwrap_or_else(|_| "signals: <borrowed>".to_string());

        write!(f, "Signaled {{ val: {}, {} }}", value, signals_display)
    }
}

impl<T: Debug> Debug for Signaled<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("Signaled");
        match self.val.try_borrow() {
            Ok(val) => s.field("val", &*val),
            Err(_) => s.field("val", &"<borrowed>"),
        };
        match self.signals.try_borrow() {
            Ok(signals) => s.field("signals", &*signals),
            Err(_) => s.field("signals", &"<borrowed>"),
        };
        s.finish()
    }
}

impl<T: Clone> Signaled<T> {
    /// Returns a cloned copy of the current [`Signaled::val`] value.
    ///
    /// # Errors
    ///
    /// Returns [`SignaledError::BorrowError`] if [`Signal::val`] is already mutably borrowed.
    pub fn get(&self) -> Result<T, SignaledError> {
        match self.val.try_borrow() {
            Ok(r) => Ok(r.clone()),
            Err(_) => Err(SignaledError::BorrowError {
                source: ErrorSource::Value,
            }),
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
        signaled
            .add_signal(signal!(move |_, _| {
                calls_clone.set(calls_clone.get() + 1)
            }))
            .unwrap();

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
        let signal_id = signaled
            .add_signal(signal!(move |_, _| {
                calls_clone.set(calls_clone.get() + 1)
            }))
            .unwrap();

        signaled.set(2).unwrap(); // Calls = 1
        assert_eq!(calls.get(), 1);

        signaled.set(3).unwrap(); // Calls = 2
        assert_eq!(calls.get(), 2);

        signaled.remove_signal(signal_id).unwrap(); // Signal removed so `calls` will not increase when `signaled` emits.

        signaled.set(4).unwrap(); // Calls = 2
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
        let signal_id = signaled
            .add_signal(signal!(move |_, _| {
                calls_clone.set(calls_clone.get() + 1)
            }))
            .unwrap();

        signaled.set(2).unwrap(); // Calls = 1
        assert_eq!(calls.get(), 1);

        signaled.set(3).unwrap(); // Calls = 2
        assert_eq!(calls.get(), 2);

        let calls_clone = Rc::clone(&calls);
        signaled
            .set_signal_callback(signal_id, move |_, _| {
                calls_clone.set(calls_clone.get() + 2);
            })
            .unwrap(); // New callback increases call count by 2.

        signaled.set(4).unwrap(); // Calls = 4
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

        signaled
            .set_signal_trigger(signal_id, |_, new| *new < 5)
            .unwrap(); // Trigger condition changed so `new_value` being < 5 will invoke callback.

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
        let signal_a: Signal<i32> = signal!(move |_, _| {
            test_value_clone.set('a');
        });
        signal_a.set_priority(3);

        let test_value_clone = Rc::clone(&test_value);
        let signal_b: Signal<i32> = signal!(move |_, _| {
            assert_eq!(test_value_clone.get(), 'a');
        }); // Signal in the middle with a callback that checks that `signal_a` callback has been invoked first.
        signal_b.set_priority(2);

        let test_value_clone = Rc::clone(&test_value);
        let signal_c: Signal<i32> = signal!(move |_, _| {
            test_value_clone.set('c');
        });
        signal_c.set_priority(1);

        let signal_a_id = signaled.add_signal(signal_a).unwrap();
        let signal_b_id = signaled.add_signal(signal_b).unwrap();
        let signal_c_id = signaled.add_signal(signal_c).unwrap();

        signaled.set(0).unwrap();
        assert_eq!(test_value.get(), 'c'); // `signal_c` has the lowest priority (1) so after all callbacks are invoked the value will be 'c'.

        signaled.set_signal_priority(signal_a_id, 1).unwrap(); // Set priority to 1, `signal_a` will now be the last to execute.

        let test_value_clone = Rc::clone(&test_value);
        signaled
            .set_signal_callback(signal_b_id, move |_, _| {
                assert_eq!(test_value_clone.get(), 'c')
            })
            .unwrap(); // Modify signal in the middle callback to check that `signal_c` callback has been invoked first.

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

    test_signaled_borrow_error!(test_borrow_mut_error_value, val, borrow, set, 1; SignaledError::BorrowMutError { source: ErrorSource::Value });
    test_signaled_borrow_error!(test_borrow_mut_error_signals, signals, borrow, add_signal, signal!(|_, _| {}); SignaledError::BorrowMutError { source: ErrorSource::Signals });
    test_signaled_borrow_error!(test_borrow_error_signals, signals, borrow_mut, set_signal_callback, |_, _| {}; SignaledError::BorrowError { source: ErrorSource::Signals }, true);
    test_signaled_borrow_error!(test_borrow_error_value, val, borrow_mut, get; SignaledError::BorrowError { source: ErrorSource::Value });

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
    test_signal_borrow_error!(test_borrow_error_signal_callback, callback, borrow_mut, emit, &1, &2; SignaledError::BorrowError { source: ErrorSource::SignalCallback });
    test_signal_borrow_error!(test_borrow_error_signal_trigger, trigger, borrow_mut, emit, &1, &2; SignaledError::BorrowError { source: ErrorSource::SignalTrigger });
    test_signal_borrow_error!(test_borrow_mut_error_signal_callback, callback, borrow, set_callback, |_, _| {}; SignaledError::BorrowMutError { source: ErrorSource::SignalCallback });
    test_signal_borrow_error!(test_borrow_mut_error_signal_trigger, trigger, borrow, set_trigger, |_, _| true; SignaledError::BorrowMutError { source: ErrorSource::SignalTrigger });

    /////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

    #[test]
    fn test_invalid_id() {
        let signaled = Signaled::new(0);
        let signal_id = signaled.add_signal(signal!(|_, _| {})).unwrap();
        assert!(
            signaled
                .remove_signal(signal_id + 1)
                .is_err_and(|e| e == SignaledError::InvalidSignalId { id: signal_id + 1 })
        );
    }

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
    fn test_getters() {
        let signaled = Signaled::new(0);
        assert_eq!(*signaled.get_ref().unwrap(), 0);
        assert_eq!(signaled.get().unwrap(), 0);

        for i in 0..10 {
            signaled.set(i).unwrap();
            assert_eq!(*signaled.get_ref().unwrap(), i);
            assert_eq!(signaled.get().unwrap(), i);
        }
    }

    #[test]
    fn test_old_new() {
        let signaled = Signaled::new(0);
        signaled
            .add_signal(signal!(|old, new| assert!(*new == *old + 1)))
            .unwrap();

        for i in 1..10 {
            signaled.set(i).unwrap();
        }
    }

    #[test]
    fn test_set_silent() {
        let calls = Rc::new(Cell::new(0));
        let signaled = Signaled::new(0);

        let calls_clone = Rc::clone(&calls);
        signaled
            .add_signal(signal!(move |_, _| calls_clone.set(calls_clone.get() + 1)))
            .unwrap();

        signaled.set(1).unwrap();
        assert_eq!(signaled.get().unwrap(), 1);
        assert_eq!(calls.get(), 1);

        signaled.set_silent(2).unwrap();
        assert_eq!(signaled.get().unwrap(), 2);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn test_combine() {
        let calls = Rc::new(Cell::new(0));
        let calls_clone = Rc::clone(&calls);
        let signal_a: Signal<i32> = Signal::new(
            move |_, _| calls_clone.set(calls_clone.get() + 1),
            |_, new| *new > 10,
            100,
            false,
            false,
        );

        let calls_clone = Rc::clone(&calls);
        let signal_b: Signal<i32> = Signal::new(
            move |_, _| calls_clone.set(calls_clone.get() + 2),
            |old, _| *old > 10,
            10,
            false,
            false,
        );

        signal_a.emit(&0, &9).unwrap(); // `new` < 10, Calls = 0
        assert_eq!(calls.get(), 0);
        signal_a.emit(&0, &11).unwrap(); // `new` > 10, Calls = 1
        assert_eq!(calls.get(), 1);

        signal_b.emit(&9, &0).unwrap(); // `old` < 10, Calls = 1
        assert_eq!(calls.get(), 1);
        signal_b.emit(&11, &0).unwrap(); // `old` > 10, Calls = 3
        assert_eq!(calls.get(), 3);

        let signal_c = Signal::combine(&[signal_a, signal_b]).unwrap();
        assert_eq!(signal_c.priority.get(), 100); // Keeps the highest priority after combining

        signal_c.emit(&9, &9).unwrap(); // Both triggers return false
        assert_eq!(calls.get(), 3);
        signal_c.emit(&9, &11).unwrap(); // One triggers return false
        assert_eq!(calls.get(), 3);
        signal_c.emit(&11, &11).unwrap(); // Both triggers return true, Calls = 6
        assert_eq!(calls.get(), 6);
    }

    #[test]
    fn test_combine_signals() {
        let signaled = Signaled::new(0);
        let signal_a_id = signaled.add_signal(signal!(|_, _| {})).unwrap();
        let signal_b_id = signaled.add_signal(signal!(|_, _| {})).unwrap();
        let signal_c_id = signaled.add_signal(signal!(|_, _| {})).unwrap();
        let signal_d_id = signaled.add_signal(signal!(|_, _| {})).unwrap();
        assert_eq!(signaled.signals.borrow().len(), 4);

        signaled
            .combine_signals(&[signal_a_id, signal_b_id, signal_c_id, signal_d_id])
            .unwrap();
        assert_eq!(signaled.signals.borrow().len(), 1);
    }

    #[test]
    fn test_once_retain() {
        let signaled = Signaled::new(1);
        signaled
            .add_signal(signal!(|_, _| {}, |old, new| *new > *old, 1, true, false))
            .unwrap();
        assert_eq!(signaled.signals.borrow().len(), 1);

        signaled.set(1).unwrap();
        assert_eq!(signaled.signals.borrow().len(), 1);

        signaled.set(2).unwrap();
        assert_eq!(signaled.signals.borrow().len(), 0);
    }

    #[test]
    fn test_set_throttled() {
        let calls = Rc::new(Cell::new(0));
        let signaled = Signaled::new(());
        signaled.set_throttle_duration(Duration::from_millis(500));

        let calls_clone = Rc::clone(&calls);
        signaled
            .add_signal(signal!(move |_, _| calls_clone.set(calls_clone.get() + 1)))
            .unwrap();

        signaled.set_throttled(()).unwrap();
        assert_eq!(calls.get(), 1);
        std::thread::sleep(Duration::from_millis(100));

        signaled.set_throttled(()).unwrap();
        assert_eq!(calls.get(), 1);
        std::thread::sleep(Duration::from_millis(500));

        signaled.set_throttled(()).unwrap();
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn test_reentrant_set() {
        let signaled = Rc::new(Signaled::new(0));
        let signaled_clone = Rc::clone(&signaled);

        signaled
            .add_signal(signal!(move |_, _| {
                let err = signaled_clone.set(2).unwrap_err();
                assert_eq!(
                    err,
                    SignaledError::BorrowMutError {
                        source: ErrorSource::Value
                    }
                );
            }))
            .unwrap();

        signaled.set(1).unwrap();
    }

    #[test]
    fn test_get_signals_info() {
        let signaled = Signaled::new(());
        let signal_a_id = signaled.add_signal(signal!(|_, _| {}, |_, _| true, 7, false, true)).unwrap();
        let signal_b_id = signaled.add_signal(signal!(|_, _| {}, |_, _| true, 3, true, false)).unwrap();

        let signals = signaled.get_signals_info().unwrap();
        assert_eq!(signals[0], SignalInfo { id: signal_a_id, priority: 7, once: false, mute: true});
        assert_eq!(signals[1], SignalInfo { id: signal_b_id, priority: 3, once: true, mute: false});
    }

    #[test]
    fn test_get_signal_info() {
        let signaled = Signaled::new(());
        let signal_a_id = signaled.add_signal(signal!(|_, _| {}, |_, _| true, 7, false, true)).unwrap();

        let signal_a_info = signaled.get_signal_info(signal_a_id).unwrap();
        assert_eq!(signal_a_info, SignalInfo { id: signal_a_id, priority: 7, once: false, mute: true});
    }

    #[test]
    fn test_get_throttle_duration() {
        let signaled = Signaled::new(());
        signaled.set_throttle_duration(Duration::from_millis(500));
        assert_eq!(Duration::from_millis(500), signaled.get_throttle_duration());
    }
}
