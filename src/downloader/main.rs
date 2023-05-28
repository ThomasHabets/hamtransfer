use ax25::ax25_parser_client::Ax25ParserClient;
use ax25::packet::FrameType::Ui;
use ax25ms::router_service_client::RouterServiceClient;
use ax25ms::StreamRequest;
use futures::{pin_mut, select};
use futures_timer::Delay;
use futures_util::FutureExt;
use futures_util::StreamExt;
use lazy_static::lazy_static;
use rand::Rng;
use regex::Regex;
use std::fs;
use structopt::StructOpt;
use tokio::time::Duration;

//use tokio_stream::StreamExt;

pub mod ax25ms {
    tonic::include_proto!("ax25ms");
}

pub mod ax25 {
    tonic::include_proto!("ax25");
}
pub mod aprs {
    tonic::include_proto!("aprs");
}

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

    // Positional argument.
    roothash: String,
}

/*
* make a UI packet with given payload
*/
async fn make_packet(
    parser: &mut Ax25ParserClient<tonic::transport::Channel>,
    dst: &str,
    src: &str,
    payload: String,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let req = tonic::Request::new(ax25::SerializeRequest {
        packet: Some(ax25::Packet {
            dst: dst.to_string(),
            src: src.to_string(),
            fcs: 0,
            aprs: None,
            repeater: vec![],
            command_response: false,
            command_response_la: true,
            rr_dst1: false,
            rr_extseq: false,
            frame_type: Some(ax25::packet::FrameType::Ui(ax25::packet::Ui {
                pid: 0xF0_i32, // TODO: some protocol ID?
                push: 0,
                payload: payload.into_bytes(),
            })),
        }),
    });
    Ok(parser.serialize(req).await?.into_inner().payload)
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
    stream: &mut tonic::Streaming<ax25ms::Frame>,
    timeout: f32,
) -> Result<Vec<u8>, DownloaderError> {
    let ms = (timeout * 1000.0) as u64;
    let sfut = stream.next().fuse();
    let tfut = Delay::new(Duration::from_millis(ms)).fuse();
    pin_mut!(sfut, tfut);
    select! {
        f = sfut => {
            let frame = f.unwrap().unwrap().payload;
            Ok(frame)
        },
        _ = tfut => {
            println!("Timeout!");
            Err(DownloaderError::Timeout)
        }
    }
}

async fn receive_streamed_block(
    decoder: &mut raptor_code::SourceBlockDecoder,
    client: &mut RouterServiceClient<tonic::transport::Channel>,
    parser: &mut Ax25ParserClient<tonic::transport::Channel>,
    size: usize,                // Only needed for progress bar.
    bytes_received: &mut usize, // Only needed for progress bar.
    packet_loss: f32,
    timeout: f32,
) -> Result<usize, DownloaderError> {
    println!("Starting stream…");
    let mut stream = client
        .stream_frames(StreamRequest {})
        .await
        .unwrap()
        .into_inner();

    println!("Awaiting data…");
    let mut encoding_symbol_length = 0;
    let mut rng = rand::thread_rng();
    while !decoder.fully_specified() {
        // Get frame.
        let frame = receive_frame(&mut stream, timeout).await?;

        if rng.gen::<f32>() < packet_loss {
            continue;
        }

        //
        // Parse UI frame.
        //
        let parsed = parser
            .parse(tonic::Request::new(ax25::ParseRequest { payload: frame }))
            .await?
            .into_inner()
            .packet
            .expect("surely the RPC reply has a packet");

        let ui = match parsed.frame_type {
            Some(Ui(ui)) => ui,
            _ => continue,
        };
        let encoding_symbol = ui.payload;
        *bytes_received += encoding_symbol.len();
        println!(
            "Got id {} size {}: Total {} = {}%",
            ui.pid,
            encoding_symbol.len(),
            bytes_received,
            100 * *bytes_received / size
        );

        encoding_symbol_length = encoding_symbol.len();
        let esi = ui.pid as u32;
        decoder.push_encoding_symbol(&encoding_symbol, esi);
    }
    Ok(encoding_symbol_length)
}

/*
* Request a block, until fully received.
*/
async fn download_block(
    opt: &Opt,
    mut client: RouterServiceClient<tonic::transport::Channel>,
    mut parser: Ax25ParserClient<tonic::transport::Channel>,
    hash: &str,
    size: usize,
    source_block_size: usize,
) -> Result<Vec<u8>, DownloaderError> {
    let mut decoder = raptor_code::SourceBlockDecoder::new(source_block_size);
    let tag = 1234_u16; // TODO
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
            &mut client,
            &mut parser,
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
                println!("Requesting more");
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
    println!("Downloaded!");
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

async fn get_meta(
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
    let mut stream = client
        .stream_frames(StreamRequest {})
        .await
        .unwrap()
        .into_inner();
    loop {
        let frame = receive_frame(&mut stream, timeout).await?;
        let parsed = parser
            .parse(tonic::Request::new(ax25::ParseRequest { payload: frame }))
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
}

#[tokio::main]
async fn main() -> Result<(), DownloaderError> {
    let opt = Opt::from_args();

    println!("Connecting…");
    let mut client = RouterServiceClient::connect(opt.router.clone()).await?;
    let mut parser = Ax25ParserClient::connect(opt.parser.clone()).await?;

    println!("Getting metadata…");

    let (source_block_size, total_size) = get_meta(
        &mut client,
        &mut parser,
        &opt.dst,
        &opt.source,
        &opt.roothash,
        opt.timeout,
    )
    .await?;
    println!("Source block size: {}", source_block_size);
    println!("Total size: {}", total_size);

    println!("Getting data…");
    let source_block = download_block(
        &opt,
        client,
        parser,
        &opt.roothash,
        total_size,
        source_block_size,
    )
    .await?;

    println!("Downloaded size {:?}", source_block.len());
    fs::write(opt.output, source_block).expect("write block");
    Ok(())
}
