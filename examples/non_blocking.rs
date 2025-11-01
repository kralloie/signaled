use signaled::{
    signal_sync,
    sync::{ErrorSource, Signal, Signaled, SignaledError},
};
use std::sync::Arc;

fn main() {
    let signaled = Arc::new(Signaled::new(0));

    let signaled_clone = Arc::clone(&signaled);
    let reentrant_signal = signal_sync!(move |_, _| {
        println!("[Callback] Fired! Now attempting a re-entrant call to try_set().");

        // This call is happening while the locks for the initial `set()` are still held.
        // `try_set()` will attempt to acquire a write lock on `val`, which is already
        // write-locked by the outer `set()` call, so it should fail immediately.
        let result = signaled_clone.try_set(999);

        println!("[Callback] try_set() result: {:?}", result);
        assert!(matches!(
            result,
            Err(SignaledError::WouldBlock {
                source: ErrorSource::Value
            })
        ));
        println!("[Callback] Correctly received WouldBlock error as expected!");
    });

    signaled.add_signal(reentrant_signal).unwrap();

    println!("[Main] Calling set(1), which will trigger the callback...");
    signaled.set(1).unwrap();
}
