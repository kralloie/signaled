use signaled::{
    signal_sync,
    sync::{Signal, Signaled},
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn main() {
    println!("[Main] Creating a new thread-safe Signaled value.");
    let calls = Arc::new(Mutex::new(0));
    let signaled = Signaled::new(());

    println!("[Main] Setting throttle duration to 500ms.");
    signaled
        .set_throttle_duration(Duration::from_millis(500))
        .unwrap();

    println!("[Main] Adding a signal that counts how many times it has fired.");
    let calls_clone = Arc::clone(&calls);
    signaled
        .add_signal(signal_sync!(move |_, _| {
            println!("[Callback] Throttled signal fired!");
            let mut lock = calls_clone.lock().unwrap();
            *lock += 1;
        }))
        .unwrap();

    println!("\n[Main] Calling set_throttled(). This should trigger the signal.");
    signaled.set_throttled(()).unwrap();
    assert_eq!(*calls.lock().unwrap(), 1);
    println!("[Main] Call count: 1");

    println!("\n[Main] Sleeping for 100ms...");
    std::thread::sleep(Duration::from_millis(100));

    println!("[Main] Calling set_throttled() again. This call should be ignored.");
    signaled.set_throttled(()).unwrap();
    assert_eq!(*calls.lock().unwrap(), 1);
    println!("[Main] Call count is still 1 (throttled).");

    println!("\n[Main] Sleeping for 500ms...");
    std::thread::sleep(Duration::from_millis(500));

    println!("[Main] Calling set_throttled() again. This should trigger the signal.");
    signaled.set_throttled(()).unwrap();
    assert_eq!(*calls.lock().unwrap(), 2);
    println!("[Main] Call count: 2");
}
