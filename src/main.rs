#![allow(dead_code)]
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;
use std::fmt::Display;
use std::sync::atomic::{AtomicU64, Ordering};

////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[derive(Debug)]
enum ErrorSource {
    Value,
    Signals,
    SignalCallback,
    SignalTrigger
}

#[derive(Debug)] 
enum ErrorType {
    BorrowError { source: ErrorSource },
    BorrowMutError { source: ErrorSource },
    InvalidSignalId { id: SignalId }
}

#[derive(Debug)]
struct SignaledError {
    message: String
}

fn signaled_error(err_type: ErrorType) -> SignaledError {
    let err_msg = match err_type {
        ErrorType::BorrowError { source } => {
            match source {
                ErrorSource::SignalCallback => "Cannot borrow Signal callback, it is already borrowed".to_string(),
                ErrorSource::SignalTrigger => "Cannot borrow Signal trigger, it is already borrowed".to_string(),
                ErrorSource::Value => "Cannot borrow Signaled value, it is already borrowed".to_string(),
                ErrorSource::Signals => "Cannot borrow Signaled signals, they are already borrowed".to_string()
            }
        }
        ErrorType::BorrowMutError { source } => {
            match source {
                ErrorSource::SignalCallback => "Cannot mutably borrow Signal callback, it is already borrowed".to_string(),
                ErrorSource::SignalTrigger => "Cannot mutably borrow Signal trigger, it is already borrowed".to_string(),
                ErrorSource::Value => "Cannot mutably borrow Signaled value, it is already borrowed".to_string(),
                ErrorSource::Signals => "Cannot mutably borrow Signaled signals, they are already borrowed".to_string()
            }
        }
        ErrorType::InvalidSignalId { id } => {
            format!("Signal ID '{}' does not match any Signal", id)
        }
    };

    SignaledError { message: err_msg }
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
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::SignalTrigger }))?;
        if trigger(value) {
            let callback = self.callback
                .try_borrow()
                .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::SignalCallback }))?;
            callback(value);
        }
        Ok(())
    }

    fn set_callback<F: Fn(&T) + 'static>(&self, callback: F) -> Result<(), SignaledError> {
        *self.callback.try_borrow_mut().map_err(|_| signaled_error(ErrorType::BorrowMutError { source: ErrorSource::SignalCallback }))? = Rc::new(callback);
        Ok(())
    }

    fn set_trigger<F: Fn(&T) -> bool + 'static>(&self, trigger: F) -> Result<(), SignaledError> {
        *self.trigger.try_borrow_mut().map_err(|_| signaled_error(ErrorType::BorrowMutError { source: ErrorSource::SignalTrigger }))? = Rc::new(trigger);
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
        *self.value.try_borrow_mut().map_err(|_| signaled_error(ErrorType::BorrowMutError { source: ErrorSource::Value }))? = value;
        self.emit()
    }

    fn get_ref(&self) -> Result<Ref<'_, T>, SignaledError> {
        match self.value.try_borrow() {
            Ok(r) => Ok(r),
            Err(_) => Err(signaled_error(ErrorType::BorrowError { source: ErrorSource::Value })) 
        }
    }

    fn emit(&self) -> Result<(), SignaledError> {
        match self.signals.try_borrow_mut() {
            Ok(mut signals) => {
                signals.sort_by(|a, b| b.priority.get().cmp(&a.priority.get()));
                for signal in signals.iter() {
                    let value = self.value
                        .try_borrow()
                        .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Value }))?;
                    signal.emit(&value)?
                }
                Ok(())
            }
            Err(_) => Err(signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))
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
            Err(_) => Err(signaled_error(ErrorType::BorrowMutError { source: ErrorSource::Signals }))
        }
    }

    fn remove_signal(&self, id: SignalId) -> Result<Signal<T>, SignaledError>{
        match self.signals.try_borrow_mut() {
            Ok(mut s) => {
                let index = s
                    .iter()
                    .position(|s| s.id == id)
                    .ok_or_else(|| signaled_error(ErrorType::InvalidSignalId { id: id }))?;
                Ok(s.remove(index))
            }
            Err(_) => Err(signaled_error(ErrorType::BorrowMutError { source: ErrorSource::Signals }))
        }
    }
 
    fn set_signal_callback<F: Fn(&T) + 'static>(&self, id: SignalId, callback: F) -> Result<(), SignaledError> {
        let signals = self.signals
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.set_callback(callback)
        }
        Ok(())
    }

    fn set_signal_trigger<F: Fn(&T) -> bool + 'static>(&self, id: SignalId, trigger: F) -> Result<(), SignaledError> {
        let signals = self.signals.try_borrow().map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))?;
        if let Some(signal) = signals.iter().find(|s| s.id == id) {
            return signal.set_trigger(trigger)
        }
        Ok(())
    }

    fn set_signal_priority(&self, id: SignalId, priority: u64) -> Result<(), SignaledError> {
        let signals = self.signals
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))?;
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
            value: self.value.clone(),
            signals: self.signals.clone()
        }
    }
}

impl<T: Clone> Signaled<T> {
    fn get(&self) -> Result<T, SignaledError> {
        match self.value.try_borrow() {
            Ok(r) => Ok(r.clone()),
            Err(_) => Err(signaled_error(ErrorType::BorrowError { source: ErrorSource::Value }))
        }
    }

    fn try_clone(&self) -> Result<Self, SignaledError> {
        let signals_clone = self.signals
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Signals }))?
            .clone();
        let value_clone = self.value
            .try_borrow()
            .map_err(|_| signaled_error(ErrorType::BorrowError { source: ErrorSource::Value }))?
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

}

#[cfg(test)]
mod tests {
    use super::*;
}