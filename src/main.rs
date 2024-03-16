use std::io::Write;
use std::net::TcpListener;
use anyhow::Context;


// HTTP/1.1 200 OK\r\n\r\n
fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:4221").unwrap();

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                println!("accepted new connection");
                // stream.write_fmt()
                write!(&mut stream, "HTTP/1.1 200 OK\r\n\r\n").context("sending TCP response")?;
            }
            Err(e) => {
                println!("error: {e}");
            }
        }
    }

    Ok(())
}
