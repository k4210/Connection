extern crate tokio;
extern crate futures;
extern crate bytes;
extern crate connection_utils;
extern crate get_if_addrs;

use tokio::net::TcpListener;
use tokio::prelude::*;
use tokio::runtime::Builder;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::string::String;
use std::net::IpAddr;
use std::collections::HashMap;
use connection_utils::SafeConsole;
use connection_utils::print;

struct Shared {
    peers: Mutex<HashMap<SocketAddr, (connection_utils::Sender, Option<String>)>>
}

impl Shared {
    fn new() -> Self {
        Shared {
            peers: Mutex::new(HashMap::new()),
        }
    }
}

fn list_clients(state: & HashMap<SocketAddr, (connection_utils::Sender, Option<String>)>) -> String {
    let mut result = String::new();
    for (_, (_, name_opt)) in state {
        if let Some(name) = name_opt {
            result += " ";
            result += name;
        }
    }
    return result;
}

pub fn list_ip() -> Option<IpAddr>{
    let mut res : Option<IpAddr> = Option::None;
    for iface in get_if_addrs::get_if_addrs().unwrap() {
        if !iface.is_loopback() {
            let ip = iface.ip(); 
            if ip.is_ipv4() {
                println!(">>> Found network adapter: {:?}", ip);
                if let None = res
                {
                    res = Some(ip);
                }
            }
        }
    }
    return res;
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    let console :SafeConsole = Arc::new(Mutex::new(connection_utils::ConsoleBuf::new()));
    let version = env!("CARGO_PKG_VERSION");
    print(&console, format!(">>> Connection version: {}", version));
    let my_ip: IpAddr = list_ip().expect("a valid ip");
    let listen_addr = SocketAddr::new(my_ip, connection_utils::SERVER_PORT);
    print(&console, format!(">>> Listen on: {:?}", listen_addr));
    let listener = TcpListener::bind(&listen_addr)?;
    let state = Arc::new(Shared::new());

    let local_state = state.clone();
    let local_console = console.clone();
    let server = listener.incoming()
        .for_each(move |socket| {
            let (sender, receiver) = futures::sync::mpsc::channel(connection_utils::CHANNEL_BUFF_SIZE);
            let addr = socket.peer_addr().expect("Socket addr 0");
            let local_state1 = local_state.clone();
            let handle_users = || -> String { 
                let mut mg = state.peers.lock().expect("State lock 0");
                let users = ">>> Other user(s):".to_string() + &list_clients(&mg);
                mg.insert(addr, (sender, None));
                users
            };
            let list_str = handle_users();
            let con = connection_utils::TextConnection::new(local_console.clone(), receiver, socket
                , Box::new(move |connection: &connection_utils::TextConnection, msg : String|{
                let addr = connection.lines.socket.peer_addr().expect("Socket address 1");
                let mut mg = local_state1.peers.lock().expect("State lock 1");
                let mut out_msg: String = String::new();
                if let Some(val) = mg.get_mut(&addr) {
                    match val {
                        (sender, None) => {
                            out_msg = format!(">>> New user: {} {:?}", &msg, &addr);
                            val.1 = Some(msg);
                            connection_utils::pass_line(sender, list_str.clone()).expect("Pass msg1");
                        },
                        (_, Some(name)) => {
                            out_msg = format!("{}: {}", &name, &msg);
                        }
                    }
                } else {
                    print(&connection.console, format!(">>> Unknown address: {:?}", &addr));
                }
                for (addrit, (sender, name)) in &mut (*mg) {
                    if *addrit != addr && *name != None {
                        connection_utils::pass_line(sender, out_msg.clone()).expect("Pass msg2");
                    }
                }
                print(&connection.console, out_msg);
            }));
            let local_console1 = local_console.clone(); 
            let local_state2 = local_state.clone();
            let match_connection = con.and_then(move |_|{
                let mut mg = local_state2.peers.lock().expect("State lock 3");
                mg.remove(&addr);
                let list_str = ">>> Connected user(s):".to_string() + &list_clients(&mg);
                for (_, (sender, name)) in &mut (*mg) {
                    if *name != None {
                        connection_utils::pass_line(sender, list_str.clone()).expect("Pass msg3");
                    }
                }
                Ok(())
            }).map_err(move |e| {
                print(&local_console1, format!(">>> transfer error = {:?}", e));
            });
            tokio::spawn(match_connection);
            Ok(())
        }).map_err(move |err| {
            print(&console, format!(">>> listining error = {:?}", err));
        });

    let mut rt = Builder::new().build().unwrap();
    rt.spawn(server);
    rt.shutdown_on_idle().wait().unwrap();
    Ok(())
}