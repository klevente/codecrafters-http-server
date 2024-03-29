use anyhow::Context;
use clap::Parser;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufStream},
    net::{TcpListener, TcpStream},
};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, value_name = "directory", default_value = "./test-files")]
    directory: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let base_dir = Arc::new(args.directory);

    let listener = TcpListener::bind("127.0.0.1:4221")
        .await
        .context("opening socket")?;

    loop {
        match listener.accept().await {
            Ok((stream, _)) => spawn_handler(BufStream::new(stream), base_dir.clone()),
            Err(e) => println!("error occurred during setting up the connection: {e}"),
        }
    }
}

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

    pub fn method_not_allowed(method: &str) -> Self {
        Self {
            status_code: 405,
            error: anyhow::anyhow!("Method {method} not allowed"),
        }
    }

    pub async fn write_to_stream(
        &self,
        stream: &mut (impl AsyncWrite + Unpin),
    ) -> anyhow::Result<()> {
        write_string_response(stream, self.status_code, &[], &self.error.to_string())
            .await
            .context("writing http error to stream")
    }
}

fn spawn_handler(stream: BufStream<TcpStream>, base_dir: Arc<PathBuf>) {
    tokio::spawn(async move {
        let mut stream = stream;
        if let Err(e) = handle_request(&mut stream, base_dir).await {
            if let Err(e) = e.write_to_stream(&mut stream).await {
                println!("Error occurred while replying with error response: {e}");
            }
        }
    });
}

async fn handle_request<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut BufStream<S>,
    base_dir: Arc<PathBuf>,
) -> Result<(), HttpError> {
    println!("accepted new connection");

    let mut request_line = String::new();
    stream
        .read_line(&mut request_line)
        .await
        .context("reading request line")?;

    let mut request_line_parts = request_line.trim().splitn(3, ' ');
    let method = request_line_parts
        .next()
        .ok_or_else(|| HttpError::bad_request("no method found in header"))?;

    let path = request_line_parts
        .next()
        .ok_or_else(|| HttpError::bad_request("no path found in header"))?;

    let standard = request_line_parts
        .next()
        .ok_or_else(|| HttpError::bad_request("no standard found in header"))?;

    println!("Incoming request: {method} {path} [{standard}]");

    let mut header_lines = Vec::new();
    loop {
        let mut header_line = String::new();
        stream
            .read_line(&mut header_line)
            .await
            .context("reading header line")?;

        if header_line.trim().is_empty() {
            break;
        }

        header_lines.push(header_line);
    }

    let mut headers = HashMap::with_capacity(header_lines.len());
    for (i, _) in header_lines.iter().enumerate() {
        let (k, v) = header_lines[i]
            .split_once(':')
            .ok_or_else(|| HttpError::bad_request("invalid header format"))?;
        headers.insert(k.trim(), v.trim());
    }

    println!("Got {} headers", headers.len());

    if path == "/" {
        write_header_only_response(stream, 200, &[]).await?;
    } else if let Some(sub_path) = path.strip_prefix("/echo/") {
        let content_length = sub_path.len().to_string();

        write_string_response(
            stream,
            200,
            &[
                ("Content-Type", "text/plain"),
                ("Content-Length", &content_length),
            ],
            sub_path,
        )
        .await?;
    } else if path == "/user-agent" {
        let user_agent = headers
            .get("User-Agent")
            .ok_or_else(|| HttpError::bad_request("no user agent header in request"))?;
        let content_length = user_agent.len().to_string();
        write_string_response(
            stream,
            200,
            &[
                ("Content-Type", "text/plain"),
                ("Content-Length", &content_length),
            ],
            user_agent
        )
        .await?;
    } else if let Some(filename) = path.strip_prefix("/files/") {
        if method == "GET" {
            let path = base_dir.join(filename);
            let mut file = File::open(path).await.map_err(|_| HttpError::not_found())?;
            let metadata = file.metadata().await.context("reading file metadata")?;
            let file_length = metadata.len().to_string();
            write_byte_stream_response(
                stream,
                200,
                &[
                    ("Content-Type", "application/octet-stream"),
                    ("Content-Length", &file_length),
                ],
                &mut file,
            )
            .await?;
        } else if method == "POST" {
            let content_length = headers
                .get("Content-Length")
                .map(|v| v.parse::<u64>().ok())
                .flatten()
                .ok_or_else(|| HttpError::bad_request("No valid Content-Length was provided"))?;
            let path = base_dir.join(filename);
            let mut file = File::create(path).await.context("opening file for write")?;

            let mut stream_limited = stream.take(content_length);
            tokio::io::copy_buf(&mut stream_limited, &mut file)
                .await
                .context("writing contents to file")?;

            write_header_only_response(stream, 201, &[]).await?;
        } else {
            return Err(HttpError::method_not_allowed(method));
        }
    } else {
        println!("No routes were matched, returning 404");
        return Err(HttpError::not_found());
    }

    Ok(())
}

async fn write_string_response(
    stream: &mut (impl AsyncWrite + Unpin),
    status_code: u16,
    headers: &[(&str, &str)],
    body: &str,
) -> anyhow::Result<()> {
    write_header_only_response(stream, status_code, headers).await?;
    stream.write_all(body.as_bytes())
        .await
        .context("writing body to stream")?;
    stream.flush().await.context("flushing stream")
}

async fn write_byte_stream_response(
    output_stream: &mut (impl AsyncWrite + Unpin),
    status_code: u16,
    headers: &[(&str, &str)],
    body_stream: &mut (impl AsyncRead + Unpin),
) -> anyhow::Result<()> {
    write_header_only_response(output_stream, status_code, headers).await?;
    let _ = tokio::io::copy(body_stream, output_stream)
        .await
        .context("streaming byte stream to output stream")?;
    output_stream.flush().await.context("flushing stream")
}

async fn write_header_only_response(
    stream: &mut (impl AsyncWrite + Unpin),
    status_code: u16,
    headers: &[(&str, &str)],
) -> anyhow::Result<()> {
    let status_code = status_code.to_string();

    stream.write_all(b"HTTP/1.1 ").await.context("writing http standard to stream")?;
    stream.write_all(status_code.as_bytes()).await.context("writing status code to stream")?;
    stream.write_all(b"\r\n").await.context("writing first newline to stream")?;

    for (k, v) in headers {
        stream.write_all(k.as_bytes()).await.context("writing header key to stream")?;
        stream.write_all(b": ").await.context("writing header separator to stream")?;
        stream.write_all(v.as_bytes()).await.context("writing header value to stream")?;
        stream.write_all(b"\r\n").await.context("writing header newline to stream")?;
    }

    stream.write_all(b"\r\n").await.context("writing final header newline to stream")
}
