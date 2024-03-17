use anyhow::Context;
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
};

#[derive(Debug)]
struct HttpError {
    status_code: u16,
    error: anyhow::Error,
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)
    }
}

impl std::error::Error for HttpError {}

impl From<anyhow::Error> for HttpError {
    fn from(value: anyhow::Error) -> Self {
        Self {
            status_code: 500,
            error: value,
        }
    }
}

impl HttpError {
    pub fn bad_request(msg: &str) -> Self {
        Self {
            status_code: 400,
            error: anyhow::anyhow!("Bad request: {msg}"),
        }
    }

    pub fn not_found() -> Self {
        Self {
            status_code: 404,
            error: anyhow::anyhow!("Not found"),
        }
    }

    pub fn write_to_stream(&self, stream: &mut TcpStream) -> anyhow::Result<()> {
        write_response(stream, self.status_code, &[], Some(&self.error.to_string()))
            .context("writing http error to stream")
    }
}

fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:4221").context("opening socket")?;

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                if let Err(e) = handle_request(&mut stream) {
                    if let Err(e) = e.write_to_stream(&mut stream) {
                        eprintln!("Error occurred while replying with error response: {e}");
                    }
                }
            }
            Err(e) => {
                println!("error occurred during setting up the connection: {e}");
            }
        }
    }

    Ok(())
}

fn handle_request(stream: &mut TcpStream) -> Result<(), HttpError> {
    println!("accepted new connection");

    let mut request = [0; 1024];
    stream.read(&mut request).context("reading request")?;

    let request = String::from_utf8_lossy(&request);

    println!("read data");

    let (start_line, rest) = request
        .split_once("\r\n")
        .ok_or_else(|| HttpError::bad_request("request header does not contain newline"))?;

    let (headers, _body) = rest.split_once("\r\n\r\n").ok_or_else(|| {
        HttpError::bad_request("request doesn't have a proper split between headers and body")
    })?;

    let path = start_line
        .splitn(3, ' ')
        .nth(1)
        .ok_or_else(|| HttpError::bad_request("request doesn't have a path"))?;

    if path == "/" {
        write_response(stream, 200, &[], None)?;
    } else if let Some(sub_path) = path.strip_prefix("/echo/") {
        let content_length = sub_path.len().to_string();

        write_response(
            stream,
            200,
            &[
                ("Content-Type", "text/plain"),
                ("Content-Length", &content_length),
            ],
            Some(sub_path),
        )?;
    } else if path == "/user-agent" {
        let headers = headers
            .split("\r\n")
            .filter_map(|header| header.split_once(": "))
            .collect::<HashMap<_, _>>();
        let user_agent = headers
            .get("User-Agent")
            .ok_or_else(|| HttpError::bad_request("no user agent header in request"))?;
        let content_length = user_agent.len().to_string();
        write_response(
            stream,
            200,
            &[
                ("Content-Type", "text/plain"),
                ("Content-Length", &content_length),
            ],
            Some(user_agent),
        )?;
    } else {
        println!("does not start with echo/");
        return Err(HttpError::not_found());
    }

    println!("successfully handled request");

    Ok(())
}

fn write_response(
    stream: &mut TcpStream,
    status_code: u16,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> anyhow::Result<()> {
    let headers = headers
        .iter()
        .fold(String::new(), |acc, (k, v)| acc + &format!("{k}: {v}\r\n"));
    let body = body.unwrap_or("");
    write!(stream, "HTTP/1.1 {status_code}\r\n{headers}\r\n{body}")
        .context("writing response to stream")
}
