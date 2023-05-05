use serde::{Deserialize, Serialize};
use std::io;
use std::{collections::HashMap, fmt, ops::Deref, time::Instant};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufStream};
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() -> io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:4488").await?;

    loop {
        let (mut socket, peer_addr) = listener.accept().await?;
        println!("Connected {:?}", peer_addr);
        tokio::spawn(async move {
            loop {
                if socket.readable().await.is_ok() && socket.writable().await.is_ok() {
                    let mut stream = BufStream::new(&mut socket);
                    // Copy data here
                    let keep_alive = handle_connection(&mut stream).await;
                    if !keep_alive {
                        break;
                    }
                }
            }
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

async fn handle_connection<'a>(stream: &'a mut BufStream<&mut TcpStream>) -> bool {
    let now = Instant::now();
    let mut request: Request;

    let mut http_header: Vec<_> = Vec::new();
    match stream.read_until(b'\n', &mut http_header).await {
        Ok(len) => {
            println!("Buf read {} took {} milis.", len, now.elapsed().as_micros());
            if len == 0 {
                return false;
            }
            let mut str = String::from_utf8(http_header).unwrap();
            str = str.trim_end_matches("\r\n").to_string();
            let parts: Vec<_> = str.split(" ").collect();
            if parts.len() != 3 {
                send_response(
                    stream,
                    HTTPCodes::BadRequest,
                    Headers::new(),
                    false,
                    "".to_string(),
                )
                .await;
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
                stream,
                HTTPCodes::BadRequest,
                Headers::new(),
                false,
                "".to_string(),
            )
            .await;
            return false;
        }
    };
    println!("Header line took {} milis.", now.elapsed().as_micros());
    let now = Instant::now();

    loop {
        let mut chunk: Vec<_> = Vec::new();
        let len = stream.read_until(b'\n', &mut chunk).await.unwrap();
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
    println!("Heades took {} milis.", now.elapsed().as_micros());
    let now = Instant::now();
    println!("Request {:?}", request);

    if let Ok(length) = request.headers.get_content_length() {
        let mut body = vec![0; length];
        stream.read(&mut body).await.unwrap();
        request.body = body;
    };
    println!("Body took {} milis.", now.elapsed().as_micros());
    let now = Instant::now();
    let mut headers = Headers::new();
    headers.add("Content-type".to_string(), "application/json".to_string());
    headers.add(
        "Host".to_string(),
        request.headers.get("Host".to_string()).unwrap(),
    );

    send_response(
        stream,
        HTTPCodes::NoContent,
        headers,
        request.keep_alive,
        "".to_string(),
    )
    .await;

    println!("Resp took {} milis.", now.elapsed().as_micros());
    request.keep_alive
}

async fn send_response<'a>(
    stream: &'a mut BufStream<&mut TcpStream>,
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
        .await
        .unwrap();

    for (key, value) in headers.iter() {
        stream
            .write_all(format!("{}: {}\r\n", key, value).as_bytes())
            .await
            .expect(
                format!(
                    "Failed to write header status: {:?} body:{:?}",
                    status_code, body
                )
                .as_str(),
            );
    }
    stream.write_all("\r\n".as_bytes()).await.unwrap();
    if body.len() > 0 {
        stream.write_all(body.as_bytes()).await.unwrap();
    }
    let elapsed_time = now.elapsed();
    println!("Writing took {} seconds.", elapsed_time.as_millis());
    stream.flush().await.unwrap();

    let elapsed_time = now.elapsed();
    println!("Flushing took {} seconds.", elapsed_time.as_millis());
}
