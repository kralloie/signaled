use signaled::{Signal, Signaled, signal};

fn main() {
    println!("[Main] Creating a new Signaled value with an initial value of 0.");
    let signaled = Signaled::new(0);

    println!("[Main] Adding a signal that only fires when the new value is greater than 10.");
    let conditional_signal = signal!(|old, new| {
        println!(
            "[Callback] Value is now {} which is greater than 10 (was {})!",
            new, old
        );
    });
    conditional_signal.set_trigger(|_, new| *new > 10).unwrap();
    signaled.add_signal(conditional_signal).unwrap();

    println!("\n[Main] Calling set(5). The signal should NOT fire.");
    signaled.set(5).unwrap();

    println!("\n[Main] Calling set(15). The signal SHOULD fire.");
    signaled.set(15).unwrap();
}
