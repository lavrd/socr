use log::{debug, info};
use signal_hook::{consts::TERM_SIGNALS, iterator::Signals};

mod consensus;
mod server;

fn main() {
    env_logger::init();
    info!("starting up");

    let (srv, srv_evr_s) = server::Server::new();
    let (cons, cons_evt_s) = consensus::Consensus::new();

    let stop_cons = consensus::spawn(cons, cons_evt_s.clone(), srv_evr_s.clone());
    let stop_srv = server::spawn(srv, srv_evr_s, cons_evt_s);

    let mut sigs = Signals::new(TERM_SIGNALS).unwrap();
    let sig = sigs.into_iter().next().unwrap();
    debug!("received termination signal: {:?}", sig);

    stop_srv();
    stop_cons();

    info!("stopped");
}
