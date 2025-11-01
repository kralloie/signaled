use signaled::{Signal, Signaled, signal};
use std::cell::Cell;
use std::rc::Rc;

fn main() {
    println!("[Main] Creating a new Signaled value with an initial value of 0.");
    let calls = Rc::new(Cell::new(0));
    let signaled = Signaled::new(0);

    println!("[Main] Adding a signal that counts how many times it has fired.");
    let calls_clone = Rc::clone(&calls);
    signaled
        .add_signal(signal!(move |_, _| calls_clone.set(calls_clone.get() + 1)))
        .unwrap();

    println!("\n[Main] Calling set(1). This will trigger the signal.");
    signaled.set(1).unwrap();
    println!(
        "[Main] Value: {}, Call count: {}",
        signaled.get().unwrap(),
        calls.get()
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(signaled.get().unwrap(), 1);

    println!("\n[Main] Calling set_silent(2). This will NOT trigger the signal.");
    signaled.set_silent(2).unwrap();
    println!(
        "[Main] Value: {}, Call count: {}",
        signaled.get().unwrap(),
        calls.get()
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(signaled.get().unwrap(), 2);

    println!("\n[Main] Calling set(3) again. The signal should trigger.");
    signaled.set(3).unwrap();
    println!(
        "[Main] Value: {}, Call count: {}",
        signaled.get().unwrap(),
        calls.get()
    );
    assert_eq!(calls.get(), 2);
    assert_eq!(signaled.get().unwrap(), 3);
}
