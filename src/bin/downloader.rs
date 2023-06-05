use ax25::ax25_parser_client::Ax25ParserClient;
use ax25::packet::FrameType::Ui;
use ax25ms::router_service_client::RouterServiceClient;
use ax25ms::StreamRequest;
use futures::{pin_mut, select};
use futures_timer::Delay;
use futures_util::FutureExt;
use futures_util::StreamExt;
use lazy_static::lazy_static;
use lib::{ax25, ax25ms, make_packet};
use log::{debug, info, warn};
use rand::Rng;
use regex::Regex;
use std::fs;
use structopt::StructOpt;
use tokio::sync::mpsc;
use tokio::time::Duration;

#[derive(StructOpt, Debug)]
#[structopt()]
struct Opt {
    #[structopt(short = "r", long = "router")]
    router: String,
    #[structopt(short = "p", long = "parser")]
    parser: String,

    #[structopt(short = "s", long = "source")]
    source: String,

    #[structopt(short = "o", long = "output")]
    output: String,

    #[structopt(short = "d", long = "dst", default_value = "CQ")]
    dst: String,

    #[structopt(long = "packet_loss", default_value = "0.0")]
    packet_loss: f32,

    #[structopt(long = "timeout", default_value = "2.0")]
    timeout: f32,

    #[structopt(short = "l", long = "list")]
    list: bool,

    // Positional argument.
    roothash: String,
}

async fn request_block(
    client: &mut RouterServiceClient<tonic::transport::Channel>,
    parser: &mut Ax25ParserClient<tonic::transport::Channel>,
    dst: &str,
    src: &str,
    hash: &str,
    tag: u16,
    existing: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let cmd = make_packet(
        parser,
        dst,
        src,
        format!("G {} 0 {} {}", tag, existing, hash).to_string(),
    )
    .await?;
    client
        .send(tonic::Request::new(ax25ms::SendRequest {
            frame: Some(ax25ms::Frame { payload: cmd }),
        }))
        .await?;
    Ok(())
}

async fn receive_frame(
    stream: &mut mpsc::Receiver<ax25ms::Frame>,
    timeout: f32,
) -> Result<Vec<u8>, DownloaderError> {
    let ms = (timeout * 1000.0) as u64;
    let sfut = stream.recv().fuse();
    let tfut = Delay::new(Duration::from_millis(ms)).fuse();
    pin_mut!(sfut, tfut);
    select! {
        f = sfut => {
            let frame = f.unwrap().payload;
            Ok(frame)
        },
        _ = tfut => {
            warn!("Timeout!");
            Err(DownloaderError::Timeout)
        }
    }
}

async fn receive_streamed_block(
    decoder: &mut raptor_code::SourceBlockDecoder,
    stream: &mut mpsc::Receiver<ax25ms::Frame>,
    parser: &mut Ax25ParserClient<tonic::transport::Channel>,
    tag: u16,
    size: usize,                // Only needed for progress bar.
    bytes_received: &mut usize, // Only needed for progress bar.
    packet_loss: f32,
    timeout: f32,
) -> Result<usize, DownloaderError> {
    info!("Awaiting data…");
    let mut encoding_symbol_length = 0;
    let mut rng = rand::thread_rng();
    while !decoder.fully_specified() {
        // Get frame.
        let frame = receive_frame(stream, timeout).await?;

        if rng.gen::<f32>() < packet_loss {
            continue;
        }

        //
        // Parse UI frame.
        //
        let parsed = parser
            .parse(tonic::Request::new(ax25::ParseRequest {
                payload: frame,
                check_fcs: true,
            }))
            .await?
            .into_inner()
            .packet
            .expect("surely the RPC reply has a packet");

        let ui = match parsed.frame_type {
            Some(Ui(ui)) => ui,
            _ => continue,
        };
        let encoding_symbol = &ui.payload[4..ui.payload.len()];
        *bytes_received += encoding_symbol.len();
        let rcv_tag = u16::from_be_bytes(ui.payload[0..2].try_into().unwrap());
        if tag != rcv_tag {
            continue;
        }
        let esi = u16::from_be_bytes(ui.payload[2..4].try_into().unwrap());

        info!(
            "Got id {} size {}: Total {} = {}%",
            esi,
            encoding_symbol.len(),
            bytes_received,
            100 * *bytes_received / size
        );

        encoding_symbol_length = encoding_symbol.len();
        decoder.push_encoding_symbol(encoding_symbol, esi as u32);
    }
    Ok(encoding_symbol_length)
}

