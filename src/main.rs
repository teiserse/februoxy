use hyper::{Client, Server, Request, Response, Body};
use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use hyper::rt::{self, Future};
use futures::future::{self, Either};
//use std::net::ToSocketAddrs;
use tokio::prelude::*;
use tokio::io::{copy, shutdown};
use tokio::net::TcpStream;
//use std::collections::HashMap;
//use std::sync::{Arc, Mutex};


fn main() {
    let in_addr = ([127, 0, 0, 1], 3001).into();
    let client_main = Client::new();

    //let mut cache = Arc::new(Mutex::new(HashMap::new()));

    let new_service = make_service_fn(move |conn: &AddrStream| {
        let _remote_addr = conn.remote_addr();
        let client = client_main.clone();
        //let mut cache_access = Arc::clone(&cache);

        service_fn(move |req: Request<Body>| {
            println!("{:?}", req);
            let destination = String::from(req.uri().authority_part().unwrap().as_str());
            if destination.contains("example") {
                Either::A(Either::A(
                    future::ok(Response::new(hyper::Body::from("Another Example\n")))
                ))
            } else if destination.contains(":443") {
                println!("Attempted HTTPS Connection!");
                //let dest_addr = destination.to_socket_addrs().unwrap().as_slice()[0];
                /*
                                let server = TcpStream::connect(&dest_addr).and_then(|server|{
                                    let (client_reader, client_writer) = conn.split();
                                    let (server_reader, server_writer) = server.split();

                                    let client_to_server = copy(client_reader, server_writer);

                                    let server_to_client = copy(server_reader, client_writer);

                                    client_to_server.join(server_to_client)
                                });*/

                let into_tcp =
                    req.into_body().on_upgrade().map_err(move |err| eprintln!("error: {}", err))
                        .map(move |upgraded| {

                            let std_server = std::net::TcpStream::connect(&destination).unwrap();

                            let server = TcpStream::from_std(std_server, &tokio::reactor::Handle::default()).unwrap();

                            let (client_reader, client_writer) = upgraded.split();
                            let (server_reader, server_writer) = server.split();
                            let client_to_server = copy(client_reader, server_writer)
                                .map(|(n, _, server_writer)| {
                                println!("{} bytes!",n);
                            });

                            let server_to_client = copy(server_reader, client_writer)
                                .map(|(n, _, client_writer)| {
                                println!("{} bytes!",n);
                            });

                            client_to_server.join(server_to_client)

                        }).map(move |_| {
                        println!("testing!");
                    });
                tokio::spawn(into_tcp);
                Either::A(Either::B(
                    future::ok(Response::new(hyper::Body::empty()))
                ))
            } else {
                Either::B(client.request(req).map(|res| {
                    println!("{:?}", res.headers());
                    //cache_access.lock().unwrap().insert("questionmark", "test");
                    res
                }))
            }
        })
    });

    let server_old = Server::bind(&in_addr)
        .serve(new_service)
        .map_err(|e| eprintln!("server error: {}", e));


    println!("Listening on http://{}", in_addr);
    rt::run(server_old);
}