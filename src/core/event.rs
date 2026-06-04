use std::any::Any;
use std::fmt::Debug;
use std::sync::Arc;

/// Type alias for event callbacks to reduce type complexity.
pub type EventCallback = Box<dyn Fn(&dyn Event) + Send + Sync>;

pub trait Event: Send + Sync + Debug {
    fn event_type(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
    fn clone_box(&self) -> Box<dyn Event>;
}

impl Clone for Box<dyn Event> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[derive(Debug, Clone)]
pub enum InstanceEvent {
    Start {
        instance_id: String,
    },
    Stop {
        instance_id: String,
        exit_code: i32,
    },
    Error {
        instance_id: String,
        error: String,
    },
    Output {
        instance_id: String,
        data: Vec<u8>,
    },
    StateChange {
        instance_id: String,
        from: crate::core::state::InstanceState,
        to: crate::core::state::InstanceState,
    },
}

impl Event for InstanceEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Start { .. } => "instance:start",
            Self::Stop { .. } => "instance:stop",
            Self::Error { .. } => "instance:error",
            Self::Output { .. } => "instance:output",
            Self::StateChange { .. } => "instance:state_change",
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Event> {
        Box::new(self.clone())
    }
}

pub trait EventBus: Send + Sync {
    fn emit(&self, event: Box<dyn Event>);
    fn subscribe(&self, event_type: &'static str, callback: EventCallback);
    fn subscribe_all(&self, callback: EventCallback);
    /// Returns a clone of this event bus as a boxed trait object.
    fn clone_boxed(&self) -> Box<dyn EventBus>;
}

pub struct TokioBroadcastBus {
    subscribers: parking_lot::RwLock<Vec<Subscriber>>,
}

impl std::fmt::Debug for TokioBroadcastBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokioBroadcastBus")
            .field("subscribers", &self.subscribers.read().len())
            .finish()
    }
}

#[derive(Clone)]
struct Subscriber {
    event_type: Option<&'static str>,
    callback: Arc<EventCallback>,
}

impl std::fmt::Debug for Subscriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscriber")
            .field("event_type", &self.event_type)
            .finish_non_exhaustive()
    }
}

impl TokioBroadcastBus {
    /// Creates a new `TokioBroadcastBus`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            subscribers: parking_lot::RwLock::new(Vec::new()),
        }
    }
}

impl Default for TokioBroadcastBus {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for TokioBroadcastBus {
    fn clone(&self) -> Self {
        let subs = self.subscribers.read();
        Self {
            subscribers: parking_lot::RwLock::new(subs.clone()),
        }
    }
}

impl EventBus for TokioBroadcastBus {
    fn emit(&self, event: Box<dyn Event>) {
        let subs = self.subscribers.read();
        for sub in subs.iter() {
            if sub.event_type.is_none() || sub.event_type == Some(event.event_type()) {
                (sub.callback)(&*event);
            }
        }
    }

    fn subscribe(&self, event_type: &'static str, callback: EventCallback) {
        self.subscribers.write().push(Subscriber {
            event_type: Some(event_type),
            callback: Arc::new(callback),
        });
    }

    fn subscribe_all(&self, callback: EventCallback) {
        self.subscribers.write().push(Subscriber {
            event_type: None,
            callback: Arc::new(callback),
        });
    }

    fn clone_boxed(&self) -> Box<dyn EventBus> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_event_bus_emit_and_subscribe() {
        let bus = TokioBroadcastBus::new();
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        bus.subscribe(
            "instance:start",
            Box::new(move |evt| {
                if evt.as_any().downcast_ref::<InstanceEvent>().is_some() {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                }
            }),
        );

        bus.emit(Box::new(InstanceEvent::Start {
            instance_id: "test".into(),
        }));

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_event_bus_wildcard_subscribe_all() {
        let bus = TokioBroadcastBus::new();
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        bus.subscribe_all(Box::new(move |_evt| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        }));

        bus.emit(Box::new(InstanceEvent::Start {
            instance_id: "a".into(),
        }));
        bus.emit(Box::new(InstanceEvent::Stop {
            instance_id: "a".into(),
            exit_code: 0,
        }));
        bus.emit(Box::new(InstanceEvent::Error {
            instance_id: "a".into(),
            error: "e".into(),
        }));

        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_event_bus_type_filtering() {
        let bus = TokioBroadcastBus::new();
        let start_counter = Arc::new(AtomicUsize::new(0));
        let stop_counter = Arc::new(AtomicUsize::new(0));
        let sc = start_counter.clone();
        let stc = stop_counter.clone();

        bus.subscribe(
            "instance:start",
            Box::new(move |_evt| {
                sc.fetch_add(1, Ordering::SeqCst);
            }),
        );
        bus.subscribe(
            "instance:stop",
            Box::new(move |_evt| {
                stc.fetch_add(1, Ordering::SeqCst);
            }),
        );

        bus.emit(Box::new(InstanceEvent::Start {
            instance_id: "x".into(),
        }));
        bus.emit(Box::new(InstanceEvent::Stop {
            instance_id: "x".into(),
            exit_code: 0,
        }));

        assert_eq!(start_counter.load(Ordering::SeqCst), 1);
        assert_eq!(stop_counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_instance_event_types() {
        let e1 = InstanceEvent::Start {
            instance_id: "i".into(),
        };
        assert_eq!(e1.event_type(), "instance:start");

        let e2 = InstanceEvent::Stop {
            instance_id: "i".into(),
            exit_code: 0,
        };
        assert_eq!(e2.event_type(), "instance:stop");

        let e3 = InstanceEvent::Error {
            instance_id: "i".into(),
            error: "e".into(),
        };
        assert_eq!(e3.event_type(), "instance:error");

        let e4 = InstanceEvent::Output {
            instance_id: "i".into(),
            data: vec![1, 2],
        };
        assert_eq!(e4.event_type(), "instance:output");

        let e5 = InstanceEvent::StateChange {
            instance_id: "i".into(),
            from: crate::core::state::InstanceState::Stopped,
            to: crate::core::state::InstanceState::Starting,
        };
        assert_eq!(e5.event_type(), "instance:state_change");
    }

    #[test]
    fn test_event_clone_box() {
        let e = InstanceEvent::Start {
            instance_id: "i".into(),
        };
        let cloned: Box<dyn Event> = e.clone_box();
        assert_eq!(cloned.event_type(), "instance:start");
    }
}
