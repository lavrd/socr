use log::{debug, info};
use signal_hook::{consts::TERM_SIGNALS, iterator::Signals};

mod consensus;
mod runner;
mod scheduler;
mod server;

fn main() {
    env_logger::init();
    info!("starting up");

    let (scheduler, scheduler_evt_s, stop_scheduler) = scheduler::Scheduler::new();
    scheduler::spawn(scheduler);

    let (srv, stop_srv) = server::Server::new(scheduler_evt_s);
    server::spawn(srv);

    let mut sigs = Signals::new(TERM_SIGNALS).unwrap();
    let sig = sigs.into_iter().next().unwrap();
    debug!("received termination signal: {:?}", sig);

    stop_srv();
    stop_scheduler();

    info!("stopped");
}
