use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    ops::Deref,
    time::Instant,
};
use thiserror::Error;
use threadpool::ThreadPool;

fn main() {
    let listener = TcpListener::bind("0.0.0.0:4488").unwrap();
    let pool = ThreadPool::new(10);

    for stream in listener.incoming() {
        pool.execute(move || {
            let stream = stream.unwrap();
            let buf_reader = BufReader::new(&stream);
            println!("Connected {:?}", stream.peer_addr());
            handle_connection(&stream, buf_reader);
        });
    }
}

#[derive(Debug)]
pub enum HTTPCodes {
    OK,
    NoContent,
    BadRequest,
}
impl HTTPCodes {
    fn as_str(&self) -> &'static str {
        match self {
            Self::OK => "200 OK",
            Self::NoContent => "204 No content",
            Self::BadRequest => "400 Bad Request",
        }
    }
}
#[derive(Error, Debug)]
pub enum HTTPError {
    #[error("Error parsing header {header:?}")]
    ParsingError { header: String },
    #[error("missing header {header:?} error")]
    MissingHeader { header: String },
    #[error("unknown data store error")]
    Unknown,
}

#[derive(Debug, Serialize, Deserialize)]
struct Test {
    name: String,
}

#[derive(Default)]
struct Headers {
    kv: HashMap<String, String>,
    raw: HashMap<String, String>,
}

const CONTENT_LENGTH_HEADER: &str = "Content-Length";
const CONNECTION_HEADER: &str = "Connection";

impl Headers {
    pub fn new() -> Headers {
        Headers {
            kv: HashMap::new(),
            raw: HashMap::new(),
        }
    }

    fn add(&mut self, key: String, value: String) {
        self.kv.insert(key.to_lowercase(), value.clone());
        self.raw.insert(key, value);
    }

    pub fn get(&self, key: String) -> Option<String> {
        match self.kv.get(&key.to_lowercase()) {
            Some(v) => Some(v.deref().to_string()),
            None => None,
        }
    }

    fn get_content_length(&self) -> Result<usize, HTTPError> {
        match self.get(CONTENT_LENGTH_HEADER.to_string()) {
            Some(length) => match length.parse::<usize>() {
                Ok(l) => Result::Ok(l),
                Err(_) => Result::Err(HTTPError::MissingHeader {
                    header: CONTENT_LENGTH_HEADER.to_string(),
                }),
            },
            None => Result::Err(HTTPError::ParsingError {
                header: CONTENT_LENGTH_HEADER.to_string(),
            }),
        }
    }

    pub fn iter(&self) -> std::collections::hash_map::Iter<String, String> {
        self.raw.iter()
    }
}

impl fmt::Debug for Headers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Headers {:?}", self.raw)
    }
}

#[derive(Debug, Default)]
struct Request {
    method: String,
    path: String,
    version: String,
    headers: Headers,
    body: Vec<u8>,
    keep_alive: bool,
}

fn handle_connection(stream: &TcpStream, mut buf_reader: BufReader<&TcpStream>) -> bool {
    let now = Instant::now();
    let mut request: Request;

    let mut http_header: Vec<_> = Vec::new();
    match buf_reader.read_until(b'\n', &mut http_header) {
        Ok(len) => {
            println!("Buf read took {} milis.", now.elapsed().as_millis());
            if len == 0 {
                return false;
            }
            let mut str = String::from_utf8(http_header).unwrap();
            str = str.trim_end_matches("\r\n").to_string();
            let parts: Vec<_> = str.split(" ").collect();
            if parts.len() != 3 {
                send_response(
                    &stream,
                    HTTPCodes::BadRequest,
                    Headers::new(),
                    false,
                    "".to_string(),
                );
                return false;
            }
            request = Request {
                method: parts[0].to_string(),
                path: parts[1].to_string(),
                version: parts[2].to_string(),
                headers: Headers::new(),
                body: Vec::new(),
                keep_alive: true,
            }
        }
        Err(_) => {
            send_response(
                &stream,
                HTTPCodes::BadRequest,
                Headers::new(),
                false,
                "".to_string(),
            );
            return false;
        }
    };
    println!("Header line took {} milis.", now.elapsed().as_millis());
    let now = Instant::now();

    loop {
        let mut chunk: Vec<_> = Vec::new();
        let len = buf_reader.read_until(b'\n', &mut chunk).unwrap();
        if len <= 2 {
            break;
        }

        let mut str = String::from_utf8(chunk).unwrap();
        str = str.trim_end_matches("\r\n").to_string();
        let parts: Vec<_> = str.split(": ").collect();
        if parts.len() != 2 {
            continue;
        }
        let key = parts[0].to_string();
        let value = parts[1].to_string();

        request.headers.add(key.clone(), value.clone());
        if key == CONNECTION_HEADER.to_string() && value == "close" {
            request.keep_alive = false;
        }
    }
    println!("Heades took {} milis.", now.elapsed().as_millis());
    let now = Instant::now();
    println!("Request {:?}", request);

    if let Ok(length) = request.headers.get_content_length() {
        let mut body = vec![0; length];
        buf_reader.read_exact(&mut body).unwrap();
        request.body = body;
    };
    println!("Body took {} milis.", now.elapsed().as_millis());
    let now = Instant::now();
    let mut headers = Headers::new();
    // headers.add("Content-type".to_string(), "application/json".to_string());
    headers.add(
        "Host".to_string(),
        request.headers.get("Host".to_string()).unwrap(),
    );

    send_response(
        &stream,
        HTTPCodes::NoContent,
        headers,
        request.keep_alive,
        // "{\"name\": \"dick\"}".to_string(),
        "".to_string(),
    );

    println!("Resp took {} milis.", now.elapsed().as_millis());
    if request.keep_alive {
        handle_connection(stream, buf_reader)
    } else {
        false
    }
}

fn send_response(
    mut stream: &TcpStream,
    status_code: HTTPCodes,
    mut headers: Headers,
    keep_alive: bool,
    body: String,
) {
    let now = Instant::now();
    headers.add("Content-Length".to_string(), body.len().to_string());
    headers.add(
        "Content-Type".to_string(),
        "text/html; charset=utf-8".to_string(),
    );
    headers.add(
        "Connection".to_string(),
        if keep_alive { "keep-alive" } else { "close" }.to_string(),
    );

    stream
        .write_all(format!("HTTP/1.1 {}\r\n", status_code.as_str()).as_bytes())
        .unwrap();

    for (key, value) in headers.iter() {
        stream
            .write_all(format!("{}: {}\r\n", key, value).as_bytes())
            .expect(
                format!(
                    "Failed to write header status: {:?} body:{:?}",
                    status_code, body
                )
                .as_str(),
            );
    }
    stream.write_all("\r\n".as_bytes()).unwrap();
    if body.len() > 0 {
        stream.write_all(body.as_bytes()).unwrap();
    }
    let elapsed_time = now.elapsed();
    println!("Writing took {} seconds.", elapsed_time.as_millis());
    stream.flush().unwrap();

    let elapsed_time = now.elapsed();
    println!("Flushing took {} seconds.", elapsed_time.as_millis());
}
