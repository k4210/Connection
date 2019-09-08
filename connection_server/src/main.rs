extern crate tokio;
extern crate futures;
extern crate bytes;
extern crate connection_utils;
#[macro_use]
extern crate lazy_static;
extern crate hyper;
extern crate dirs;

use tokio::net::{TcpListener, TcpStream};
use tokio::prelude::*;
use tokio::runtime::Builder;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::string::String;
use std::net::IpAddr;
use std::collections::HashMap;
use connection_utils::TextConnection;
use std::collections::VecDeque;

pub const HISTORY_SIZE: usize = 16;
pub type Clients = HashMap<SocketAddr, (connection_utils::Sender, Option<String>)>;

lazy_static! { 
    static ref CONSOLE: connection_utils::SafeConsole = Arc::new(Mutex::new(connection_utils::ConsoleBuf::new())); 
    static ref PEERS: Arc<Mutex<Clients>> = Arc::new(Mutex::new(HashMap::new()));
    static ref HISTORY: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::with_capacity(HISTORY_SIZE)));
}

pub fn print(line : &str) { CONSOLE.lock().unwrap().cprint(line); }

//////////////////////////////////////////////////////////////////////////////////////////////

fn push_history(msg: String){
    let mut local_history = HISTORY.lock().expect("history");
    if local_history.len() == HISTORY_SIZE {
        local_history.pop_front();
    } 
    local_history.push_back(msg);
}

fn list_clients(clients: &Clients, excluded_adds: &SocketAddr) -> String {
    let mut result = String::new();
    for (addr, (_, name_opt)) in clients {
        if let Some(name) = name_opt {
            if excluded_adds != addr {
                result += " ";
                result += name;
            }
        }
    }
    return result;
}

fn handle_new_named_user(clients: &mut Clients, addr: &SocketAddr, msg: String) -> String{
    let list_str = list_clients(&clients, &addr);
    let (sender, out_name) = clients.get_mut(&addr).expect("Known address");
    let out_msg = format!(">>> New user: {} {:?}", &msg, &addr);
    *out_name = Some(msg);
    for line in HISTORY.lock().expect("history1").iter() {
        connection_utils::pass_line(sender, line.clone()).expect("Pass msg0");
    }
    connection_utils::pass_line(sender, format!(">>> Connected! Other user(s): {}", &list_str)).expect("Pass msg1");
    out_msg
}

fn handle_add_user(addr: &SocketAddr) -> connection_utils::Receiver {
    let (sender, receiver) = futures::sync::mpsc::channel(connection_utils::CHANNEL_BUFF_SIZE);
    print(&format!(">>> {} connected", &addr));
    PEERS.lock().expect("State lock 0").insert(*addr, (sender, None));
    receiver
}

fn handle_removed_user(addr: &SocketAddr) {
    let mut clients = PEERS.lock().expect("State lock 3");
    if let Some((_, Some(disconected_name))) = clients.remove(&addr) {
        let list_str = format!(">>> {} left. User(s):{}", disconected_name, &list_clients(&clients, &addr));
        for (_, (sender, name)) in &mut (*clients) {
            if *name != None {
                connection_utils::pass_line(sender, list_str.clone()).expect("Pass msg3");
            }
        }
        print(&list_str);
    }
    else { print(&format!(">>> {} disconnected", &addr)); }
}

fn handle_receive_msg(connection: &TextConnection, msg : String){
    let addr = connection.lines.socket.peer_addr().expect("Socket address 1");
    let mut mg = PEERS.lock().expect("State lock 1");
    let out_msg: String;
    if let (_, Some(name)) = mg.get(&addr).expect("Known address") {
        out_msg = format!("{}: {}", &name, &msg);
        push_history(out_msg.clone());
    } else {
       out_msg = handle_new_named_user(&mut mg, &addr, msg);
    }
    for (addrit, (sender, name)) in &mut (*mg) {
        if *addrit != addr && *name != None {
            connection_utils::pass_line(sender, out_msg.clone()).expect("Pass msg2");
        }
    }
    print(&out_msg);
}

