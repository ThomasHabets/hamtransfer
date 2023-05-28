use async_std::task;
use ax25::ax25_parser_client::Ax25ParserClient;
use ax25::packet::Ui;
use ax25::Packet;
use ax25::SerializeRequest;
use ax25ms::router_service_client::RouterServiceClient;
use ax25ms::Frame;
use ax25ms::SendRequest;
use lazy_static::lazy_static;
use rand::prelude::SliceRandom;
use rand::thread_rng;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::time::Duration;
use structopt::StructOpt;
use tokio_stream::StreamExt;

pub mod ax25ms {
    // The string specified here must match the proto package name
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

    #[structopt(short = "i", long = "input")]
    input: String,

    #[structopt(short = "s", long = "packet-size", default_value = "200")]
    size: usize,

    #[structopt(short = "R", long = "repair", default_value = "50")]
    repair: usize,

    #[structopt(short = "S", long = "source")]
    source: String,
}

fn float_to_usize(f: f64) -> Option<usize> {
    let ret = f as usize;
    let back = ret as f64;
    if back != f {
        return None;
    }
    Some(ret)
}

fn usize_to_float(f: usize) -> Option<f64> {
    let ret = f as f64;
    let back = ret as usize;
    if back != f {
        return None;
    }
    Some(ret)
}

use std::str;
async fn get_request(
    stream: &mut tonic::Streaming<Frame>,
    parser: &mut Ax25ParserClient<tonic::transport::Channel>,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    loop {
        let frame = stream.next().await.unwrap().unwrap().payload;
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

        let cmd = match str::from_utf8(&ui.payload) {
            Ok(x) => x,
            _ => {
                //println!("Invalid request: {:?}", ui.payload);
                continue;
            }
        };
        return Ok((parsed.src, cmd.to_string()));
    }
}

fn max_source_syms(size: usize, packet_size: usize) -> usize {
    let f = (usize_to_float(size).unwrap() / usize_to_float(packet_size).unwrap()).ceil();
    float_to_usize(f).unwrap()
}

async fn transmit(
    client: &mut RouterServiceClient<tonic::transport::Channel>,
    parser: &mut Ax25ParserClient<tonic::transport::Channel>,
    dst: &str,
    src: String,
    _tag: String, // TODO: add to protocol.
    packet_size: usize,
    nb_repair: u32,
    source_data: Vec<u8>,
    packets: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let len = source_data.len();

    let max_source_symbols = max_source_syms(source_data.len(), packet_size);

    println!("Max source symbols: {}", max_source_symbols);
    println!("Total len: {}", len);
    // State that needs to be sent:
    // * max_source_symbols
    // * total size
    // * packet_size is implicitly sent by seeing the block size.

    let mut encoder = raptor_code::SourceBlockEncoder::new(&source_data, max_source_symbols);
    let n = encoder.nb_source_symbols() + nb_repair;

    // Transmit RPC.

    println!("Total chunks: {}", n);
    let mut txlist: Vec<u32> = (0..n).step_by(1).collect();
    txlist.shuffle(&mut thread_rng());

    let mut transmitted = 0;
    for esi in txlist {
        transmitted += 1;
        if transmitted > packets {
            return Ok(());
        }
        let encoding_symbol = encoder.fountain(esi);
        let len = encoding_symbol.len();

        // Test code that writes it to disk.
        //
        //println!("{:?}", encoding_symbol.len());
        fs::write(format!("tmp/{}", esi), &encoding_symbol).expect("write block");

        // TODO: surely we can default these values?
        let request = tonic::Request::new(SerializeRequest {
            packet: Some(Packet {
                dst: dst.to_string(),
                src: src.clone(),
                fcs: 0,
                aprs: None,
                repeater: vec![],
                command_response: false,
                command_response_la: true,
                rr_dst1: false,
                rr_extseq: false,
                frame_type: Some(ax25::packet::FrameType::Ui(Ui {
                    pid: esi as i32, // TODO: move into payload.
                    push: 0,
                    payload: encoding_symbol,
                })),
            }),
        });
        let serd = parser.serialize(request).await?;

        let request = tonic::Request::new(SendRequest {
            frame: Some(Frame {
                payload: serd.into_inner().payload,
            }),
        });
        client.send(request).await?;
        //println!("Sent esi {} of size {}", esi, &len);
        if true {
            let millis: u64 = 8000 * len as u64 / 9600;
            task::sleep(Duration::from_millis(millis)).await;
        }
    }
    Ok(())
}

