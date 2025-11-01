use signaled::signal_sync;
use signaled::sync::{Signal, Signaled, SignaledError};
use std::sync::Arc;
use std::thread;

fn main() {
    // Use a flag to ensure the panic only happens once.
    let should_panic = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let signaled = Arc::new(Signaled::new(0));

    let panic_signal = signal_sync!(move |_, _| {
        if should_panic.swap(false, std::sync::atomic::Ordering::SeqCst) {
            panic!("This panic will poison the signal lock!");
        }
    });

    signaled.add_signal(panic_signal).unwrap();

    println!("Spawning a thread that will panic inside a signal callback...");

    let signaled_clone = Arc::clone(&signaled);
    let handle = thread::spawn(move || {
        // This call to set() will trigger the panic.
        let _ = signaled_clone.set(1);
    });

    // Wait for the thread to panic and finish.
    let thread_result = handle.join();
    assert!(thread_result.is_err());
    println!("Thread panicked as expected.");

    println!("\nAttempting to call set() again on the poisoned Signaled...");
    let result = signaled.set(2);

    match result {
        Err(SignaledError::PoisonedLock { source }) => {
            println!("Correctly received a PoisonedLock error!");
            println!("Error source: {:?}", source);
        }
        _ => {
            panic!("Expected a PoisonedLock error, but got something else.");
        }
    }
}
