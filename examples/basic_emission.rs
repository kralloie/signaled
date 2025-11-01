use signaled::{Signal, Signaled, signal};

fn main() {
    println!("[Main] Creating a new Signaled value with an initial value of 0.");
    let signaled = Signaled::new(0);

    println!("[Main] Adding a signal that prints the old and new values.");
    signaled
        .add_signal(signal!(|old, new| {
            println!("[Callback] Value changed from {} to {}.", old, new);
        }))
        .unwrap();

    println!("\n[Main] Calling set(10). This will trigger the signal.");
    signaled.set(10).unwrap();

    println!("\n[Main] Calling set(20). This will trigger the signal again.");
    signaled.set(20).unwrap();
}
