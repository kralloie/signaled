use signaled::{
    signal_sync,
    sync::{Signal, Signaled},
};
use std::sync::{Arc, Mutex};
use std::thread;

fn main() {
    println!("[Main] Creating a new thread-safe Signaled value.");
    let signaled = Arc::new(Mutex::new(Signaled::new(0)));

    let calls = Arc::new(Mutex::new(0));
    let calls_clone = Arc::clone(&calls);
    signaled
        .lock()
        .unwrap()
        .add_signal(signal_sync!(move |_, _| {
            let mut lock = calls_clone.lock().unwrap();
            *lock += 1;
        }))
        .unwrap();
    println!("[Main] Added a signal to count the number of calls.");

    let mut handles = Vec::new();

    println!("[Main] Spawning 10 threads, each calling set() 10 times.");
    for i in 0..10 {
        let signaled_clone = Arc::clone(&signaled);
        let handle = thread::spawn(move || {
            let lock = signaled_clone.lock().unwrap();
            for j in 0..10 {
                lock.set(j).unwrap();
            }
            println!("[Thread {}] Finished.", i);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }
    println!("[Main] All threads have finished.");

    println!("\n[Main] Total calls: {}", *calls.lock().unwrap());
    assert_eq!(*calls.lock().unwrap(), 100);
}
