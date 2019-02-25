use futures::future::{self, Either};
use hyper::header::HeaderValue;
use hyper::rt::{self, Future};
use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, Request, Response, Server};
use hyper::{HeaderMap, StatusCode, Version};
use std::collections::HashMap;
use std::io::{stdout, stdin, Read, Write};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;
use termion::{AsyncReader, async_stdin};
use termion::raw::IntoRawMode;
use termion::input::TermRead;
use termion::event::Key;
use tokio::io::copy;
use tokio::net::TcpStream;
use tokio::prelude::*;

const BOX_UPPER_LEFT: char = '╔';
const BOX_UPPER_RIGHT: char = '╗';
const BOX_BOTTOM_LEFT: char = '╚';
const BOX_BOTTOM_RIGHT: char = '╝';
const BOX_HORIZONTAL: char = '═';
const BOX_VERTICAL: char = '║';
const BOX_VERT_LEFT: char = '╣';
const BOX_VERT_RIGHT: char = '╠';
const BOX_HORZ_DOWN: char = '╦';
const BOX_HORZ_UP: char = '╩';
const BOX_VERT_HORZ: char = '╬';

#[derive(Clone)]
struct CachedResponse {
    status: StatusCode,
    version: Version,
    headers: HeaderMap<HeaderValue>,
    body: Vec<u8>,
}

