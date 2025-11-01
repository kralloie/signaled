use signaled::{Signal, Signaled, signal};

fn main() {
    println!("[Main] Creating a new Signaled value with an initial value of 0.");
    let signaled = Signaled::new(0);

    println!("[Main] Adding a signal with an initial behavior.");
    let signal_id = signaled
        .add_signal(signal!(|_, new| {
            println!("[Callback] Initial behavior: Value is now {}.", new);
        }))
        .unwrap();

    println!("\n[Main] Calling set(1) to trigger initial signal behavior...");
    signaled.set(1).unwrap();

    println!("\n[Main] Changing the signal's callback to a new behavior...");
    signaled
        .set_signal_callback(signal_id, |_, new| {
            println!(
                "[Callback] New behavior: Value has been updated to {}.",
                new
            );
        })
        .unwrap();

    println!("\n[Main] Calling set(2) to trigger the new signal behavior...");
    signaled.set(2).unwrap();

    println!("\n[Main] Adding a trigger to only fire when value is greater than 10...");
    signaled
        .set_signal_trigger(signal_id, |_, new| *new > 10)
        .unwrap();

    println!("\n[Main] Calling set(5). The signal should NOT fire.");
    signaled.set(5).unwrap();

    println!("\n[Main] Calling set(15). The signal SHOULD fire.");
    signaled.set(15).unwrap();
}