/*
* Request a block, until fully received.
*/
async fn download_block(
    opt: &Opt,
    stream: &mut mpsc::Receiver<ax25ms::Frame>,
    mut client: RouterServiceClient<tonic::transport::Channel>,
    mut parser: Ax25ParserClient<tonic::transport::Channel>,
    hash: &str,
    size: usize,
    source_block_size: usize,
) -> Result<Vec<u8>, DownloaderError> {
    let mut decoder = raptor_code::SourceBlockDecoder::new(source_block_size);
    let tag = rand::thread_rng().gen::<u16>();
    request_block(
        &mut client,
        &mut parser,
        &opt.dst,
        &opt.source,
        hash,
        tag,
        0,
    )
    .await?;

    let mut bytes_done = 0_usize;
    let len;
    loop {
        match receive_streamed_block(
            &mut decoder,
            stream,
            &mut parser,
            tag,
            size,
            &mut bytes_done,
            opt.packet_loss,
            opt.timeout,
        )
        .await
        {
            Ok(l) => {
                len = l;
                break;
            }
            Err(DownloaderError::Timeout) => {
                debug!("Requesting more");
                request_block(
                    &mut client,
                    &mut parser,
                    &opt.dst,
                    &opt.source,
                    hash,
                    tag,
                    bytes_done,
                )
                .await?;
                continue;
            }
            Err(e) => {
                return Err(e);
            }
        }
    }
    info!("Downloaded!");
    let mut source_block = decoder.decode(len * source_block_size).expect("decode");
    source_block.resize(size, 0); // Will only ever shrink.

    let digest = sha256::digest(&source_block[..]);
    if digest != hash {
        return Err(DownloaderError::ChecksumMismatch(digest, hash.to_string()));
    }
    Ok(source_block)
}

#[derive(Debug)]
enum DownloaderError {
    RPCError(tonic::transport::Error),
    RPCStatusError(tonic::Status),
    StreamError(Box<dyn std::error::Error>),
    ChecksumMismatch(String, String),
    Timeout,
}
impl From<Box<dyn std::error::Error>> for DownloaderError {
    fn from(error: Box<dyn std::error::Error>) -> Self {
        DownloaderError::StreamError(error)
    }
}
impl From<tonic::transport::Error> for DownloaderError {
    fn from(error: tonic::transport::Error) -> Self {
        DownloaderError::RPCError(error)
    }
}
impl From<tonic::Status> for DownloaderError {
    fn from(error: tonic::Status) -> Self {
        DownloaderError::RPCStatusError(error)
    }
}

impl std::fmt::Display for DownloaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::RPCError(e) => write!(f, "RPC Error: {e}"),
            Self::RPCStatusError(e) => write!(f, "RPC status Error: {e}"),
            Self::StreamError(e) => write!(f, "Stream Error: {e}"),
            Self::ChecksumMismatch(chk1, chk2) => write!(f, "Checksum Mismatch: {chk1} != {chk2}"),
            Self::Timeout => write!(f, "Got timeout :-("),
        }
    }
}

async fn list(
    stream: &mut mpsc::Receiver<ax25ms::Frame>,
    client: &mut RouterServiceClient<tonic::transport::Channel>,
    parser: &mut Ax25ParserClient<tonic::transport::Channel>,
    dst: &str,
    src: &str,
    timeout: f32,
) -> Result<(), DownloaderError> {
    let mut rng = rand::thread_rng();
    let tag = rng.gen::<u16>();
    let cmd = make_packet(parser, dst, src, format!("L {}", tag).to_string()).await?;
    client
        .send(tonic::Request::new(ax25ms::SendRequest {
            frame: Some(ax25ms::Frame { payload: cmd }),
        }))
        .await?;
    loop {
        let frame = receive_frame(stream, timeout).await?;
        debug!("List got some frame");
        let parsed = parser
            .parse(tonic::Request::new(ax25::ParseRequest {
                payload: frame,
                check_fcs: true,
            }))
            .await?
            .into_inner()
            .packet
            .expect("surely the RPC reply has a packet");
        let ui = match parsed.frame_type {
            Some(ax25::packet::FrameType::Ui(ui)) => ui,
            _ => {
                debug!("Not an UI frame");
                continue;
            }
        };
        let reply = match std::str::from_utf8(&ui.payload) {
            Ok(x) => x,
            _ => {
                debug!("Not UTF8");
                continue;
            }
        };
        if reply == format!("l {}", tag) {
            return Ok(());
        }
        let m = match LIST_REPLY_RE.captures(reply) {
            Some(x) => x,
            None => {
                debug!("Not a list reply");
                continue;
            }
        };
        if m[1] != format!("{}", tag) {
            debug!("Wrong tag");
            continue;
        }
        let list = reply.lines().collect::<Vec<&str>>();
        println!("List reply:\n");
        for entry in &list[1..] {
            println!("{:?}", entry);
        }
    }
}

