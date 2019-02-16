use hyper::{Client, Server, Response};
use hyper::service::service_fn;
use hyper::rt::{self, Future};
use std::collections::HashMap;
use std::sync::Arc;

fn main() {
    let in_addr = ([127, 0, 0, 1], 3001).into();
    let client_main = Client::new();
    let mut cache = Arc::new(HashMap::new());

    let new_service = move || {
        let client = client_main.clone();
        let mut cache_access = Arc::clone(&cache);
        service_fn(move |req| {
            println!("{:?}", req);
            match cache_access.get(req.uri()) {
                Some(res) => res,
                None =>
                    client.request(req).map(|res| {
                        println!("{:?}", res.headers());
                        cache_access.insert(req.uri(), res);
                        res
                    })
            }
        })
    };

    let server = Server::bind(&in_addr)
        .serve(new_service)
        .map_err(|e| eprintln!("server error: {}", e));

    println!("Listening on http://{}", in_addr);
    rt::run(server);
}