fn main() {
    let in_addr = ([127, 0, 0, 1], 3001).into();
    let client_main = Client::new();
    let blocked_domains = Arc::new(Mutex::new(vec!["www.example.com", "garfeet.me"]));
    let response_cache = Arc::new(Mutex::new(HashMap::new()));
    let current_conn = Arc::new(Mutex::new(0));
    let (send_out, send_in) = mpsc::channel();

    let new_service = make_service_fn(move |conn: &AddrStream| {
        let _remote_addr = conn.remote_addr();
        let client = client_main.clone();
        let blocker = Arc::clone(&blocked_domains);
        let cache = Arc::clone(&response_cache);
        let id_maker = Arc::clone(&current_conn);
        let conn_id: u64 = id_maker.lock().unwrap().clone();
        *id_maker.lock().unwrap() += 1;
        let output = mpsc::Sender::clone(&send_out);

        service_fn(move |req: Request<Body>| {
            output
                .send((
                    conn_id,
                    format!("Request for {}, Type: {}", req.uri(), req.method()),
                ))
                .unwrap();
            let destination = String::from(req.uri().authority_part().unwrap().as_str());

            let block_len = blocker.lock().unwrap().len().clone();
            let mut is_blocked = false;

            for iter in 0..block_len {
                if destination.contains(blocker.lock().unwrap()[iter]) {
                    is_blocked = true;
                }
            }

            if is_blocked {
                output
                    .send((conn_id.clone(), format!("Blocked Domain!")))
                    .unwrap();
                Either::A(Either::A(future::ok(Response::new(hyper::Body::from(
                    "You have sent a request to a blocked domain.\
                     \r\nThis infraction will not be tolerated.\
                     \r\nYou will be reported to the system administrator.",
                )))))
            } else if req.method() == hyper::Method::CONNECT {
                let output = mpsc::Sender::clone(&output);
                let into_tcp = req
                    .into_body()
                    .on_upgrade()
                    .map_err({
                        let output = mpsc::Sender::clone(&output);
                        move |err| {
                            output
                                .send((conn_id.clone(), format!("error: {}", err)))
                                .unwrap()
                        }
                    })
                    .and_then(move |upgraded| {
                        let output = mpsc::Sender::clone(&output);
                        let std_server = std::net::TcpStream::connect(&destination).unwrap();

                        let server =
                            TcpStream::from_std(std_server, &tokio::reactor::Handle::default())
                                .unwrap();

                        let (client_reader, client_writer) = upgraded.split();
                        let (server_reader, server_writer) = server.split();
                        let client_to_server = copy(client_reader, server_writer)
                            .map_err({
                                let output = mpsc::Sender::clone(&output);
                                move |err| {
                                    output
                                        .send((conn_id.clone(), format!("error: {}", err)))
                                        .unwrap()
                                }
                            })
                            .map({
                                let output = mpsc::Sender::clone(&output);
                                move |(n, _, _)| {
                                    if n != 0 {
                                        output
                                            .send((
                                                conn_id.clone(),
                                                format!("{} bytes sent to server!", n),
                                            ))
                                            .unwrap()
                                    };
                                }
                            });

                        let server_to_client = copy(server_reader, client_writer)
                            .map_err({
                                let output = mpsc::Sender::clone(&output);
                                move |err| {
                                    output
                                        .send((conn_id.clone(), format!("error: {}", err)))
                                        .unwrap()
                                }
                            })
                            .map({
                                let output = mpsc::Sender::clone(&output);
                                move |(n, _, _)| {
                                    if n != 0 {
                                        output
                                            .send((
                                                conn_id.clone(),
                                                format!("{} bytes sent to client!", n),
                                            ))
                                            .unwrap()
                                    };
                                }
                            });

                        client_to_server.join(server_to_client)
                    })
                    .map(|_| ());
                tokio::spawn(into_tcp);
                Either::A(Either::B(future::ok(Response::new(hyper::Body::empty()))))
            } else {
                match cache.lock().unwrap().get(&destination) {
                    None => {
                        let output = mpsc::Sender::clone(&output);
                        output
                            .send((
                                conn_id.clone(),
                                format!("URL not seen before, caching and returning."),
                            ))
                            .unwrap();
                        Either::B(Either::A(client.request(req).map({
                            let cache = Arc::clone(&cache);
                            move |res: Response<Body>| {
                                let (parts, body) = res.into_parts();
                                let content = body
                                    .fold(Vec::new(), |mut acc, chunk| {
                                        acc.extend_from_slice(&*chunk);
                                        futures::future::ok::<_, hyper::Error>(acc)
                                    })
                                    .wait()
                                    .unwrap();
                                let to_cache = CachedResponse {
                                    status: parts.status,
                                    version: parts.version,
                                    headers: parts.headers.clone(),
                                    body: content,
                                };
                                cache
                                    .lock()
                                    .unwrap()
                                    .insert(destination.clone(), to_cache.clone());

                                let mut sendback = Response::builder()
                                    .status(to_cache.status)
                                    .version(to_cache.version)
                                    .body(Body::from(to_cache.body))
                                    .unwrap();
                                *sendback.headers_mut() = to_cache.headers;
                                sendback
                            }
                        })))
                    }
                    Some(in_cache) => {
                        output
                            .send((
                                conn_id.clone(),
                                format!("URL seen before, retrieving from cache."),
                            ))
                            .unwrap();
                        let response = in_cache.clone();
                        let mut sendback = Response::builder()
                            .status(response.status)
                            .version(response.version)
                            .body(Body::from(response.body))
                            .unwrap();
                        *sendback.headers_mut() = response.headers;
                        Either::B(Either::B(future::ok(sendback)))
                    }
                }
            }
        })
    });

    thread::spawn(move || {
        let mut stdout = stdout();
        if termion::is_tty(&stdout) {
            let mut stdout = stdout.lock().into_raw_mode().unwrap();
            let mut stdin = stdin();
            let (width, height) = termion::terminal_size().unwrap();

            write!(
                stdout,
                "{}{}",
                termion::clear::All,
                termion::cursor::Goto(1, 1)
            )
            .unwrap();

            write!(
                stdout,
                "{}{}{}{}{}{}{}{}",
                termion::cursor::Goto(1, 1),
                BOX_UPPER_LEFT,
                termion::cursor::Goto(width, 1),
                BOX_UPPER_RIGHT,
                termion::cursor::Goto(1, height),
                BOX_BOTTOM_LEFT,
                termion::cursor::Goto(width, height),
                BOX_BOTTOM_RIGHT
            )
            .unwrap();

            for col in 2..width {
                write!(stdout,"{}{}{}{}",
                       termion::cursor::Goto(col,1),
                       BOX_HORIZONTAL,
                       termion::cursor::Goto(col,height),
                       BOX_HORIZONTAL
                    ).unwrap();
            }

            for row in 2..height {
                write!(stdout,"{}{}{}{}",
                       termion::cursor::Goto(1,row),
                       BOX_VERTICAL,
                       termion::cursor::Goto(width,row),
                       BOX_VERTICAL
                       ).unwrap();
            }

            let boundary = (width / 3) * 2;
            write!(stdout,"{}{}{}{}",
                   termion::cursor::Goto(boundary,1),
                   BOX_HORZ_DOWN,
                   termion::cursor::Goto(boundary,height),
                   BOX_HORZ_UP
                   ).unwrap();

            for row in 2..height {
            write!(stdout,"{}{}",
                   termion::cursor::Goto(boundary,row),
                   BOX_VERTICAL
                ).unwrap();
            }

            stdout.flush().unwrap();

            for input in stdin.keys() {
                match input.unwrap() {
                   Key::Char('q') => std::process::exit(0),
                   Key::Char(given) => {write!(stdout,"{}{}",
                       termion::cursor::Goto(50,50),
                       given
                       ).unwrap();
                   stdout.flush().unwrap();
                   },
                   _ => {}
            }}

        } else {
            for message in send_in {
                writeln!(stdout, "Connection:  {}  -  {}", message.0, message.1).unwrap();
            }
        }
    });

    let server = Server::bind(&in_addr)
        .serve(new_service)
        .map_err(|e| eprintln!("server error: {}", e));

    rt::run(server);
}
