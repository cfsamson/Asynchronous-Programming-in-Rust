use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
};

use mio::{net::TcpStream, Events, Interest, Poll, Registry, Token};

use super::Waker;

static REACTOR: OnceLock<Reactor> = OnceLock::new();

pub fn reactor() -> &'static Reactor {
    REACTOR.get().expect("Called outside an executor context")
}

type Wakers = Arc<Mutex<HashMap<usize, Waker>>>;
pub struct Reactor {
    wakers: Wakers,
    registry: Registry,
    next_id: AtomicUsize,
}

impl Reactor {
    pub fn register(&self, stream: &mut TcpStream, interest: Interest, waker: Waker, id: usize) {
        let is_new = self
            .wakers
            .lock()
            // Must always store the most recent waker
            .map(|mut w| w.insert(id, waker).is_none())
            .unwrap();

        if is_new {
            self.registry.register(stream, Token(id), interest).unwrap();
        }
    }

    pub fn deregister(&self, stream: &mut TcpStream, id: usize) {
        self.wakers.lock().map(|mut w| w.remove(&id)).unwrap();
        self.registry.deregister(stream).unwrap();
    }

    pub fn next_id(&self) -> usize {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

pub fn start() {
    use thread::spawn;

    let wakers = Arc::new(Mutex::new(HashMap::new()));
    let poll = Poll::new().unwrap();
    let registry = poll.registry().try_clone().unwrap();
    let next_id = AtomicUsize::new(1);
    let reactor = Reactor {
        wakers: wakers.clone(),
        registry,
        next_id,
    };

    REACTOR.set(reactor).ok().unwrap();

    spawn(move || event_loop(poll, wakers));
}

fn event_loop(mut poll: Poll, wakers: Wakers) {
    let mut events = Events::with_capacity(100);
    loop {
        poll.poll(&mut events, None).unwrap();
        for e in events.iter() {
            if e.is_read_closed() || e.is_write_closed() {
                continue;
            }
            let Token(id) = e.token();
            wakers
                .lock()
                .map(|w| {
                    // if we removed it from the list we're done with this resource
                    // and must not call wake!
                    if let Some(waker) = w.get(&id) {
                        waker.wake();
                    }
                })
                .unwrap();
        }
    }
}