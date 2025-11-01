use signaled::{
    signal_sync,
    sync::{Signal, Signaled},
};
use std::thread;
use std::time::Duration;

fn main() {
    println!("[Main] Creating a new thread-safe Signaled value.");
    let signaled = Signaled::new(0);

    println!("[Main] Adding a signal that will run in a separate thread.");
    signaled
        .add_signal(signal_sync!(move |old, new| {
            println!("[Callback] Signal emitting in a separate thread...");
            println!("[Callback] Value changed from {} to {}.", old, new);
            thread::sleep(Duration::from_millis(500));
            println!("[Callback] Signal emission finished.");
        }))
        .unwrap();

    println!("\n[Main] Calling set_and_spawn()...");
    let handle = signaled.set_and_spawn(1).unwrap();
    println!("[Main] Main thread continues to run while the signal callback executes.");

    handle.join().unwrap().unwrap();
    println!("[Main] Spawned thread finished.");
}