#[derive(Debug)]
enum Request {
    None,
    Meta {
        dst: String,
        hash: String,
    },
    Get {
        dst: String,
        #[allow(dead_code)]
        frequency: String,
        tag: u16,
        #[allow(dead_code)]
        existing: u32,
        id: String,
    },
}

lazy_static! {
    static ref GET_RE: Regex = Regex::new(r"G (\d+) ([^ ]+) (\d+) (\w+)").unwrap();
    static ref META_RE: Regex = Regex::new(r"M (\w+)").unwrap();
}

fn parse_request(src: &str, s: &str) -> Result<Request, Box<dyn std::error::Error>> {
    if let Some(m) = GET_RE.captures(s) {
        let tag = match m[1].parse::<u16>() {
            Ok(x) => x,
            _ => {
                println!("Tag is not u16");
                return Ok(Request::None);
            }
        };
        let existing = match m[2].parse::<u32>() {
            Ok(x) => x,
            _ => {
                println!("`existing` is not u32");
                return Ok(Request::None);
            }
        };
        println!("Got request from {} {:?}", &src, s);
        return Ok(Request::Get {
            dst: src.to_string(),
            frequency: m[1].to_string(),
            tag,
            existing,
            id: m[4].to_string(),
        });
    }
    if let Some(m) = META_RE.captures(s) {
        return Ok(Request::Meta {
            dst: src.to_string(),
            hash: m[1].to_string(),
        });
    }
    Ok(Request::None)
}

