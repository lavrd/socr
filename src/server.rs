use std::{
    collections::HashMap,
    io::{ErrorKind, Read, Write},
    str::from_utf8,
    sync::mpsc::{channel, Sender},
    thread,
};

use log::{debug, trace};
use mio::{
    event::Event,
    net::{TcpListener, TcpStream},
    Events, Interest, Poll, Token, Waker,
};

const SERVER: Token = Token(0);
const WAKER: Token = Token(1);

pub(crate) struct Server {
    poll: Poll,
    stop_s: Sender<()>,
}

impl Server {
    pub(crate) fn new() -> (Self, impl Fn()) {
        let poll = Poll::new().unwrap();
        let waker = Waker::new(poll.registry(), WAKER).unwrap();
        let (stop_s, stop_r) = channel::<()>();
        (Self { poll, stop_s }, move || {
            trace!("stop server closure was executed");
            waker.wake().unwrap();
            stop_r.recv().unwrap();
            trace!("received signal that server was dropped");
        })
    }
}

pub(crate) fn spawn(srv: Server) {
    thread::spawn(move || run(srv));
}

fn run(mut srv: Server) {
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
            trace!("new event: {:?}", event);

            match event.token() {
                SERVER => {
                    handle_server_event(&mut srv, &tcp_srv, &mut connections, &mut next_token)
                }
                WAKER => {
                    trace!("received event to drop epoll loop");
                    return;
                }
                token => handle_client_event(event, &token, &mut connections),
            }
        }
    }
}

fn handle_server_event(
    srv: &mut Server,
    tcp_srv: &TcpListener,
    connections: &mut HashMap<Token, TcpStream>,
    next_token: &mut usize,
) {
    let (mut socket, addr) = tcp_srv.accept().unwrap();
    debug!("new client: {:?}", addr);

    let token = Token(*next_token);
    *next_token += 1;
    srv.poll.registry().register(&mut socket, token, Interest::READABLE).unwrap();

    connections.insert(token, socket);
}

fn handle_client_event(event: &Event, token: &Token, connections: &mut HashMap<Token, TcpStream>) {
    let conn = connections.get_mut(token).unwrap();
    let addr = conn.peer_addr();
    if event.is_readable() {
        let mut buf: Vec<u8> = vec![0; 1024];
        let n = conn.read(&mut buf);
        match n {
            Ok(0) => {
                debug!("connection closed: {:?}", addr);
                connections.remove(token);
            }
            Ok(n) => {
                debug!("received data: {:?}", from_utf8(&buf[..n]).unwrap());
                conn.write_all(&buf[0..n]).unwrap();
            }
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                // This error means that in socket buffer there are no data but it is not closed.
            }
            Err(e) => {
                panic!("failed to read from connection: {:?}: {:?}", addr, e)
            }
        }
    }
    if event.is_writable() {
        unimplemented!()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        trace!("send signal that server was dropped");
        self.stop_s.send(()).unwrap();
        trace!("sent signal that server was dropped");
    }
}
