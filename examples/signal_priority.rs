use signaled::{Signal, Signaled, signal};

fn main() {
    println!("[Main] Creating a new Signaled value with an initial value of 0.");
    let signaled = Signaled::new(0);

    println!("[Main] Adding a low-priority signal (priority 1).");
    let low_priority_signal = signal!(|_, _| {
        println!("[Callback] Low priority signal fired.");
    });
    low_priority_signal.set_priority(1);
    signaled.add_signal(low_priority_signal).unwrap();

    println!("[Main] Adding a high-priority signal (priority 10).");
    let high_priority_signal = signal!(|_, _| {
        println!("[Callback] High priority signal fired.");
    });
    high_priority_signal.set_priority(10);
    signaled.add_signal(high_priority_signal).unwrap();

    println!("\n[Main] Calling set(1). The high-priority signal should fire first.");
    signaled.set(1).unwrap();
}
