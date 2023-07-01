use std::{
    str::from_utf8,
    sync::mpsc::{channel, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use log::{trace, warn};

use crate::server::Event as SrvEvent;

const TICK_INTERVAL: Duration = Duration::from_millis(3_000);

pub(crate) enum Event {
    Req { buf: Vec<u8>, res: Sender<Vec<u8>> },
    Stop,
}

pub(crate) struct Consensus {
    evt_r: Receiver<Event>,
    last_tick: Instant,
}

impl Consensus {
    pub(crate) fn new() -> (Self, Sender<Event>) {
        let (evt_s, evt_r) = channel::<Event>();
        (
            Self {
                evt_r,
                last_tick: Instant::now() - TICK_INTERVAL,
            },
            evt_s,
        )
    }

    fn tick(&mut self, srv_evt_s: &Sender<SrvEvent>) {
        if self.last_tick.elapsed() < TICK_INTERVAL {
            return;
        }
        trace!("tick");
        let res = srv_evt_s.send(SrvEvent::Broadcast {
            buf: b"hello from other node".to_vec(),
        });
        if let Err(e) = res {
            // It can be okay when we shutdown server while tick is executing.
            warn!("failed to send broadcast server event: {}", e);
        }
        self.last_tick = Instant::now();
    }
}

pub(crate) fn spawn(
    cons: Consensus,
    evt_s: Sender<Event>,
    srv_evt_s: Sender<SrvEvent>,
) -> impl FnOnce() {
    let (stop_s, stop_r) = channel::<()>();

    thread::spawn(move || handle_events(cons, srv_evt_s, stop_s));

    move || {
        trace!("consensus stop closure was executed");
        evt_s.send(Event::Stop).unwrap();
        stop_r.recv().unwrap();
        trace!("received signal that consensus was dropped");
    }
}

fn handle_events(mut cons: Consensus, srv_evt_s: Sender<SrvEvent>, stop_s: Sender<()>) {
    cons.tick(&srv_evt_s);
    loop {
        let evt = cons.evt_r.recv_timeout(TICK_INTERVAL);
        cons.tick(&srv_evt_s);
        if evt.is_err() {
            continue;
        }
        let evt = evt.unwrap();
        match evt {
            Event::Req { buf, res } => {
                trace!("received request from server: {}", from_utf8(&buf).unwrap());
                res.send(b"response".to_vec()).unwrap();
            }
            Event::Stop => {
                trace!("received event to stop events handler");
                stop_s.send(()).unwrap();
                return;
            }
        }
    }
}