async fn get_meta(
    stream: &mut mpsc::Receiver<ax25ms::Frame>,
    client: &mut RouterServiceClient<tonic::transport::Channel>,
    parser: &mut Ax25ParserClient<tonic::transport::Channel>,
    dst: &str,
    src: &str,
    hash: &str,
    timeout: f32,
) -> Result<(usize, usize), DownloaderError> {
    let cmd = make_packet(parser, dst, src, format!("M {}", hash).to_string()).await?;
    client
        .send(tonic::Request::new(ax25ms::SendRequest {
            frame: Some(ax25ms::Frame { payload: cmd }),
        }))
        .await?;
    loop {
        let frame = receive_frame(stream, timeout).await?;
        let parsed = parser
            .parse(tonic::Request::new(ax25::ParseRequest {
                payload: frame,
                check_fcs: true,
            }))
            .await?
            .into_inner()
            .packet
            .expect("surely the RPC reply has a packet");
        let ui = match parsed.frame_type {
            Some(ax25::packet::FrameType::Ui(ui)) => ui,
            _ => continue,
        };
        let reply = match std::str::from_utf8(&ui.payload) {
            Ok(x) => x,
            _ => {
                continue;
            }
        };
        let m = match META_REPLY_RE.captures(reply) {
            Some(x) => x,
            None => continue,
        };
        if m[1] != *hash {
            continue;
        }
        let block = match m[2].parse::<usize>() {
            Ok(x) => x,
            _ => continue,
        };
        let size = match m[3].parse::<usize>() {
            Ok(x) => x,
            _ => continue,
        };
        return Ok((block, size));
    }
}

lazy_static! {
    static ref META_REPLY_RE: Regex = Regex::new(r"m (\w+) (\d+) (\d+)").unwrap();
    static ref LIST_REPLY_RE: Regex = Regex::new(r"(?m)l (\d+)\n.*").unwrap();
}

fn start_stream(
    mut client: RouterServiceClient<tonic::transport::Channel>,
) -> tokio::sync::mpsc::Receiver<ax25ms::Frame> {
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(async move {
        let mut stream = client
            .stream_frames(StreamRequest {})
            .await
            .unwrap()
            .into_inner();
        while let Some(item) = stream.next().await {
            tx.send(item.unwrap()).await.unwrap();
        }
    });
    rx
}

#[tokio::main]
async fn main() -> Result<(), DownloaderError> {
    let opt = Opt::from_args();

    stderrlog::new()
        .module(module_path!())
        .quiet(false)
        .verbosity(3)
        .timestamp(stderrlog::Timestamp::Second)
        .init()
        .unwrap();

    info!("Connecting…");
    // TODO: merge clients.
    let mut client = RouterServiceClient::connect(opt.router.clone()).await?;
    let stream_client = RouterServiceClient::connect(opt.router.clone()).await?;
    let mut parser = Ax25ParserClient::connect(opt.parser.clone()).await?;

    info!("Getting metadata…");

    let mut stream = start_stream(stream_client);

    if opt.list {
        list(
            &mut stream,
            &mut client,
            &mut parser,
            &opt.dst,
            &opt.source,
            opt.timeout,
        )
        .await?;
        return Ok(());
    }
    let (source_block_size, total_size) = get_meta(
        &mut stream,
        &mut client,
        &mut parser,
        &opt.dst,
        &opt.source,
        &opt.roothash,
        opt.timeout,
    )
    .await?;
    info!("Source block size: {}", source_block_size);
    info!("Total size: {}", total_size);

    info!("Getting data…");
    let source_block = download_block(
        &opt,
        &mut stream,
        client,
        parser,
        &opt.roothash,
        total_size,
        source_block_size,
    )
    .await?;

    info!("Downloaded size {:?}", source_block.len());
    fs::write(opt.output, source_block).expect("write block");
    Ok(())
}
