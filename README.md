# Signaled 📡

A lightweight reactive programming library for Rust, providing a signal-slot mechanism.
`Signaled<T>` holds a value of type `T` and a collection of `Signal<T>` instances, which are
callbacks triggered when the value changes. Signals support priorities and conditional triggers,
making it ideal for reactive UI updates, event handling, or state management.

## Features

- **Reactive Updates**: Update a value and automatically emit signals to registered callbacks.
- **Priority-Based Signals**: Signals are executed in descending priority order.
- **Conditional Triggers**: Signals can have trigger functions to control callback execution.
- **Safe Mutability**: Uses `RefCell` for interior mutability with runtime borrow checking.
- **Error Handling**: Returns `Result` with `SignaledError` for borrow conflicts and invalid signal IDs.

## Limitations

- Single-threaded due to use of `Rc` and `RefCell`. For multi-threaded use, consider wrapping in `Arc<Mutex<_>>`.
- Cloning a `Signaled<T>` or `Signal<T>` may panic if the underlying `RefCell` is borrowed.
- Recursive calls to `set` or `emit` in signal callbacks may cause borrow errors.
- Re-entrant calls (e.g. calling `set` from within the callback of a `Signal<T>`) may panic due borrow errors.

## Examples

Basic usage to create a `Signaled<i32>`, add a signal, and emit changes:

```rust
use signaled::{Signaled, Signal, SignaledError};

let signaled = Signaled::new(0);
let signal = Signal::new(|old: &i32, new: &i32| println!("Old: {} | New: {}", old, new));
signaled.add_signal(signal).unwrap();
signaled.set(42).unwrap(); // Prints "Old: 0 | New: 42"
```

Using priorities and triggers:

```rust
use signaled::{Signaled, Signal, SignaledError};

let signaled = Signaled::new(0);
let high_priority = Signal::new(|old: &i32, new: &i32| println!("High: Old: {}, New: {}", old, new));
high_priority.set_priority(10);
let conditional = Signal::new(|old: &i32, new: &i32| println!("Conditional: Old: {}, New: {}", old, new));
conditional.set_trigger(|old: &i32, new: &i32| *new > *old + 5).unwrap();
signaled.add_signal(high_priority).unwrap();
signaled.add_signal(conditional).unwrap();
signaled.set(10).unwrap(); // Prints "High: Old: 0, New: 10" and "Conditional: Old: 0, New: 10"
signaled.set(3).unwrap(); // Prints only "High: Old: 10, New: 3"
```

## Error Handling

Methods like `set`, `emit`, and `remove_signal` return `Result` with `SignaledError` for:
- `BorrowError`: Attempted to immutably borrow a value already borrowed.
- `BorrowMutError`: Attempted to mutably borrow a value already borrowed.
- `InvalidSignalId`: Provided a signal ID that does not exist.

```rust
use signaled::{Signaled, SignaledError, ErrorType, ErrorSource};

let signaled = Signaled::new(0);
let _borrow = signaled.get_ref().unwrap();
let result = signaled.set(1);
assert!(matches!(
    result,
    Err(SignaledError { message }) if message == "Cannot mutably borrow Signaled value, it is already borrowed"
));
```
