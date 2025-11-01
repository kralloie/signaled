use signaled::{Signal, Signaled, signal};

fn main() {
    println!("[Main] Creating a new Signaled value with an initial value of 0.");
    let signaled = Signaled::new(0);

    println!("[Main] Adding a signal that will only fire once.");
    let once_signal = signal!(|_, new| {
        println!(
            "[Callback] This signal only fires once. The new value is {}.",
            new
        );
    });
    once_signal.set_once(true);
    signaled.add_signal(once_signal).unwrap();

    println!("\n[Main] Calling set(1). The signal should fire.");
    signaled.set(1).unwrap();

    println!("\n[Main] Calling set(2). The signal should NOT fire again.");
    signaled.set(2).unwrap();
}
