extern crate tokio;
extern crate futures;
extern crate bytes;
extern crate console;
extern crate connection_utils;
extern crate hyper;
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

lazy_static! { static ref CONSOLE: connection_utils::SafeConsole = Arc::new(Mutex::new(connection_utils::ConsoleBuf::new())); }
pub fn print(line : String) { CONSOLE.lock().unwrap().cprint(&line); }

fn handle_received_msg(_: &connection_utils::TextConnection, msg : String) {
    let title_size = std::cmp::min(msg.len(), 23usize);
    console::Term::stdout().set_title(format!(": {}", &msg[0..title_size]));
    print(msg);
}

fn text_protocol_job(receiver: connection_utils::Receiver, connect_addr: SocketAddr, rt: &mut tokio::runtime::Runtime) {
    print(format!(">>> trying to connect with: {:?}", connect_addr));
    let start_connection = TcpStream::connect(&connect_addr)
        .and_then(move |socket| {
            let connection = connection_utils::TextConnection::new(CONSOLE.clone(), receiver, socket, Box::new(handle_received_msg))
                .and_then(move |_|{ print(">>> DISCONNECTED".to_string()); Ok(())})
                .map_err (move |e|{ print(format!(">>> transfer error = {:?}", e)); });
            tokio::spawn(connection);
            Ok(())
        })
        .map_err(move |err| { print(format!(">>> connection error = {:?}", err)); });
    rt.spawn(start_connection);
}

fn input_job(name: String, mut text_sender: connection_utils::Sender, mut file_sender: connection_utils::Sender, rt: &mut tokio::runtime::Runtime) {
    let input_handler = connection_utils::InputReader::new(CONSOLE.clone())
        .for_each(move |line| {
            if let Some(filename) = clientonly::parse_send_file(&line) {
                if let Err(e) = connection_utils::pass_line(&mut file_sender, filename) { // todo..
                    print(format!("Cannot send, error: {}", e));
                }
            } else if let Err(e) = connection_utils::pass_line(&mut text_sender, line.clone()) {
                print(format!("Cannot send, error: {}", e));
            } else {
                print(format!("{}: {}", &name, &line));
            }
            Ok(())
        }).map_err(move |err| { print(format!(">>> input error = {:?}", err)); });
    rt.spawn(input_handler);
}

//to lib
fn spawn_put_file_request(content : Vec<u8>, uri_str : String ) {
    let uri : hyper::Uri = uri_str.parse().expect("valid uri");
    let request = hyper::Request::put(uri).body(content.into()).expect("request builder");
    let future = hyper::Client::new().request(request)
        .and_then(move |res| { print(format!(">>> Response: {}", res.status())); Ok(()) })
        .map_err( move |err| { print(format!(">>> Receive response error {:?}", err)); });
    tokio::spawn(future);
}

fn send_file_job_inner(path_str: String, uri_str: String) {
    let path = std::path::Path::new(&path_str);
    if !path.exists() { print(format!(">>> WRONG PATH {}", &path_str)); return; }
    let filename = path.file_name().expect("proper filename").to_str().expect("string").to_string();

    let task = tokio::fs::File::open(path_str.clone())
        .and_then(move |mut file| {
            print(format!(">>> Reading... {}", &path_str));
            let mut content : Vec<u8> = vec![];
            if let Err(e) = file.read_to_end(&mut content) { return Err(e); }
            print(format!(">>> Sending... {}", &filename));
            spawn_put_file_request(content, format!("{}{}", uri_str, filename));
            Ok(())
        })
        .map_err(|err| { print(format!(">>> Send file error {:?}", err)); });
    tokio::spawn(task);
}

fn send_file_job(receiver: connection_utils::Receiver, uri_str: String, rt: &mut tokio::runtime::Runtime) {
    let send_loop = receiver
        .for_each(move |vmsg|{
            let path_str = connection_utils::bytes_to_str(&vmsg);
            send_file_job_inner(path_str, uri_str.clone()); 
            Ok(()) 
        })
        .map_err(|err| { print(format!(">>> send file channel error = {:?}", err)); });
    rt.spawn(send_loop);
}

