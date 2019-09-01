extern crate tokio;
extern crate futures;
extern crate bytes;
extern crate console;
extern crate connection_utils;

mod clientonly;

use tokio::net::TcpStream;
use tokio::prelude::*;
use tokio::runtime::Builder;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::string::String;
use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;
use connection_utils::SafeConsole;
use connection_utils::print;

fn handle_received_msg(connection: &connection_utils::TextConnection, msg : String) {
    let title_size = std::cmp::min(msg.len(), 23usize);
    console::Term::stdout().set_title(format!(": {}", &msg[0..title_size]));
    print(&connection.console, msg);
}

fn handle_input(console: SafeConsole, mut sender : &mut connection_utils::Sender, line : String) {
    print(&console, format!("y: {}", &line));
    if ":quit" == line 
    {
        std::process::exit(0);
    }
    connection_utils::pass_line(&mut sender, line).unwrap();
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    console::Term::stdout().set_title("con:");
    let console :SafeConsole = Arc::new(Mutex::new(connection_utils::ConsoleBuf::new()));
    let version = env!("CARGO_PKG_VERSION");
    print(&console, format!(">>> Connection version: {}", version));

    if let Some(param) = std::env::args().nth(1) {
        if param == "-update" {
            print(&console, "Updating...".to_string());
            let _ = clientonly::update();
            return Ok(());
        }
    }

    let (name, other_ip) = clientonly::process_params();
    let (mut sender, receiver) = futures::sync::mpsc::channel(connection_utils::CHANNEL_BUFF_SIZE);
    connection_utils::pass_line(&mut sender, name).unwrap();
    let mut rt = Builder::new().build().unwrap();

    if let Ok(addr_ip) = Ipv4Addr::from_str(&other_ip)  {
        let connect_addr = SocketAddr::new(IpAddr::V4(addr_ip), connection_utils::SERVER_PORT);
        print(&console, format!(">>> trying to connect with: {:?}", connect_addr));

        let local_console = console.clone();
        let local_console2 = console.clone();
        let connection = TcpStream::connect(&connect_addr).and_then(move |socket| {
            print(&local_console, ">>> Connected!".to_string());
            let peer = connection_utils::TextConnection::new(local_console.clone()
                , receiver, socket, Box::new(handle_received_msg));
            let match_peer = peer.map_err(move |e| {
                print(&local_console, format!(">>> transfer error = {:?}", e));
            });
            tokio::spawn(match_peer);
            Ok(())
        })
        .map_err(move |err| {
            print(&local_console2, format!(">>> connection error = {:?}", err));
        });
        rt.spawn(connection);
    } else {
        print(&console, format!(">>> wrong ip: {}", other_ip));
        return Ok(());
    }

    {
        let local_console = console.clone();
        let local_console2 = console.clone();
        let input_handler = connection_utils::InputReader::new(console.clone())
            .for_each(move |line| {
                handle_input(local_console.clone(), &mut sender, line);
                Ok(())
            }).map_err(move |err| {
                print(&local_console2, format!(">>> input error = {:?}", err));
            });
        rt.spawn(input_handler);
    }

    rt.shutdown_on_idle().wait().unwrap();
    Ok(())
} 