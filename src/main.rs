use anyhow::Context;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

/*
GET /echo/abc HTTP/1.1
Host: localhost:4221
User-Agent: curl/7.64.1
 */
fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:4221").context("opening socket")?;

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                handle_request(stream)?;
            }
            Err(e) => {
                println!("error occurred during setting up the connection: {e}");
            }
        }
    }

    Ok(())
}

fn handle_request(mut stream: TcpStream) -> anyhow::Result<()> {
    println!("accepted new connection");

    let mut request = [0; 1024];
    stream.read(&mut request).context("reading request")?;

    let request = String::from_utf8_lossy(&request);

    println!("read data");

    let Some((start_line, rest)) = request.split_once("\r\n") else {
        anyhow::bail!("request doesn't have a newline");
    };

    let Some((headers, _body)) = rest.split_once("\r\n\r\n") else {
        anyhow::bail!("request doesn't have a proper split between headrees and body");
    };

    let Some(path) = start_line.splitn(3, ' ').nth(1) else {
        anyhow::bail!("request doesn't have a path");
    };

    if path == "/" {
        write_response(&mut stream, 200, &[], None)?;
    } else if let Some(sub_path) = path.strip_prefix("/echo/") {
        let content_length = sub_path.len().to_string();

        write_response(
            &mut stream,
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
        let Some(user_agent) = headers.get("User-Agent") else {
            anyhow::bail!("no user agent found");
        };
        let content_length = user_agent.len().to_string();
        write_response(
            &mut stream,
            200,
            &[
                ("Content-Type", "text/plain"),
                ("Content-Length", &content_length),
            ],
            Some(user_agent),
        )?;
    } else {
        println!("does not start with echo/");
        write_response(&mut stream, 404, &[], None)?;
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
