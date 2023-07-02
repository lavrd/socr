use std::{
    collections::HashMap,
    io::{ErrorKind, Read, Write},
    str::from_utf8,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    thread,
};

use log::{trace, warn};
use mio::{
    net::{TcpListener, TcpStream},
    Events, Interest, Poll, Token, Waker,
};

use crate::consensus::Event as ConsEvent;

const SERVER: Token = Token(0);
const WAKER: Token = Token(1);

pub(crate) enum Event {
    Broadcast { buf: Vec<u8> },
    Stop, // do not use outside stop function
}

#[derive(Clone)]
pub(crate) struct EventSender {
    evt_s: Sender<Event>,
    waker: Arc<Waker>,
}

impl EventSender {
    pub(crate) fn send(&self, evt: Event) {
        let res = self.evt_s.send(evt);
        if let Err(e) = res {
            // It can be okay when we shutdown server while tick is executing.
            warn!("failed to send broadcast server event: {}", e);
            return;
        }
        self.waker.wake().unwrap();
    }
}

pub(crate) struct Server {
    poll: Poll,
    evt_r: Receiver<Event>,
}

impl Server {
    pub(crate) fn new() -> (Self, EventSender) {
        let poll = Poll::new().unwrap();
        let waker = Waker::new(poll.registry(), WAKER).unwrap();
        let (evt_s, evt_r) = channel::<Event>();
        (
            Self { poll, evt_r },
            EventSender {
                evt_s,
                waker: Arc::new(waker),
            },
        )
    }
}

pub(crate) fn spawn(
    srv: Server,
    evt_s: EventSender,
    cons_evt_s: Sender<ConsEvent>,
) -> impl FnOnce() {
    let (stop_s, stop_r) = channel::<()>();

    thread::spawn(move || handle_epoll_events(srv, cons_evt_s, stop_s));

    move || {
        trace!("stop server closure was executed");
        evt_s.send(Event::Stop);
        stop_r.recv().unwrap();
        trace!("received signal that server was dropped");
    }
}

fn handle_epoll_events(mut srv: Server, cons_evt_s: Sender<ConsEvent>, stop_s: Sender<()>) {
    let mut events = Events::with_capacity(128);

    let addr: std::net::SocketAddr = "127.0.0.1:12400".parse().unwrap();
    let mut tcp_srv = TcpListener::bind(addr).unwrap();
    srv.poll.registry().register(&mut tcp_srv, SERVER, Interest::READABLE).unwrap();

    let mut connections: HashMap<Token, TcpStream> = HashMap::new();
    // We start from 2 to avoid conflict with predefined tokens.
    let mut next_token = 2;

    loop {
        srv.poll.poll(&mut events, None).unwrap();

        for event in events.iter() {
            trace!("new epoll event: {:?}", event);

            match event.token() {
                SERVER => {
                    handle_server_event(&srv.poll, &tcp_srv, &mut connections, &mut next_token)
                }
                WAKER => {
                    let is_stop = handle_waker_event(&srv.evt_r, &mut connections, &stop_s);
                    if is_stop {
                        trace!("stop epoll events handler loop");
                        return;
                    }
                }
                token => {
                    if event.is_readable() {
                        handle_client_event(&cons_evt_s, &mut connections, token);
                    }
                }
            }
        }
    }
}

fn handle_server_event(
    poll: &Poll,
    tcp_srv: &TcpListener,
    connections: &mut HashMap<Token, TcpStream>,
    next_token: &mut usize,
) {
    let (mut stream, addr) = tcp_srv.accept().unwrap();

    let token = Token(*next_token);
    *next_token += 1;
    poll.registry().register(&mut stream, token, Interest::READABLE).unwrap();

    connections.insert(token, stream);
    trace!("client connected: {}", addr);
}

fn handle_waker_event(
    evt_r: &Receiver<Event>,
    connections: &mut HashMap<Token, TcpStream>,
    stop_s: &Sender<()>,
) -> bool {
    while let Ok(evt) = evt_r.try_recv() {
        match evt {
            Event::Broadcast { mut buf } => {
                trace!("starting to broadcast data: \"{}\"", from_utf8(&buf).unwrap());
                let mut count = 0;
                for stream in connections.values_mut() {
                    write(stream, &mut buf);
                    count += 1;
                }
                trace!("broadcast finished; sent {} times", count);
            }
            Event::Stop => {
                trace!("received event to drop epoll loop");
                stop_s.send(()).unwrap();
                return true;
            }
        }
    }
    false
}

fn handle_client_event(
    cons_evt_s: &Sender<ConsEvent>,
    connections: &mut HashMap<Token, TcpStream>,
    token: Token,
) {
    let stream = connections.get_mut(&token).unwrap();
    let addr = stream.peer_addr().unwrap();

    let mut buf: Vec<u8> = vec![0; 1024];
    let n = stream.read(&mut buf);
    let buf = match n {
        Ok(0) => {
            let addr = stream.peer_addr().unwrap();
            connections.remove(&token).unwrap();
            trace!("client disconnected: {}", addr);
            return;
        }
        Ok(n) => {
            buf.truncate(n);
            // Remove new line if exists.
            if buf.last() == Some(&10) {
                buf.pop();
            }
            buf
        }
        Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
            // This error means that there are no data in socket buffer but it is not closed.
            return;
        }
        Err(e) => {
            let addr = stream.peer_addr().unwrap();
            panic!("failed to read from connection: {:?}: {:?}", addr, e)
        }
    };

    trace!("received new data from {}: {}", addr, from_utf8(&buf).unwrap());
    let (s, r) = channel::<Vec<u8>>();
    cons_evt_s.send(ConsEvent::Req { buf, res: s }).unwrap();
    let mut buf = r.recv().unwrap();
    if !buf.is_empty() {
        write(stream, &mut buf);
    }
}

fn write(stream: &mut TcpStream, buf: &mut Vec<u8>) {
    // Add new line if not exists.
    if buf.last() != Some(&10) {
        buf.push(10);
    }
    stream.write_all(buf).unwrap();
}
