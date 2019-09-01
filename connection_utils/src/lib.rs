extern crate tokio;
extern crate tokio_io;
#[macro_use]
extern crate futures;
extern crate bytes;
extern crate ascii;
extern crate ansi_escapes;
extern crate console;

use bytes::{BufMut, Bytes, BytesMut}; 
use tokio::io;
use tokio::io::AsyncRead;
use tokio::net::TcpStream;
use tokio::prelude::*;
use std::string::String;
use std::sync::{Arc, Mutex};

pub type Sender = futures::sync::mpsc::Sender<Bytes>;
pub type Receiver = futures::sync::mpsc::Receiver<Bytes>;
pub const SERVER_PORT: u16 = 49494;
pub const CHANNEL_BUFF_SIZE: usize = 1024usize;
pub const LINES_PER_TICK: usize = 10;

pub fn bytes_to_str(buff : &bytes::Bytes) -> String {
    (*String::from_utf8_lossy(&buff[..])).to_string()
}

////////////////////

pub struct ConsoleBuf {
    read_bytes: String
}

impl ConsoleBuf {
    pub fn new() -> Self {
        print!("{}{}", ansi_escapes::EraseScreen, ansi_escapes::CursorDown(256));
        ConsoleBuf { read_bytes: String::new() }
    }

    pub fn cprint(&self, msg: String){
        print!("{}\r{}\n{}\r{}", ansi_escapes::EraseLine, msg, ansi_escapes::EraseLine, self.read_bytes);
        let _ = std::io::stdout().flush();
    }

    pub fn handle_input(&mut self, ch: char) -> Option<String>
    {
        if ch == ascii::AsciiChar::LineFeed {
            return Some(std::mem::replace(&mut self.read_bytes, String::new()));
        }
        if ch == ascii::AsciiChar::BackSpace.as_char() {
            self.read_bytes.pop();
            print!("{}\r{}", ansi_escapes::EraseLine, self.read_bytes);
        } else {
            print!("{}", ch);
            self.read_bytes.push(ch);
        }
        None
    }
}

///////////////////

pub struct LinesTcp {
    pub socket: TcpStream,
    rd: bytes::BytesMut,
    wr: bytes::BytesMut,
}

impl LinesTcp {
    pub fn new(socket: TcpStream) -> Self {
        LinesTcp {
            socket,
            rd: bytes::BytesMut::new(),
            wr: bytes::BytesMut::new(),
        }
    }

    pub fn buffer(&mut self, line: &[u8]) {
        self.wr.reserve(line.len());
        self.wr.put(line);
    }

    pub fn poll_flush(&mut self) -> Poll<(), tokio::io::Error> {
        while !self.wr.is_empty() {
            let n = try_ready!(self.socket.poll_write(&self.wr));
            assert!(n > 0); 
            let _ = self.wr.split_to(n);
        }

        Ok(Async::Ready(()))
    }

    fn fill_read_buf(&mut self) -> Poll<(), io::Error> {
        loop {
            self.rd.reserve(1024);
            let n = try_ready!(self.socket.read_buf(&mut self.rd));
            if n == 0 {
                return Ok(Async::Ready(()));
            }
        }
    }
}

impl Stream for LinesTcp {
    type Item = bytes::BytesMut;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let sock_closed = self.fill_read_buf()?.is_ready();
        let pos = self.rd.windows(2).enumerate()
            .find(|&(_, bytes)| bytes == b"\r\n")
            .map(|(i, _)| i);

        if let Some(pos) = pos {
            let mut line = self.rd.split_to(pos + 2);
            line.split_off(pos);
            return Ok(Async::Ready(Some(line)));
        }

        if sock_closed {
            Ok(Async::Ready(None))
        } else {
            Ok(Async::NotReady)
        }
    }
}

//////////////////

pub struct InputReader{
    consolebuf: Arc<Mutex<ConsoleBuf>>,
    terminal: console::Term
} 

impl InputReader {
    pub fn new(consolebuf: Arc<Mutex<ConsoleBuf>>) -> Self {
        InputReader { 
            consolebuf,
            terminal: console::Term::stdout()
        }
    }
}

impl Stream for InputReader {
    type Item = String;
    type Error = io::Error;
    fn poll(&mut self) -> Poll<Option<String>, io::Error> {
        let _ = io::stdout().flush();
        let ch = self.terminal.read_char()?;
        task::current().notify();
        if let Some(line) = self.consolebuf.lock().unwrap().handle_input(ch) {
            return Ok(Async::Ready(Some(line)));
        }
        Ok(Async::NotReady)
    }
}

/////////////////////////////////////////////////////////

pub fn parse_send_file(msg : &String) -> Option<String> {
    let start_pattern = ":send \"";
    if !msg.starts_with(start_pattern) {
        return None;
    } 
    if !msg.ends_with('\"') {
        return None;
    } 
    if !(msg.len() > 8) {
        return None;
    }
    let mut result = msg.replace(start_pattern, "");
    result.pop();
    Some(result)
}

pub fn parse_accept_file(msg : &String) -> Option<(String, String)> {
    let start_pattern = ":send \"";
    if msg.starts_with(start_pattern) && msg.len() > 8 {
        let temp = msg.replace(start_pattern, "");
        let v: Vec<&str> = temp.split("\" ").collect();
        if 2 == v.len() {
            return Some((v[0].to_string(), v[1].to_string()));
        }
    }
    None
}

///////////////////////////////////////////////////////////////////

pub fn pass_line(sender: &mut Sender, line : String) -> Result<(), futures::sync::mpsc::TrySendError<Bytes>>{
    let mut outmsg = BytesMut::from(line);
    outmsg.extend_from_slice(b"\r\n");
    sender.try_send(outmsg.freeze())
}


//////////////////////////////////////////////////////////////////

pub type SafeConsole = Arc<Mutex<ConsoleBuf>>;
pub type HandleReceivedFn = dyn Fn(&TextConnection, String)->() + Send;
pub fn print(console: &SafeConsole, line : String) {
    console.lock().unwrap().cprint(line);
}

//////////////////////////////////////////////////////////////////

pub struct TextConnection {
    pub lines: LinesTcp,
    pub receiver: Receiver,
    pub console: SafeConsole,
    pub callback: Box<HandleReceivedFn>,
}

impl Future for TextConnection {
    type Item = ();
    type Error = io::Error;

    fn poll(&mut self) -> Poll<(), io::Error> {
        for i in 0..LINES_PER_TICK {
            match self.receiver.poll().unwrap() {
                Async::Ready(Some(v)) => {
                    self.lines.buffer(&v);
                    if i + 1 == LINES_PER_TICK {
                        task::current().notify();
                    }
                }
                _ => break,
            }
        }

        let _ = self.lines.poll_flush()?;
        loop {
            match self.lines.poll() {
                Ok(Async::Ready(Some(message))) => {
                    let message = message.freeze();
                    let message_str = bytes_to_str(&message);
                    (self.callback)(&self, message_str);
                },
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Ok(Async::Ready(None)) | Err(_) => { 
                    print(&self.console, format!(">>> {:?} DISCONNECTED", self.lines.socket.peer_addr().unwrap()));
                    return Ok(Async::Ready(()));
                }
            }
        }
    }
}

impl TextConnection {
    pub fn new(console: SafeConsole, receiver: Receiver, socket: TcpStream, callback: Box<HandleReceivedFn>) -> TextConnection {
        TextConnection {
            lines: LinesTcp::new(socket),
            receiver,
            console,
            callback
        }
    }
}

//////////////////////////////////////////////////////////////////