extern crate tokio;
extern crate futures;
extern crate bytes;
extern crate console;
extern crate connection_utils;
extern crate hyper;
extern crate dirs;
#[macro_use]
extern crate lazy_static;

mod clientonly;

use tokio::net::TcpStream;
use tokio::prelude::*;
use tokio::runtime::Builder;
use std::sync::{Arc, Mutex};
use std::string::String;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;

lazy_static! { 
    static ref CONSOLE: connection_utils::SafeConsole = Arc::new(Mutex::new(connection_utils::ConsoleBuf::new())); 
}

pub fn print(line : &str) { CONSOLE.lock().unwrap().cprint(line); }

/////////////////////////////////////////////////////////////////

fn spawn_put_request(content : Vec<u8>, uri_str : String ) {
    let uri : hyper::Uri = uri_str.parse().expect("valid uri");
    let request = hyper::Request::put(uri).body(content.into()).expect("request builder");
    let future = hyper::Client::new().request(request)
        .and_then(move |res| { print(&format!(">>> Response: {}", res.status())); Ok(()) })
        .map_err( move |err| { print(&format!(">>> Receive response error {:?}", err)); });
    tokio::spawn(future);
}

fn spawn_send_single_file(path_str: String, file_server_uri: String) {
    let path = std::path::Path::new(&path_str);
    if !path.exists() || !path.is_file() { print(&format!(">>> WRONG PATH {}", &path_str)); return; }
    let filename = path.file_name().expect("proper filename").to_str().expect("string").to_string();
    let task = tokio::fs::File::open(path_str.clone())
        .and_then(move |mut file| {
            print(&format!(">>> Reading... {}", &path_str));
            let mut content : Vec<u8> = vec![];
            if let Err(e) = file.read_to_end(&mut content) { return Err(e); }
            print(&format!(">>> Sending... {}{} {} bytes", &file_server_uri, &filename, content.len()));
            spawn_put_request(content, format!("{}{}", file_server_uri, filename));
            Ok(())
        })
        .map_err(|err| { print(&format!(">>> Send file error {:?}", err)); });
    tokio::spawn(task);
}

/////////////////////////////////////////////////////////////////

fn spawn_receive_file_request(in_filename_str: &String, file_server_uri: &String) {
    let filename_buff = std::path::Path::new(in_filename_str);
    let mut filename = String::new();
    if let Some(_) = filename_buff.file_stem() {
        if let Some(local_filename) = filename_buff.file_name() {
            filename = local_filename.to_str().expect("string").to_string();
        }
    }
    if filename.is_empty() { 
        print(&format!(">>> Wrong file name: {}", in_filename_str)); 
        return;
    }

    let mut download_file_path = dirs::download_dir().unwrap();
    download_file_path.push(&filename);
    let uri : hyper::Uri = format!("{}{}", file_server_uri, filename).parse().expect("valid uri");
    let request = hyper::Request::get(uri).body(hyper::Body::empty()).expect("request builder");
    let future = hyper::Client::new().request(request)
        .and_then(move |res| { 
            if res.status() != hyper::http::StatusCode::OK {
                print(&format!(">>> Response: {}", res.status())); 
            } else if let Err(e) = connection_utils::save_body_to_file_blocking(res.into_body(), &download_file_path) {
                print(&format!(">>> Receiving file error: {:?}", e));
            } else {
                 print(&format!(">>> Saved: {}", download_file_path.to_str().unwrap()));
            }
            Ok(()) 
        }).map_err( move |err| { print(&format!(">>> Receive response error {:?}", err)); });
    tokio::spawn(future);
}

/////////////////////////////////////////////////////////////////

fn handle_received_msg(_: &connection_utils::TextConnection, msg : String) {
    let title_size = std::cmp::min(msg.len(), 24usize);
    console::Term::stdout().set_title(format!(">{}", &msg[..title_size]));
    print(&msg);
}

fn text_protocol_job(receiver: connection_utils::Receiver, connect_addr: SocketAddr
    , rt: &mut tokio::runtime::Runtime) {
    print(&format!(">>> trying to connect with: {:?}", connect_addr));
    let start_connection = TcpStream::connect(&connect_addr)
        .and_then(move |socket| {
            let connection = connection_utils::TextConnection::new(receiver, socket, Box::new(handle_received_msg))
                .and_then(move |_|{ print(">>> DISCONNECTED"); Ok(())})
                .map_err (move |e|{ print(&format!(">>> transfer error = {:?}", e)); });
            tokio::spawn(connection);
            Ok(())
        })
        .map_err(move |err| { print(&format!(">>> connection error = {:?}", err)); });
    rt.spawn(start_connection);
}

/////////////////////////////////////////////////////////////////

fn input_job(name: String, mut text_sender: connection_utils::Sender, file_server_uri: String, rt: &mut tokio::runtime::Runtime) {
    let input_handler = connection_utils::InputReader::new(CONSOLE.clone())
        .for_each(move |line| {
            if let Some(filename) = clientonly::parse_send_file(&line) {
                spawn_send_single_file(filename, file_server_uri.clone());
            } else if let Some(filename) = clientonly::parse_receive_file(&line) {
                spawn_receive_file_request(&filename, &file_server_uri);
            } else if let Err(e) = connection_utils::pass_line(&mut text_sender, line.clone()) {
                print(&format!("Cannot send, error: {}", e));
            } else {
                print(&format!("{}: {}", &name, &line));
            }
            Ok(())
        }).map_err(move |err| { print(&format!(">>> input error = {:?}", err)); });
    rt.spawn(input_handler);
}

////////////////////////////////////////////////////////////////

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    console::Term::stdout().set_title("con:");
    print(&format!(">>> Connection version: {}", env!("CARGO_PKG_VERSION")));
        if let Some(param) = std::env::args().nth(1) {
        if param == "-update" {
            print(">>> Updating...");
            let _ = clientonly::update();
            return Ok(());
        }
    }

    let (name, server_ip_str) = clientonly::process_params();
    if let Ok(server_ip4) = Ipv4Addr::from_str(&server_ip_str) {
        let mut rt = Builder::new().build().unwrap();
        let (mut text_sender, text_receiver) = futures::sync::mpsc::channel(connection_utils::CHANNEL_BUFF_SIZE);
        let file_server_uri = format!("http://{}:{}/", &server_ip_str, connection_utils::SERVER_PORT_FILE);
        let text_server_addr = SocketAddr::new(IpAddr::V4(server_ip4), connection_utils::SERVER_PORT_TEXT);

        connection_utils::pass_line(&mut text_sender, name.clone()).unwrap(); //intoduce yourself
        text_protocol_job(text_receiver, text_server_addr, &mut rt);
        input_job(name, text_sender, file_server_uri, &mut rt);

        rt.shutdown_on_idle().wait().unwrap();
    } else { print(&format!(">>> wrong ip: {}", server_ip_str)); }
    Ok(())
} 