fn save_body_to_file_blocking(body: hyper::Body, file_path: &std::path::PathBuf) {
    let file = std::fs::File::create(file_path).map_err(|e| { print(format!(">>> create file error = {:?}", e)) }).unwrap();
    let mut local_file = file.try_clone().unwrap();
    body.for_each(move |chunk| {
        local_file.write_all(&chunk).map_err(|e| { print(format!(">>> save file error = {:?}", e)); }).unwrap();
        Ok(())
    }).map_err(|e| { print(format!(">>> read body error = {:?}", e)); }).wait().unwrap();
    file.sync_all().map_err(|e| { print(format!(">>> sync file error = {:?}", e)) }).unwrap();
}

fn receive_file_handle_request(request: hyper::Request<hyper::Body>) -> hyper::Response<hyper::Body> {
    if request.method() != hyper::Method::PUT {
        print(format!(">>> METHOD_NOT_ALLOWED: {:?}", request.method()));
        return hyper::Response::builder().status(hyper::http::StatusCode::METHOD_NOT_ALLOWED)
            .header("Allow", "PUT").body(hyper::Body::empty()).unwrap();
    }
    let mut file_path = dirs::download_dir().unwrap();
    file_path.push(request.uri().path());
    if !file_path.is_file() {
        print(format!(">>> wrong PUT path: {:?}", request.uri().path()));
        return hyper::Response::builder().status(hyper::http::StatusCode::BAD_REQUEST)
            .body(hyper::Body::empty()).unwrap();
    }
    print(format!(">>> Receiving file... {:?}", &file_path));
    let already_exist = file_path.exists();
    save_body_to_file_blocking(request.into_body(), &file_path);

    print(format!(">>> File saved {:?}", &file_path));
    let status = if already_exist {hyper::http::StatusCode::OK} else {hyper::http::StatusCode::CREATED};
    hyper::Response::builder().status(status).body(hyper::Body::empty()).unwrap()
}

fn receive_file_server_job(addr : SocketAddr, rt: &mut tokio::runtime::Runtime){
    let server = hyper::Server::bind(&addr)
        .serve(|| { hyper::service::service_fn_ok(receive_file_handle_request) })
        .map_err(|err| { print(format!(">>> Receive file error {:?}", err)); });
    rt.spawn(server);
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    console::Term::stdout().set_title("con:");
    print(format!(">>> Connection version: {}", env!("CARGO_PKG_VERSION")));
        if let Some(param) = std::env::args().nth(1) {
        if param == "-update" {
            println!(">>> Updating...");
            let _ = clientonly::update();
            return Ok(());
        }
    }

    let (name, server_ip_str) = clientonly::process_params();
    if let Ok(server_ip4) = Ipv4Addr::from_str(&server_ip_str) {
        let mut rt = Builder::new().build().unwrap();
        let (mut text_sender, text_receiver) = futures::sync::mpsc::channel(connection_utils::CHANNEL_BUFF_SIZE);
        let (file_sender, file_receiver) = futures::sync::mpsc::channel(connection_utils::CHANNEL_BUFF_SIZE);
        let server_ip = IpAddr::V4(server_ip4);
        let uri_str = format!("http://{}:{}/", &server_ip_str, connection_utils::SERVER_PORT_FILE);
        let my_ip = clientonly::list_ip().expect("valid ip");

        connection_utils::pass_line(&mut text_sender, name.clone()).unwrap(); //intoduce yourself
        text_protocol_job(text_receiver, SocketAddr::new(server_ip, connection_utils::SERVER_PORT_TEXT), &mut rt);
        send_file_job(file_receiver, uri_str, &mut rt);
        receive_file_server_job(SocketAddr::new(my_ip, connection_utils::SERVER_PORT_TEXT), &mut rt);
        input_job( name, text_sender, file_sender, &mut rt);

        rt.shutdown_on_idle().wait().unwrap();
    } else { print(format!(">>> wrong ip: {}", server_ip_str)); }
    Ok(())
} 