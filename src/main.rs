#![allow(dead_code)]
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;
use std::fmt::Display;
use std::sync::atomic::{AtomicU64, Ordering};

////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
#[derive(Debug)]
enum SignaledError {
    BorrowError(String),
    BorrowMutError(String),
    InvalidSignalId(String)
}

type SignalId = u64;

static SIGNAL_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

fn new_signal_id() -> SignalId {
    let id = SIGNAL_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    debug_assert!(id != u64::MAX, "SignalId counter overflow");
    id
}

struct Signal<T> {
    callback: RefCell<Rc<dyn Fn(&T)>>,
    trigger: RefCell<Rc<dyn Fn(&T) -> bool>>,
    id: u64,
    priority: Cell<u64>
}

impl<T> Signal<T> {
    fn new<F: Fn(&T) + 'static, G: Fn(&T) -> bool + 'static>(callback: F, trigger: G, priority: u64) -> Self {
        Signal {
            callback: RefCell::new(Rc::new(callback)),
            trigger: RefCell::new(Rc::new(trigger)),
            id: new_signal_id(),
            priority: Cell::new(priority)
        }
    }

    fn emit(&self, value: &T) -> Result<(), SignaledError>{
        let trigger = self.trigger
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError("Signal `trigger` is already borrowed".to_string()))?;
        if trigger(value) {
            let callback = self.callback
                .try_borrow()
                .map_err(|_| SignaledError::BorrowError("Signal `callback` is already borrowed".to_string()))?;
            callback(value);
        }
        Ok(())
    }

    fn set_callback<F: Fn(&T) + 'static>(&self, callback: F) -> Result<(), SignaledError> {
        *self.callback.try_borrow_mut().map_err(|_| SignaledError::BorrowMutError("Signal `callback` is already borrowed".to_string()))? = Rc::new(callback);
        Ok(())
    }

    fn set_trigger<F: Fn(&T) -> bool + 'static>(&self, trigger: F) -> Result<(), SignaledError> {
        *self.trigger.try_borrow_mut().map_err(|_| SignaledError::BorrowMutError("Signal `trigger` is already borrowed".to_string()))? = Rc::new(trigger);
        Ok(())
    }

    fn set_priority(&self, priority: u64) {
        self.priority.set(priority);
    }
}

impl<T> Display for Signal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Signal {{ id: {}, priority: {} }}", self.id, self.priority.get())
    }
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Signal {
            callback: self.callback.clone(),
            trigger: self.trigger.clone(),
            id: self.id.clone(),
            priority: self.priority.clone()
        }
    }
}
struct Signaled<T> {
    value: RefCell<T>,
    signals: RefCell<Vec<Signal<T>>>
}

impl<T> Signaled<T> {
    fn new(value: T) -> Self {
        Self {
            value: RefCell::new(value),
            signals: RefCell::new(Vec::new())
        }
    }

    fn set(&self, value: T) -> Result<(), SignaledError> {
        *self.value.try_borrow_mut().map_err(|_| SignaledError::BorrowMutError("Signaled `value` is already borrowed".to_string()))? = value;
        self.emit()
    }

