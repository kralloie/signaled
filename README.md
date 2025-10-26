<div align="center">

<img src="https://github.com/kralloie/signaled/blob/main/assets/signaled.png" width="200">

# Signaled

<img src="https://github.com/kralloie/signaled/actions/workflows/tests.yml/badge.svg" alt="test">
<img src="https://github.com/kralloie/signaled/actions/workflows/docs.yml/badge.svg" alt="docs">
<img src="https://img.shields.io/badge/license-MIT-green.svg" alt="License">
</div>
<br>

A lightweight reactive programming library for Rust, providing a signal-slot mechanism. `Signaled<T>` holds a value and emits signals to registered callbacks when the value changes.

This library comes in two versions:
- **`signaled::sync`**: A thread-safe implementation using `RwLock` and `Mutex`. Recommended for most applications.
- **`signaled`**: A single-threaded implementation using `RefCell`. Ideal for contexts where thread safety is not required.

## Features

- **Reactive Updates**: Update a value and automatically emit signals to registered callbacks.
- **Priority-Based Signals**: Signals are executed in descending priority order.
- **Conditional Triggers**: Signals can have trigger functions to control callback execution.
- **One-Time Signals**: Signals can be flagged as `once`. A `once` signal is automatically removed after its callback is successfully executed (i.e., when its trigger condition is met).

---

## Thread-Safe Usage (`signaled::sync`)

The `sync` module provides a fully thread-safe implementation suitable for multi-threaded applications. It uses `RwLock` and `Mutex` for interior mutability.

### Example

```rust
use signaled::sync::{Signaled, Signal};
use signaled::signal_sync;
use std::sync::{Arc, Mutex};

let signaled = Arc::new(Signaled::new(0));
let calls = Arc::new(Mutex::new(0));

let calls_clone = Arc::clone(&calls);
let signal = signal_sync!(move |old: &i32, new: &i32| {
    println!("Value changed: {} -> {}", old, new);
    let mut lock = calls_clone.lock().unwrap();
    *lock += 1;
});

signaled.add_signal(signal).unwrap();

// Set the value from different threads
let threads: Vec<_> = (1..=3).map(|i| {
    let signaled_clone = Arc::clone(&signaled);
    std::thread::spawn(move || {
        signaled_clone.set(i).unwrap();
    })
}).collect();

for handle in threads {
    handle.join().unwrap();
}

assert_eq!(*calls.lock().unwrap(), 3);
println!("Final value: {}", signaled.get().unwrap());
```

### Error Handling (`sync`)

Methods may return `SignaledError` for:
- `PoisonedLock`: Attempted to acquire a poisoned `RwLock` or `Mutex`.
- `WouldBlock`: A `try_` method failed to acquire a lock immediately.
- `InvalidSignalId`: Provided a `Signal` ID that does not exist.

### ⚠️ Deadlock Warning
Incorrectly managing locks can lead to deadlocks. For example, holding a read lock on the `Signaled` value while trying to call `set` from the same thread will deadlock. Non-blocking `try_*` methods are provided as an alternative.

---

## Single-Threaded Usage (`signaled`)

This is a high-performance version for single-threaded contexts. It uses `RefCell` for interior mutability and provides runtime borrow checking.

### Example

```rust
use signaled::{Signaled, Signal, signal};

let signaled = Signaled::new(0);
let high_priority = signal!(|old: &i32, new: &i32| println!("High: Old: {}, New: {}", old, new));
high_priority.set_priority(10);

let conditional = signal!(|old: &i32, new: &i32| println!("Conditional: Old: {}, New: {}", old, new));
conditional.set_trigger(|old: &i32, new: &i32| *new > *old + 5).unwrap();

signaled.add_signal(high_priority).unwrap();
signaled.add_signal(conditional).unwrap();

signaled.set(10).unwrap();
signaled.set(3).unwrap();
```

<details>
<summary>Console Output</summary>

```console
High: Old: 0, New: 10
Conditional: Old: 0, New: 10
High: Old: 10, New: 3
```

</details>

### Error Handling (Single-Threaded)

Methods may return `SignaledError` for:
- `BorrowError`: Attempted to immutably borrow a value already mutably borrowed.
- `BorrowMutError`: Attempted to mutably borrow a value already borrowed.
- `InvalidSignalId`: Provided a `Signal` ID that does not exist.

### ⚠️ Re-entrant Calls
Recursive or re-entrant calls (e.g., calling `set` from within a signal's callback) may cause a panic due to `RefCell` borrow errors.