/*
* TODO: merge with exact duplicate in downloader
*/
async fn make_packet(
    parser: &mut Ax25ParserClient<tonic::transport::Channel>,
    dst: &str,
    src: &str,
    payload: String,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let req = tonic::Request::new(ax25::SerializeRequest {
        packet: Some(ax25::Packet {
            dst: dst.to_string(), // TODO: set callsign.
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

async fn handle_meta(
    client: &mut RouterServiceClient<tonic::transport::Channel>,
    parser: &mut Ax25ParserClient<tonic::transport::Channel>,
    block: &[u8],
    dst: &str,
    src: String,
    hash: String,
    packet_size: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let size = block.len();
    let max_source_symbols = max_source_syms(size, packet_size);
    let reply = make_packet(
        parser,
        dst,
        &src,
        format!("m {} {} {}", hash, max_source_symbols, size),
    )
    .await?;
    client
        .send(tonic::Request::new(ax25ms::SendRequest {
            frame: Some(ax25ms::Frame { payload: reply }),
        }))
        .await?;
    Ok(())
}

async fn handle_get(
    client: &mut RouterServiceClient<tonic::transport::Channel>,
    parser: &mut Ax25ParserClient<tonic::transport::Channel>,
    block: &[u8],
    dst: &str,
    src: String,
    tag: String,
    packet_size: usize,
    nb_repair: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Handling GET");
    let mut source_data = block.to_vec();

    // Pad to nearest payload size.
    let max_source_symbols = max_source_syms(block.len(), packet_size);

    let excess = source_data.len() % max_source_symbols;
    let padded_size = if excess == 0 {
        source_data.len()
    } else {
        source_data.len() + max_source_symbols - excess
    };
    source_data.resize(padded_size, 0); // multiple of packet_size.
    let packets = ((source_data.len() / packet_size) as f32 * 1.2 + 2.0) as usize; // TODO: tweak default overhead.
    println!("Sending {} packets", packets);

    transmit(
        client,
        parser,
        dst,
        src,
        tag,
        packet_size,
        nb_repair,
        source_data,
        packets,
    )
    .await
}

#[derive(Debug)]
pub enum UploaderError {
    RPCError(tonic::transport::Error),
    RPCStatusError(tonic::Status),
    IOError(std::io::Error),
    StreamError(Box<dyn std::error::Error>),
    ChecksumMismatch(String, String),
    Timeout,
    HashNotFound,
}
impl From<Box<dyn std::error::Error>> for UploaderError {
    fn from(error: Box<dyn std::error::Error>) -> Self {
        UploaderError::StreamError(error)
    }
}
impl From<tonic::transport::Error> for UploaderError {
    fn from(error: tonic::transport::Error) -> Self {
        UploaderError::RPCError(error)
    }
}
impl From<tonic::Status> for UploaderError {
    fn from(error: tonic::Status) -> Self {
        UploaderError::RPCStatusError(error)
    }
}
impl From<std::io::Error> for UploaderError {
    fn from(error: std::io::Error) -> Self {
        UploaderError::IOError(error)
    }
}

struct File {
    name: String,
    hash: String,
}

pub struct DirectoryIndex {
    files: HashMap<String, File>,
    base: String,
}

impl DirectoryIndex {
    pub fn new(dir: &str) -> Result<DirectoryIndex, UploaderError> {
        let mut files = HashMap::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::metadata(&path)?;
            //let fn = Path::new("./foo.file");
            let hash = sha256::try_digest(path.as_path()).unwrap();
            let fname = path.file_name().unwrap().to_str().unwrap();
            if metadata.is_file() {
                files.insert(
                    hash.clone(),
                    File {
                        name: fname.to_string(),
                        hash: hash.clone(),
                    },
                );
            }
            println!("Indexed file: {} {}", hash, fname);
        }
        Ok(DirectoryIndex {
            base: dir.to_string(),
            files: files,
        })
    }

    pub fn get_block(&self, hash: &str) -> Result<Vec<u8>, UploaderError> {
        match self.files.get(hash) {
            Some(f) => {
                println!("Found hash {} at {}", hash, f.name);
                Ok(fs::read(std::path::Path::new(&self.base).join(&f.name))?)
            }
            None => Err(UploaderError::HashNotFound),
        }
    }
}

fn get_block(id: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    //match blockmap.get(id.as_str()) {
    let connection = rusqlite::Connection::open("blockmap.sqlite").unwrap();
    let mut stmt = connection.prepare("SELECT data FROM blocks WHERE id=?")?;

    let data: Vec<u8> = stmt.query_row([&id], |row| Ok(row.get(0).unwrap()))?;
    println!("Read data len {}", data.len());
    Ok(data)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opt = Opt::from_args();

    let index = DirectoryIndex::new(&opt.input).unwrap();

    println!("Running…");
    let mut client = RouterServiceClient::connect(opt.router).await?;
    let mut parser = Ax25ParserClient::connect(opt.parser).await?;

    println!("Awaiting requests…");
    let mut stream = client
        .stream_frames(ax25ms::StreamRequest {})
        .await
        .unwrap()
        .into_inner();

    let nb_repair = u32::try_from(opt.repair).unwrap();
    loop {
        let (src, req) = get_request(&mut stream, &mut parser).await?;
        match parse_request(&src, &req) {
            Ok(Request::None) => continue,
            Ok(Request::Get {
                dst,
                frequency: _,
                tag,
                existing: _,
                id,
            }) => match index.get_block(&id) {
                Ok(block) => {
                    handle_get(
                        &mut client,
                        &mut parser,
                        &block,
                        &dst,
                        opt.source.clone(),
                        tag.to_string(),
                        opt.size,
                        nb_repair,
                    )
                    .await?;
                }
                Err(e) => {
                    println!("Unknown block {}: {:?}", id, e);
                }
            },
            Ok(Request::Meta { dst, hash }) => match index.get_block(&hash) {
                Ok(block) => {
                    handle_meta(
                        &mut client,
                        &mut parser,
                        &block,
                        &dst,
                        opt.source.clone(),
                        hash,
                        opt.size,
                    )
                    .await?;
                }
                Err(e) => {
                    println!("Unknown block {}: {:?}", hash, e);
                }
            },
            Err(_x) => continue,
        };
        //println!("Request: {:?}", preq);
    }
}
