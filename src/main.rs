use anyhow::Context;
use std::io::{Read, Write};
use std::net::TcpListener;

// HTTP/1.1 200 OK\r\n\r\n

/*
GET /index.html HTTP/1.1
Host: localhost:4221
User-Agent: curl/7.64.1
 */
fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:4221").unwrap();

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                println!("accepted new connection");

                let mut request = [0; 1024];
                stream.read(&mut request).context("reading request")?;

                let request = String::from_utf8_lossy(&request);

                println!("read data");

                let Some((start_line, _)) = request.split_once("\r\n") else {
                    anyhow::bail!("request doesn't have a newline");
                };

                let Some(path) = start_line.splitn(3, ' ').nth(1) else {
                    anyhow::bail!("request doesn't have a path");
                };

                let status_code = if path == "/" { 200 } else { 404 };

                write!(&mut stream, "HTTP/1.1 {status_code} OK\r\n\r\n")
                    .context("sending TCP response")?;
                println!("after response");
            }
            Err(e) => {
                println!("error: {e}");
            }
        }
    }

    Ok(())
}
