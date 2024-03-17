use anyhow::Context;
use clap::Parser;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
pub(crate) use tokio::{
    fs::File,
    io::BufStream,
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, value_name = "directory", default_value = "./test-files")]
    directory: PathBuf,
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
        write_string_response(stream, self.status_code, &[], Some(&self.error.to_string()))
            .await
            .context("writing http error to stream")
    }
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

    let mut headers = HashMap::new();
    loop {
        let mut header_line = String::new();
        stream
            .read_line(&mut header_line)
            .await
            .context("reading header line")?;

        if header_line.trim().is_empty() {
            break;
        }

        let (k, v) = header_line
            .split_once(':')
            .ok_or_else(|| HttpError::bad_request("invalid header format"))?;
        headers.insert(k.trim().to_owned(), v.trim().to_owned());
    }

    if path == "/" {
        write_string_response(stream, 200, &[], None).await?;
    } else if let Some(sub_path) = path.strip_prefix("/echo/") {
        let content_length = sub_path.len().to_string();

        write_string_response(
            stream,
            200,
            &[
                ("Content-Type", "text/plain"),
                ("Content-Length", &content_length),
            ],
            Some(sub_path),
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
            Some(user_agent),
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
            let path = base_dir.join(filename);
            let mut file = File::create(path).await.context("opening file for write")?;

            tokio::io::copy_buf(stream, &mut file)
                .await
                .context("writing contents to file")?;

            write_string_response(stream, 201, &[], None).await?;
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
    body: Option<&str>,
) -> anyhow::Result<()> {
    write_response_header(stream, status_code, headers).await?;
    let body = body.unwrap_or("");
    stream
        .write_all(format!("{body}").as_bytes())
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
    write_response_header(output_stream, status_code, headers).await?;
    let _ = tokio::io::copy(body_stream, output_stream)
        .await
        .context("streaming byte stream to output stream")?;
    output_stream.flush().await.context("flushing stream")
}

async fn write_response_header(
    stream: &mut (impl AsyncWrite + Unpin),
    status_code: u16,
    headers: &[(&str, &str)],
) -> anyhow::Result<()> {
    let headers = headers
        .iter()
        .fold(String::new(), |acc, (k, v)| acc + &format!("{k}: {v}\r\n"));

    stream
        .write_all(format!("HTTP/1.1 {}\r\n{}\r\n", status_code, headers).as_bytes())
        .await
        .context("writing header to stream")
}
