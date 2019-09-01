extern crate tokio;
extern crate futures;
extern crate bytes;
extern crate connection_utils;
extern crate ipconfig;

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


pub fn list_ip() -> Option<IpAddr>{
    let mut res : Option<IpAddr> = Option::None;
    for adapter in ipconfig::get_adapters().unwrap() {
        if (adapter.oper_status() == ipconfig::OperStatus::IfOperStatusUp) && 
            (adapter.if_type() != ipconfig::IfType::SoftwareLoopback) {
            for ip in adapter.ip_addresses() {
                if ip.is_ipv4() {
                    println!(">>> Found network adapter: {} {:?}", adapter.friendly_name(), ip);
                    if let None = res
                    {
                        res = Some(*ip);
                    }
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
    let my_ip: IpAddr = list_ip().unwrap();
    let listen_addr = SocketAddr::new(my_ip, connection_utils::SERVER_PORT);
    print(&console, format!(">>> Listen on: {:?}", listen_addr));
    let listener = TcpListener::bind(&listen_addr)?;
    let state = Arc::new(Shared::new());

    let local_state = state.clone();
    let local_console = console.clone();
    let server = listener.incoming()
        .for_each(move |socket| {
            let (sender, receiver) = futures::sync::mpsc::channel(connection_utils::CHANNEL_BUFF_SIZE);
            let addr = socket.peer_addr().unwrap();
            let local_state1 = local_state.clone();
            state.peers.lock().unwrap().insert(addr, (sender, None));
            let con = connection_utils::TextConnection::new(local_console.clone(), receiver, socket
                , Box::new(move |connection: &connection_utils::TextConnection, msg : String|{
                let addr = connection.lines.socket.peer_addr().unwrap();
                let mut mg = local_state1.peers.lock().unwrap();
                let mut out_msg: String = String::new();
                if let Some(val) = mg.get_mut(&addr) {
                    match val {
                        (_, None) => {
                            print(&connection.console , format!(">>> New client: {} {:?}", msg, &addr));
                            val.1 = Some(msg);
                            return;
                        },
                        (_, Some(name)) => {
                            out_msg = format!("{}: {}", &name, msg);
                        }
                    }
                } else {
                    print(&connection.console , format!(">>> Unknown address: {:?}", &addr));
                }

                for (addrit, (sender, _)) in &mut (*mg) {
                    if *addrit != addr {
                        connection_utils::pass_line(sender, out_msg.clone()).unwrap();
                    }
                }
                print(&connection.console, out_msg);
            }));
            let local_console1 = local_console.clone(); 
            let local_state2 = local_state.clone();
            let match_connection = con.and_then(move |_|{
                local_state2.peers.lock().unwrap().remove(&addr);
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