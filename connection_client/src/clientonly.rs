extern crate tokio;
extern crate futures;
extern crate bytes;
extern crate console;

use tokio::prelude::*;
use std::string::String;
use std::path::Path;
use std::fs::File;
use std::io::{BufReader, BufRead};

pub fn update() -> Result<(), Box<dyn ::std::error::Error>> {
    let current_version = env!("CARGO_PKG_VERSION");
    let bulid_result = self_update::backends::github::Update::configure()
        .repo_owner("k4210")
        .repo_name("connection")
        .bin_name("connection_client.exe")
        .show_download_progress(true)
        .current_version(current_version)
        .build();
    match bulid_result {
        Ok(update_builder) => {
            match update_builder.update() {
                Ok(status) => {println!("Update status: {}", status.version());},
                Err(e) => {println!("Update Error: {:?}", e);}
            }
        },
        Err(e) => println!("Find build Error: {:?}", e)
    };
    Ok(())
}
 
pub fn read_config(file_path : &Path) -> (Option<String>, Option<String>) {
    if file_path.exists() && file_path.is_file() {
        let file = File::open(&file_path).unwrap();
        let mut reader = BufReader::new(file);
        
        let mut name_str = String::new();
        let name = match reader.read_line(&mut name_str) {
            Ok(0) => None,
            Ok(_) => {
                name_str.truncate(name_str.trim_end().len());
                Some(name_str)
            },
            _ => None
        };

        let mut ip_str = String::new();
        let ip = match reader.read_line(&mut ip_str) {
            Ok(0) => None,
            Ok(_) => {
                ip_str.truncate(ip_str.trim_end().len());
                Some(ip_str)
            },
            _ => None
        };
        return (name, ip);
    }
    (None, None)
}

pub fn save_config(file_path : &Path, name : &String, ip : &String) {
    let mut f = File::create(file_path).unwrap();
    writeln!(&mut f, "{}", name).unwrap();
    writeln!(&mut f, "{}", ip).unwrap();
    let _ = f.sync_data();
    println!(">>> config: {:?}", file_path);
}

pub fn process_params() -> (String, String) {
    let mut file_path = dirs::data_local_dir().unwrap();
    file_path.push("Connection.cfg");
    let (name_config, other_ip_config) = read_config(file_path.as_path());
    let name = match std::env::args().nth(1) {
        Some(val) => val,
        None => match name_config {
            Some(val) => val,
            None => "UnnamedUser".to_string()
        }
    };
    let ip = match std::env::args().nth(2) {
        Some(val) => val,
        None => match other_ip_config {
            Some(val) => val,
            None => "89.67.243.241".to_string()
        }
    };
    save_config(file_path.as_path(), &name, &ip);
    println!(">>> name: {}", name);
    println!(">>> other ip: {:?}", ip);
    (name, ip)
}

pub fn parse_send_file(msg : &String) -> Option<String> {
    let start_pattern = ":send \"";
    if !msg.starts_with(start_pattern) {
        //println!(">>> !msg.starts_with");
        return None;
    } 
    if !msg.ends_with('\"') {
        //println!(">>> !msg.ends_with");
        return None;
    } 
    if !(msg.len() > 8) {
        //println!(">>> !msg.len() > 8");
        return None;
    }
    let mut result = msg.replace(start_pattern, "");
    result.pop();
    Some(result)
}

pub fn parse_receive_file(msg : &String) -> Option<String> {
    let start_pattern = ":receive \"";
    if !msg.starts_with(start_pattern) {
        //println!(">>> !msg.starts_with");
        return None;
    } 
    if !msg.ends_with('\"') {
        //println!(">>> !msg.ends_with");
        return None;
    } 
    if !(msg.len() > 10) {
        //println!(">>> !msg.len() > 8");
        return None;
    }
    let mut result = msg.replace(start_pattern, "");
    result.pop();
    Some(result)
}
