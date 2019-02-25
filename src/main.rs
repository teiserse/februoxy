use futures::future::{self, Either};
use hyper::header::HeaderValue;
use hyper::rt::{self, Future};
use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, Request, Response, Server};
use hyper::{HeaderMap, StatusCode, Version};
use std::collections::HashMap;
use std::io::{stdout, Write};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;
use termion::async_stdin;
use termion::raw::IntoRawMode;
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
                    format!("{} request for {}", req.method(), req.uri()),
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
                        let method_used = req.method().clone();
                        Either::B(Either::A(client.request(req).map({
                            let cache = Arc::clone(&cache);
                            move |res: Response<Body>| {
                                if let Some(header) =
                                    res.headers().get(hyper::header::CACHE_CONTROL)
                                {
                                    if method_used != hyper::Method::GET {
                                        return res;
                                    }
                                    if header.to_str().unwrap().contains("no-cache") {
                                        output
                                            .send((
                                                conn_id.clone(),
                                                format!("URL not seen before, no-cache."),
                                            ))
                                            .unwrap();
                                        return res;
                                    }
                                }

                                output
                                    .send((
                                        conn_id.clone(),
                                        format!("URL not seen before, caching and returning."),
                                    ))
                                    .unwrap();

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

    let (send_end, get_end) = futures::sync::oneshot::channel::<()>();

    let server = Server::bind(&in_addr)
        .serve(new_service)
        .with_graceful_shutdown(get_end)
        .map_err(|e| eprintln!("server error: {}", e));

    let mut stdin = async_stdin().bytes();

//    let blocked_list = Arc::clone(&blocked_domains);

    thread::spawn( move || {
        let mut stdout = stdout();
        if termion::is_tty(&stdout) {
            let mut stdout = stdout.lock().into_raw_mode().unwrap();
            let (width, height) = termion::terminal_size().unwrap();
            let mut current_given = Vec::new();
            let mut messages = Vec::new();
            let mut prev_len = 0;
            
            write!(
                stdout,
                "{}{}{}",
                termion::clear::All,
                termion::cursor::Goto(1, 1),
                termion::cursor::Hide
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
                write!(
                    stdout,
                    "{}{}{}{}",
                    termion::cursor::Goto(col, 1),
                    BOX_HORIZONTAL,
                    termion::cursor::Goto(col, height),
                    BOX_HORIZONTAL
                )
                .unwrap();
            }

            for row in 2..height {
                write!(
                    stdout,
                    "{}{}{}{}",
                    termion::cursor::Goto(1, row),
                    BOX_VERTICAL,
                    termion::cursor::Goto(width, row),
                    BOX_VERTICAL
                )
                .unwrap();
            }

            let boundary = (width / 3) * 2;
            write!(
                stdout,
                "{}{}{}{}",
                termion::cursor::Goto(boundary, 1),
                BOX_HORZ_DOWN,
                termion::cursor::Goto(boundary, height),
                BOX_HORZ_UP
            )
            .unwrap();

            for row in 2..height {
                write!(
                    stdout,
                    "{}{}",
                    termion::cursor::Goto(boundary, row),
                    BOX_VERTICAL
                )
                .unwrap();
            }

                write!(
                    stdout,
                    "{} C-ID {} Message{}{}{}{}{}FEBRUOXY - For CS3031",
                    termion::cursor::Goto(2,2),
                    BOX_VERTICAL,
                    termion::cursor::Goto(8,1),
                    BOX_HORZ_DOWN,
                    termion::cursor::Goto(8,height),
                    BOX_HORZ_UP,
                    termion::cursor::Goto(boundary + 1,2)
                ).unwrap();

                write!(
                    stdout,
                    "{}{}{}{}{}{}{}{}",
                    termion::cursor::Goto(1,3),
                    BOX_VERT_RIGHT,
                    BOX_HORIZONTAL.to_string().repeat(6),
                    BOX_VERT_HORZ,
                    BOX_HORIZONTAL.to_string().repeat((boundary - 9) as usize),
                    BOX_VERT_HORZ,
                    BOX_HORIZONTAL.to_string().repeat((width - boundary - 1) as usize),
                    BOX_VERT_LEFT
                ).unwrap();

                write!(
                    stdout,
                    "{}Blocking List:",
                    termion::cursor::Goto(boundary + 2, 4)
                ).unwrap();
/*
                write!(
                    stdout,
                    "{}{:2}: {}{}{:2}:{}",
                    termion::cursor::Goto(boundary + 1, 5),
                    0,
                    blocked_list.lock().unwrap()[0],
                    termion::cursor::Goto(boundary + 1, 6),
                    1,
                    blocked_list.lock().unwrap()[1]
                ).unwrap();
*/
            stdout.flush().unwrap();

            loop {
                let in_byte = stdin.next();
                if in_byte.is_some() {
                    match in_byte.unwrap() {
                        Ok(27) => {
                            writeln!(
                                stdout,
                                "{}{}",
                                termion::cursor::Goto(1, height),
                                termion::cursor::Show
                            )
                            .unwrap();
                            send_end.send(()).unwrap();
                            return;
                        }
                        Ok(127) => {
                            current_given.pop();
                        }
                        Ok(num) => {
                            current_given.push(num);
                        }
                        _ => {
                            writeln!(
                                stdout,
                                "{}{}",
                                termion::cursor::Goto(1, height),
                                termion::cursor::Show
                            )
                            .unwrap();
                            send_end.send(()).unwrap();
                            return;
                        }
                    }
                }

                let get_message = send_in.try_recv();
                match get_message {
                    Ok(recieved) => {
                        messages.push(recieved);
                    }
                    Err(_) => {}
                }

                let current_len = messages.len();
                if { current_len != prev_len } {
                    let mut line_number = 4;
                    while line_number < height && (line_number as usize) < current_len {
                        let (id, message) = if current_len < ((height+1) as usize) {
                            &messages[(line_number - 4) as usize]
                        } else {
                            &messages[(current_len - 1) - (((height) - line_number - 1) as usize)]
                        };
                        let mess_to_print = if message.len() < ((boundary - 14) as usize) {
                            message.clone()
                        } else {
                            let mut trimmed = message.clone();
                            trimmed.truncate((boundary - 14) as usize);
                            trimmed.push_str("...");
                            trimmed
                        };
                        write!(
                            stdout,
                            "{}{}{}{:4}  {}  {}",
                            termion::cursor::Goto(2, line_number),
                            " ".repeat((boundary - 2) as usize),
                            termion::cursor::Goto(2, line_number),
                            id % 10000,
                            BOX_VERTICAL,
                            mess_to_print
                        )
                        .unwrap();
                        line_number += 1;
                    }
                    prev_len = current_len;
                }

                write!(
                    stdout,
                    "{}{}{}{}",
                    termion::cursor::Goto(boundary + 1, height - 1),
                    " ".repeat((width - boundary - 1) as usize),
                    termion::cursor::Goto(boundary + 1, height - 1),
                    std::str::from_utf8(&current_given).unwrap()
                )
                .unwrap();

                stdout.flush().unwrap();
                thread::sleep(Duration::from_millis(20));
            }
        } else {
            for message in send_in {
                writeln!(stdout, "Connection:  {}  -  {}", message.0, message.1).unwrap();
            }
        }
    });

    rt::run(server);
}
