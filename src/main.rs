use hyper::{Client, Server};
use hyper::service::service_fn;
use hyper::rt::{self, Future};
use std::net::SocketAddr;
fn main() {
    let in_addr = ([127, 0, 0, 1], 3001).into();

    let client_main = Client::new();

    let new_service = move || {
        let client = client_main.clone();
        service_fn(move |req| {
            println!("{:?}", req);
            client.request(req)
        })
    };

    let server = Server::bind(&in_addr)
        .serve(new_service)
        .map_err(|e| eprintln!("server error: {}", e));

    println!("Listening on http://{}", in_addr);
    rt::run(server);
}