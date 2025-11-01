use signaled::{Signal, Signaled, signal};
use std::cell::Cell;
use std::rc::Rc;

fn main() {
    println!("[Main] Creating a new Signaled value with an initial value of 0.");
    let calls = Rc::new(Cell::new(0));
    let signaled = Signaled::new(0);

    println!("[Main] Adding a signal that counts how many times it has fired.");
    let calls_clone = Rc::clone(&calls);
    let signal = signal!(move |_, _| calls_clone.set(calls_clone.get() + 1));
    let signal_id = signaled.add_signal(signal).unwrap();

    println!("\n[Main] Calling set(1). The signal is unmuted.");
    signaled.set(1).unwrap();
    println!("[Main] Call count: {}", calls.get());
    assert_eq!(calls.get(), 1);

    println!("\n[Main] Muting the signal...");
    signaled.set_signal_mute(signal_id, true).unwrap();

    println!("[Main] Calling set(2). The signal is muted and should not fire.");
    signaled.set(2).unwrap();
    println!("[Main] Call count: {}", calls.get());
    assert_eq!(calls.get(), 1);

    println!("\n[Main] Un-muting the signal...");
    signaled.set_signal_mute(signal_id, false).unwrap();

    println!("[Main] Calling set(3). The signal is unmuted and should fire again.");
    signaled.set(3).unwrap();
    println!("[Main] Call count: {}", calls.get());
    assert_eq!(calls.get(), 2);
}