fn handle_text_connection(socket :TcpStream) -> Result<(), std::io::Error> {
    let addr = socket.peer_addr().expect("Socket addr 0");
    let receiver = handle_add_user(&addr);
    let con = TextConnection::new(receiver, socket, Box::new(handle_receive_msg))
        .and_then(move |_|{ handle_removed_user(&addr); Ok(()) })
        .map_err(move |e| { print(&format!(">>> transfer error = {:?}", e)); });
    tokio::spawn(con);
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////////////

fn spawn_save_body_to_file(body: hyper::Body, file_path: std::path::PathBuf){
    let task = body
        .fold(Vec::new(), |mut v, chunk| {
            v.extend(&chunk[..]);
            future::ok::<_, hyper::Error>(v)
        })
        .map_err(|err| { std::io::Error::new(std::io::ErrorKind::Other, err.to_string()) })
        .and_then(move |chunks| {
            let mut file = std::fs::File::create(&file_path)?;
            if let Err(e) = file.write_all(&chunks) { return Err(e); }
            if let Err(e) = file.sync_all()  { return Err(e); }

            let mut clients = PEERS.lock().expect("clients");
            let msg = format!(">>> Server received file: {}", file_path.file_name().unwrap().to_str().unwrap());            
            for (_, (sender, name)) in &mut (*clients) {
                if *name != None {
                    connection_utils::pass_line(sender, msg.clone()).expect("Pass msg");
                }
            }
            print(&msg);
            push_history(msg);
            Ok(())
        }).map_err(|err| { print(&format!("save_body_to_file error: {:?}", err)); });
    tokio::spawn(task);
}

fn handle_file_server_request(request: hyper::Request<hyper::Body>) -> hyper::Response<hyper::Body> {
    print(&format!(">>> Request received {:?}", request));

    let method = request.method();
    if method != hyper::Method::PUT && method != hyper::Method::GET {
        print(&format!(">>> METHOD_NOT_ALLOWED: {:?}", method));
        return hyper::Response::builder().status(hyper::http::StatusCode::METHOD_NOT_ALLOWED)
            .header("Allow", "PUT, GET").body(hyper::Body::empty()).unwrap();
    }
    let path_uri = request.uri().path().to_owned();
    let file_name = std::path::Path::new(&path_uri);
    if None == file_name.file_stem() {
        print(&format!(">>> wrong request path: {:?}", file_name));
        return hyper::Response::builder().status(hyper::http::StatusCode::BAD_REQUEST).body(hyper::Body::empty()).unwrap();
    }
    let filename_str = file_name.file_name().unwrap();
    let mut file_path = dirs::download_dir().unwrap();
    file_path.push(filename_str);
    let already_exist = file_path.exists();

    ///// PUT
    if method == hyper::Method::PUT {
        print(&format!(">>> Receiving file... {:?}", &file_path));
        spawn_save_body_to_file(request.into_body(), file_path);
        let status = if already_exist {hyper::http::StatusCode::OK} else {hyper::http::StatusCode::CREATED};
        return hyper::Response::builder().status(status).body(hyper::Body::empty()).unwrap()
    } 

    /////// GET
    std::assert_eq!(method, hyper::Method::GET);
    if !already_exist {
        print(&format!(">>> wrong request path: {:?}", file_name));
        return hyper::Response::builder().status(hyper::http::StatusCode::NOT_FOUND).body(hyper::Body::empty()).unwrap();
    }
    let file_content = connection_utils::read_file_blocking(&file_path);
    if let Err(e) = file_content {
        print(&format!(">>> Reading file error: {:?}", e));
        return hyper::Response::builder().status(hyper::http::StatusCode::INTERNAL_SERVER_ERROR).body(hyper::Body::empty()).unwrap();
    }
    print(&format!(">>> File send {:?}", &file_path));
    hyper::Response::new(file_content.unwrap().into())
}

////////////////////////////////////////////////////////////////////////////////////////

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    let version = env!("CARGO_PKG_VERSION");
    print(&format!(">>> Connection version: {}", version));
    let my_ip: IpAddr = connection_utils::list_ip().expect("a valid ip");
    let mut rt = Builder::new().build().unwrap();

    {
        let listen_addr = SocketAddr::new(my_ip, connection_utils::SERVER_PORT_TEXT);
        print(&format!(">>> Text server on: {:?}", &listen_addr));
        let text_server = TcpListener::bind(&listen_addr)?.incoming()
            .for_each(handle_text_connection)
            .map_err(move |err| { print(&format!(">>> Text server error = {:?}", err)); });
        rt.spawn(text_server);
    }

    {
        let listen_addr = SocketAddr::new(my_ip, connection_utils::SERVER_PORT_FILE);
        print(&format!(">>> File server on {:?}", &listen_addr));
        let file_server = hyper::Server::bind(&listen_addr)
            .serve(|| { hyper::service::service_fn_ok(handle_file_server_request) })
            .map_err(|err| { print(&format!(">>> File server error {:?}", err)); });
        rt.spawn(file_server);
    }

    rt.shutdown_on_idle().wait().unwrap();
    Ok(())
}