    fn get_ref(&self) -> Result<Ref<'_, T>, SignaledError> {
        match self.value.try_borrow() {
            Ok(r) => Ok(r),
            Err(_) => Err(SignaledError::BorrowError("Signaled `value` is already borrowed".to_string())) 
        }
    }

    fn emit(&self) -> Result<(), SignaledError> {
        match self.signals.try_borrow_mut() {
            Ok(mut signals) => {
                signals.sort_by(|a, b| b.priority.get().cmp(&a.priority.get()));
                for signal in signals.iter() {
                    let value = self.value
                        .try_borrow()
                        .map_err(|_| SignaledError::BorrowError("Signaled `value` is already borrowed".to_string()))?;
                    signal.emit(&value)?
                }
                Ok(())
            }
            Err(_) => Err(SignaledError::BorrowError("Signaled `signals` are already borrowed".to_string()))
        }
    }

    fn add_signal(&self, signal: Signal<T>) -> Result<SignalId, SignaledError> {
        match self.signals.try_borrow_mut() {
            Ok(mut s) => {
                let id = signal.id;
                if s.iter().any(|sig| sig.id == id) {
                    return Ok(id);
                }
                s.push(signal);
                Ok(id)
            }
            Err(_) => Err(SignaledError::BorrowMutError("Signaled `signals` are already borrowed".to_string()))
        }
    }

    fn remove_signal(&self, id: SignalId) -> Result<Signal<T>, SignaledError>{
        match self.signals.try_borrow_mut() {
            Ok(mut s) => {
                let index = s
                    .iter()
                    .position(|s| s.id == id)
                    .ok_or_else(|| SignaledError::InvalidSignalId(format!("ID: {} does not match any signal", id)))?;
                Ok(s.remove(index))
            }
            Err(_) => Err(SignaledError::BorrowMutError("Targeted signal is already borrowed".to_string()))
        }
    }
 
    fn set_signal_callback<F: Fn(&T) + 'static>(&self, id: SignalId, callback: F) -> Result<(), SignaledError> {
        let signals = self.signals
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError("Signaled `signals` are already borrowed".to_string()))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.set_callback(callback)
        }
        Ok(())
    }

    fn set_signal_trigger<F: Fn(&T) -> bool + 'static>(&self, id: SignalId, trigger: F) -> Result<(), SignaledError> {
        let signals = self.signals.try_borrow().map_err(|_| SignaledError::BorrowError("Signaled `signals` are already borrowed".to_string()))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.set_trigger(trigger)
        }
        Ok(())
    }

    fn set_signal_priority(&self, id: SignalId, priority: u64) -> Result<(), SignaledError> {
        let signals = self.signals
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError("Signaled `signals` are already borrowed".to_string()))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            signal.set_priority(priority);
        }
        Ok(())
    }

}

impl<T: Display> Display for Signaled<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let signals = self.signals.borrow();
        let value = self.value.borrow();
        let signals_len = signals.len();
        let signals: String = signals
            .iter()
            .map(|s| s.id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        write!(f, "Signaled {{ value: {}, signal_count: {}, signals: [{}] }}", value, signals_len, signals)
    }
}

impl<T: Clone> Clone for Signaled<T> {
    fn clone(&self) -> Self {
        Signaled {
            value: RefCell::new(self.value.borrow().clone()),
            signals: RefCell::new(self.signals.borrow().clone())
        }
    }
}

impl<T: Clone> Signaled<T> {
    fn get(&self) -> Result<T, SignaledError> {
        match self.value.try_borrow() {
            Ok(r) => Ok(r.clone()),
            Err(_) => Err(SignaledError::BorrowError("Signaled is already borrowed".to_string()))
        }
    }

    fn try_clone(&self) -> Result<Self, SignaledError> {
        let signals_clone = self.signals
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError("Signaled `signals` are already borrowed".to_string()))?
            .clone();
        let value_clone = self.value
            .try_borrow()
            .map_err(|_| SignaledError::BorrowError("Signaled `value` is already borrowed".to_string()))?
            .clone();
        Ok(
            Self {
                value: RefCell::new(value_clone),
                signals: RefCell::new(signals_clone)
            }
        )
    }
}

////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
fn main() {
    let signal = Signal::new(|v| println!("Old: {}", v), |_| true, 0);
    let clone = signal.clone();
    signal.set_callback(|v| println!("New: {}", v)).unwrap();
    clone.emit(&1).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_clone() {
        let signal = Signal::new(
            |v: &i32| println!("Signal: {}", v),
            |_| true,
            1
        );
        let cloned = signal.clone();
        signal.emit(&42);
        cloned.emit(&42);
    }

    #[test]
    fn test_signaled_clone() {
        let x = Signaled::new(5);
        x.add_signal(Signal::new(
                |v| println!("X signal: {}", v),
                |_| true,
                1
            )
        );
        let y = x.clone();
        x.set(10);
        y.set(20);
        assert_eq!(*x.value.borrow(), 10);
        assert_eq!(*y.value.borrow(), 20);
        assert_eq!(x.signals.borrow().len(), 1);
        assert_eq!(y.signals.borrow().len(), 1);
    }

    #[test]
    fn test_add_sub() {
        let signaled = Signaled::new(15);
        assert_eq!(*signaled.get_ref().unwrap() - 5, 10);
        assert_eq!(signaled.get().unwrap() + 5, 20);
    }
}