use signaled::{Signal, Signaled, signal};
use std::cell::RefCell;
use std::rc::Rc;

fn main() {
    println!("[Main] Creating a new Signaled value with an initial value of 0.");
    let signaled = Signaled::new(0);

    let tracker = Rc::new(RefCell::new(Vec::<&'static str>::new()));

    println!("[Main] Adding Signal A.");
    let tracker_a = Rc::clone(&tracker);
    let signal_a = signal!(move |_, _| {
        tracker_a.borrow_mut().push("A");
    });
    let signal_a_id = signaled.add_signal(signal_a).unwrap();

    println!("[Main] Adding Signal B.");
    let tracker_b = Rc::clone(&tracker);
    let signal_b = signal!(move |_, _| {
        tracker_b.borrow_mut().push("B");
    });
    let signal_b_id = signaled.add_signal(signal_b).unwrap();

    println!("\n[Main] Calling set(1). Both signals should fire independently.");
    signaled.set(1).unwrap();
    println!("[Main] Tracker after set: {:?}", tracker.borrow());
    assert_eq!(*tracker.borrow(), vec!["A", "B"]);

    tracker.borrow_mut().clear();

    println!("\n[Main] Combining Signal A and Signal B...");
    let combined_id = signaled
        .combine_signals(&[signal_a_id, signal_b_id])
        .unwrap();
    println!("[Main] New combined signal created (ID: {}).", combined_id);

    println!("\n[Main] Calling set(2). The new combined signal should fire.");
    signaled.set(2).unwrap();
    println!("[Main] Tracker after set: {:?}", tracker.borrow());
    assert_eq!(*tracker.borrow(), vec!["A", "B"]);
}
