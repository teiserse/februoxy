use hyper::{Client, Server, Request, Response, Body};
use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use hyper::rt::{self, Future};
use futures::future::{self, Either};
use tokio::prelude::*;
use tokio::io::copy;
use tokio::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::collections::hash_map::RandomState;

fn main() {
    let in_addr = ([127, 0, 0, 1], 3001).into();
    let client_main = Client::new();
    let blocked_domains = Arc::new(Mutex::new(vec!["www.example.com", "garfeet.me"]));
    let response_cache: Arc<Mutex<HashMap<String, String, RandomState>>> = Arc::new(Mutex::new(HashMap::new()));
    let current_conn = Arc::new(Mutex::new(0));

    let new_service = make_service_fn(move |conn: &AddrStream| {
        let _remote_addr = conn.remote_addr();
        let client = client_main.clone();
        let blocker = Arc::clone(&blocked_domains);
        let cache = Arc::clone(&response_cache);
        let id_maker = Arc::clone(&current_conn);
        let conn_id: u64 = id_maker.lock().unwrap().clone();
        *id_maker.lock().unwrap() += 1;

        service_fn(move |req: Request<Body>| {
            println!("Connection:  {}  -  Request for {}, Type: {}", conn_id, req.uri(), req.method());
            let destination = String::from(req.uri().authority_part().unwrap().as_str());

            let block_len = blocker.lock().unwrap().len().clone();
            let mut is_blocked = false;

            for iter in 0..block_len {
                if destination.contains(blocker.lock().unwrap()[iter]) {
                    is_blocked = true;
                }
            }

            if is_blocked {
                println!("Connection:  {}  -  Blocked Domain!", &conn_id);
                Either::A(Either::A(
                    future::ok(Response::new(hyper::Body::from(
                        "You have sent a request to a blocked domain.\
                        \r\nThis infraction will not be tolerated.\
                        \r\nYou will be reported to the system administrator.")))
                ))
            } else if destination.contains(":443") {
                let into_tcp =
                    req.into_body().on_upgrade().map_err(move |err| eprintln!("Connection: {}  -  error: {}", &conn_id, err))
                        .and_then(move |upgraded| {
                            let std_server = std::net::TcpStream::connect(&destination).unwrap();

                            let server = TcpStream::from_std(std_server, &tokio::reactor::Handle::default()).unwrap();

                            let (client_reader, client_writer) = upgraded.split();
                            let (server_reader, server_writer) = server.split();
                            let client_to_server = copy(client_reader, server_writer)
                                .map_err(move |err| eprintln!("Connection:  {}  -  error: {}", &conn_id, err))
                                .map(move |(n, _, _)| {
                                    if n != 0 { println!("Connection:  {}  -  {} bytes sent to server!", &conn_id, n) };
                                });

                            let server_to_client = copy(server_reader, client_writer)
                                .map_err(move |err| eprintln!("Connection:  {}  -  error: {}", &conn_id, err))
                                .map(move |(n, _, _)| {
                                    if n != 0 { println!("Connection:  {}  -  {} bytes sent to client!", &conn_id, n) };
                                });

                            client_to_server.join(server_to_client)
                        }).map(|_| { () });
                tokio::spawn(into_tcp);
                Either::A(Either::B(
                    future::ok(Response::new(hyper::Body::empty()))
                ))
            } else {
                Either::B(client.request(req).map(|res| {
                    cache.lock().unwrap().insert(String::from("test"), String::from("ing"));
                    res
                }))
            }
        })
    });

    let server = Server::bind(&in_addr)
        .serve(new_service)
        .map_err(|e| eprintln!("server error: {}", e));


    println!("Listening on http://{}", in_addr);
    rt::run(server);
}