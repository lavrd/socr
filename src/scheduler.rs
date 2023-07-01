use std::{
    str::from_utf8,
    sync::mpsc::{channel, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use log::{debug, trace};

const TICK_INTERVAL: Duration = Duration::from_secs(3);

pub(crate) enum Event {
    Rpc(Vec<u8>),
    Stop,
}

pub(crate) struct Scheduler {
    stop_s: Sender<()>,
    evt_r: Receiver<Event>,
    last_tick: Instant,
}

impl Scheduler {
    pub(crate) fn new() -> (Self, Sender<Event>, impl FnOnce()) {
        let (stop_s, stop_r) = channel::<()>();
        let (evt_s, evt_r) = channel::<Event>();
        (
            Self {
                stop_s,
                evt_r,
                last_tick: Instant::now() - TICK_INTERVAL,
            },
            evt_s.clone(),
            move || {
                trace!("stop scheduler closure was executed");
                evt_s.send(Event::Stop).unwrap();
                stop_r.recv().unwrap();
                trace!("received signal that scheduler was dropped");
            },
        )
    }

    fn tick(&mut self) {
        if self.last_tick.elapsed() < TICK_INTERVAL {
            return;
        }
        trace!("tick");
        self.last_tick = Instant::now();
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        trace!("send that scheduler was dropped");
        self.stop_s.send(()).unwrap();
        trace!("sent that scheduler was dropped");
    }
}

pub(crate) fn spawn(scheduler: Scheduler) {
    thread::spawn(move || run(scheduler));
}

fn run(mut scheduler: Scheduler) {
    scheduler.tick();
    loop {
        let evt = scheduler.evt_r.recv_timeout(TICK_INTERVAL);
        scheduler.tick();
        if evt.is_err() {
            continue;
        }
        let evt = evt.unwrap();
        match evt {
            Event::Rpc(buf) => {
                debug!("new rpc request: {:?}", from_utf8(&buf).unwrap());
            }
            Event::Stop => {
                trace!("received stop event");
                return;
            }
        }
    }
}
