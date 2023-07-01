use std::{
    collections::HashMap,
    io::{ErrorKind, Read, Write},
    str::from_utf8,
    sync::mpsc::{channel, Receiver, Sender},
    thread,
};

use log::trace;
use mio::{
    event::Event as MioEvent,
    net::{TcpListener, TcpStream},
    Events, Interest, Poll, Token, Waker,
};

use crate::consensus::Event as ConsEvent;

const SERVER: Token = Token(0);
const WAKER: Token = Token(1);

pub(crate) enum Event {
    Broadcast { buf: Vec<u8> },
    Read { token: Token },
    NewStream { token: Token, stream: TcpStream },
    DropStream { token: Token },
    Stop, // do not use outside stop function
}

pub(crate) struct Server {
    evt_r: Receiver<Event>,
}

impl Server {
    pub(crate) fn new() -> (Self, Sender<Event>) {
        let (evt_s, evt_r) = channel::<Event>();
        (Self { evt_r }, evt_s)
    }

    fn handle_events(
        &self,
        evt_s: Sender<Event>,
        cons_evt_s: Sender<ConsEvent>,
        stop_s: Sender<()>,
    ) {
        let mut connections: HashMap<Token, TcpStream> = HashMap::new();
        for evt in self.evt_r.iter() {
            let is_stop = self.handle_event(&evt_s, evt, &mut connections, &cons_evt_s, &stop_s);
            if is_stop {
                trace!("stop events handler loop");
                return;
            }
        }
    }

    fn handle_event(
        &self,
        evt_s: &Sender<Event>,
        evt: Event,
        connections: &mut HashMap<Token, TcpStream>,
        cons_evt_s: &Sender<ConsEvent>,
        stop_s: &Sender<()>,
    ) -> bool {
        let mut is_stop = false;
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
            Event::Read { token } => {
                let stream = connections.get_mut(&token).unwrap();
                let addr = stream.peer_addr().unwrap();
                let buf = read(evt_s, stream, token);
                if let Some(buf) = buf {
                    trace!("received new data from {}: {}", addr, from_utf8(&buf).unwrap());
                    let (s, r) = channel::<Vec<u8>>();
                    cons_evt_s.send(ConsEvent::Req { buf, res: s }).unwrap();
                    let mut buf = r.recv().unwrap();
                    if !buf.is_empty() {
                        write(stream, &mut buf);
                    }
                }
            }
            Event::NewStream { token, stream } => {
                let addr = stream.peer_addr().unwrap();
                connections.insert(token, stream);
                trace!("client connected: {}", addr);
            }
            Event::DropStream { token } => {
                let stream = connections.get(&token).unwrap();
                let addr = stream.peer_addr().unwrap();
                connections.remove(&token).unwrap();
                trace!("client disconnected: {}", addr);
            }
            Event::Stop => {
                trace!("received event to stop handling events");
                stop_s.send(()).unwrap();
                is_stop = true;
            }
        }
        is_stop
    }
}

pub(crate) fn spawn(
    srv: Server,
    evt_s: Sender<Event>,
    cons_evt_s: Sender<ConsEvent>,
) -> impl FnOnce() {
    let poll = Poll::new().unwrap();
    let waker = Waker::new(poll.registry(), WAKER).unwrap();

    let (stop_s, stop_r) = channel::<()>();

    let evt_s_ = evt_s.clone();
    let stop_s_ = stop_s.clone();
    thread::spawn(move || srv.handle_events(evt_s_, cons_evt_s, stop_s_));

    let evt_s_ = evt_s.clone();
    thread::spawn(move || handle_epoll_events(evt_s_, poll, stop_s));

    move || {
        trace!("stop server closure was executed");
        waker.wake().unwrap();
        stop_r.recv().unwrap(); // wait for epoll handler
        evt_s.send(Event::Stop).unwrap();
        stop_r.recv().unwrap(); // wait for events handler
        trace!("received signal that server was dropped");
    }
}

fn handle_epoll_events(evt_s: Sender<Event>, mut poll: Poll, stop_s: Sender<()>) {
    let mut events = Events::with_capacity(128);

    let addr: std::net::SocketAddr = "127.0.0.1:12400".parse().unwrap();
    let mut tcp_srv = TcpListener::bind(addr).unwrap();
    poll.registry().register(&mut tcp_srv, SERVER, Interest::READABLE).unwrap();

    // We start from 2 to avoid conflict with predefined tokens.
    let mut next_token = 2;

    loop {
        poll.poll(&mut events, None).unwrap();

        for event in events.iter() {
            trace!("new epoll event: {:?}", event);

            match event.token() {
                SERVER => handle_server_event(&evt_s, &poll, &tcp_srv, &mut next_token),
                WAKER => {
                    trace!("received event to drop epoll loop");
                    stop_s.send(()).unwrap();
                    return;
                }
                token => handle_client_event(&evt_s, event, token),
            }
        }
    }
}

fn handle_server_event(
    evt_s: &Sender<Event>,
    poll: &Poll,
    tcp_srv: &TcpListener,
    next_token: &mut usize,
) {
    let (mut stream, _) = tcp_srv.accept().unwrap();

    let token = Token(*next_token);
    *next_token += 1;
    poll.registry().register(&mut stream, token, Interest::READABLE).unwrap();

    evt_s.send(Event::NewStream { token, stream }).unwrap();
}

fn handle_client_event(evt_s: &Sender<Event>, event: &MioEvent, token: Token) {
    if event.is_readable() {
        evt_s.send(Event::Read { token }).unwrap();
    }
}

fn write(stream: &mut TcpStream, buf: &mut Vec<u8>) {
    // Add new line if not exists.
    if buf.last() != Some(&10) {
        buf.push(10);
    }
    stream.write_all(buf).unwrap();
}

fn read(evt_s: &Sender<Event>, stream: &mut TcpStream, token: Token) -> Option<Vec<u8>> {
    let mut buf: Vec<u8> = vec![0; 1024];
    let n = stream.read(&mut buf);
    match n {
        Ok(0) => {
            evt_s.send(Event::DropStream { token }).unwrap();
            None
        }
        Ok(n) => {
            buf.truncate(n);
            // Remove new line if exists.
            if buf.last() == Some(&10) {
                buf.pop();
            }
            Some(buf)
        }
        Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
            // This error means that there are no data in socket buffer but it is not closed.
            None
        }
        Err(e) => {
            let addr = stream.peer_addr().unwrap();
            panic!("failed to read from connection: {:?}: {:?}", addr, e)
        }
    }
}
