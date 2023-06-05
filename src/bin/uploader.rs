use async_std::task;
use ax25::ax25_parser_client::Ax25ParserClient;
use ax25::packet::Ui;
use ax25::Packet;
use ax25::SerializeRequest;
use ax25ms::router_service_client::RouterServiceClient;
use ax25ms::Frame;
use ax25ms::SendRequest;
use lazy_static::lazy_static;
use log::{debug, info, warn};
use rand::prelude::SliceRandom;
use rand::thread_rng;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::time::Duration;
use structopt::StructOpt;
use tokio_stream::StreamExt;

use lib::{ax25, ax25ms, make_packet};

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

        let cmd = match str::from_utf8(&ui.payload) {
            Ok(x) => x,
            _ => {
                //warn!("Invalid request: {:?}", ui.payload);
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
    tag: u16,
    packet_size: usize,
    nb_repair: u32,
    source_data: Vec<u8>,
    packets: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let len = source_data.len();

    let max_source_symbols = max_source_syms(source_data.len(), packet_size);

    debug!("Max source symbols: {}", max_source_symbols);
    debug!("Total len: {}", len);
    // State that needs to be sent:
    // * max_source_symbols
    // * total size
    // * packet_size is implicitly sent by seeing the block size.

    let mut encoder = raptor_code::SourceBlockEncoder::new(&source_data, max_source_symbols);
    let n = (encoder.nb_source_symbols() + nb_repair) as u16;

    // Transmit RPC.

    debug!("Total chunks: {}", n);
    let txlist = {
        let mut x: Vec<u16> = (0..n).step_by(1).collect();
        x.shuffle(&mut thread_rng());
        x.resize(packets, 0);
        x
    };

    for esi in txlist {
        let encoding_symbol = encoder.fountain(esi as u32);
        let len = encoding_symbol.len();

        let mut payload = tag.to_be_bytes().to_vec();
        payload.extend(esi.to_be_bytes().to_vec());
        payload.extend(encoding_symbol);
        // TODO: surely we can default these values?
        let request = tonic::Request::new(SerializeRequest {
            set_fcs: true,
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
                    pid: 0xF0_i32,
                    push: 0,
                    payload,
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
    List {
        dst: String,
        tag: u16,
    },
}

lazy_static! {
    //                                      cmd    tag   freq    exist hash
    static ref GET_RE: Regex = Regex::new(r"(G|GM) (\d+) ([^ ]+) (\d+) (\w+)").unwrap();
    static ref META_RE: Regex = Regex::new(r"M (\w+)").unwrap();
    static ref LIST_RE: Regex = Regex::new(r"L (\d+)").unwrap();
}

fn parse_get_request(
    cmd: &str,
    dst: &str,
    frequency: &str,
    tag: &str,
    existing: &str,
    hash: &str,
) -> Result<Vec<Request>, Box<dyn std::error::Error>> {
    let tag = match tag.parse::<u16>() {
        Ok(x) => x,
        _ => {
            warn!("Tag is not u16");
            return Ok(vec![]);
        }
    };
    let existing = match existing.parse::<u32>() {
        Ok(x) => x,
        _ => {
            warn!("`existing` is not u32");
            return Ok(vec![]);
        }
    };
    let g = Request::Get {
        dst: dst.to_string(),
        frequency: frequency.to_string(),
        tag,
        existing,
        id: hash.to_string(),
    };
    if cmd == "G" {
        return Ok(vec![g]);
    }
    if cmd == "GM" {
        let m = Request::Meta {
            dst: dst.to_string(),
            hash: hash.to_string(),
        };
        return Ok(vec![m, g]);
    }
    Ok(vec![])
}

fn parse_request(src: &str, s: &str) -> Result<Vec<Request>, Box<dyn std::error::Error>> {
    if let Some(m) = GET_RE.captures(s) {
        info!("Got request from {} {:?}", &src, s);
        return parse_get_request(
            &m[1], /* cmd */
            src, &m[3], /* frequency */
            &m[2], /* tag */
            &m[4], /* existing */
            &m[5], /* hash */
        );
    }
    if let Some(m) = META_RE.captures(s) {
        return Ok(vec![Request::Meta {
            dst: src.to_string(),
            hash: m[1].to_string(),
        }]);
    }
    if let Some(m) = LIST_RE.captures(s) {
        let tag = match m[1].parse::<u16>() {
            Ok(x) => x,
            _ => {
                warn!("Tag is not u16");
                return Ok(vec![]);
            }
        };
        return Ok(vec![Request::List {
            dst: src.to_string(),
            tag,
        }]);
    }
    Ok(vec![])
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
    tag: u16,
    packet_size: usize,
    nb_repair: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    debug!("Handling GET");

    // Pad to nearest payload size.
    let max_source_symbols = max_source_syms(block.len(), packet_size);

    let source_data = {
        let mut x = block.to_vec();

        let excess = x.len() % max_source_symbols;
        let padded_size = if excess == 0 {
            x.len()
        } else {
            x.len() + max_source_symbols - excess
        };
        x.resize(padded_size, 0); // multiple of packet_size.
        x
    };
    let packets = ((source_data.len() / packet_size) as f32 * 1.2 + 2.0) as usize; // TODO: tweak default overhead.
    debug!("Sending {} packets", packets);

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
}

/// Block index created from a directory of files.
///
/// No recursive scanning, just files directly in the directory.
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
                    },
                );
            }
            info!("Indexed file: {} {}", hash, fname);
        }
        Ok(DirectoryIndex {
            base: dir.to_string(),
            files,
        })
    }

    pub fn get_block(&self, hash: &str) -> Result<Vec<u8>, UploaderError> {
        match self.files.get(hash) {
            Some(f) => {
                info!("Found hash {} at {}", hash, f.name);
                Ok(fs::read(std::path::Path::new(&self.base).join(&f.name))?)
            }
            None => Err(UploaderError::HashNotFound),
        }
    }

    pub fn list(&self) -> Vec<FileEntry> {
        let mut ret = Vec::new();
        for (hash, f) in self.files.iter() {
            ret.push(FileEntry {
                name: f.name.clone(),
                hash: hash.to_owned(),
            });
        }
        ret
    }
}

pub struct FileEntry {
    name: String,
    hash: String,
}

/// Get a block from sqlite database.
///
/// This is not used at the moment, but I have plans! :-)
#[allow(dead_code)]
fn get_block(id: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    //match blockmap.get(id.as_str()) {
    let connection = rusqlite::Connection::open("blockmap.sqlite").unwrap();
    let mut stmt = connection.prepare("SELECT data FROM blocks WHERE id=?")?;

    let data: Vec<u8> = stmt.query_row([&id], |row| Ok(row.get(0).unwrap()))?;
    info!("Read data len {}", data.len());
    Ok(data)
}

async fn process_requests(
    client: &mut RouterServiceClient<tonic::transport::Channel>,
    parser: &mut Ax25ParserClient<tonic::transport::Channel>,
    opt: &Opt,
    index: &DirectoryIndex,
    reqs: &[Request],
) -> Result<(), UploaderError> {
    let nb_repair = u32::try_from(opt.repair).unwrap();
    for r in reqs {
        match r {
            Request::Get {
                dst,
                frequency: _,
                tag,
                existing: _,
                id,
            } => match index.get_block(id) {
                Ok(block) => {
                    handle_get(
                        client,
                        parser,
                        &block,
                        dst,
                        opt.source.clone(),
                        *tag,
                        opt.size,
                        nb_repair,
                    )
                    .await?;
                }
                Err(e) => {
                    warn!("Unknown block {}: {:?}", id, e);
                }
            },
            Request::Meta { dst, hash } => match index.get_block(hash) {
                Ok(block) => {
                    handle_meta(
                        client,
                        parser,
                        &block,
                        dst,
                        opt.source.clone(),
                        hash.to_string(),
                        opt.size,
                    )
                    .await?;
                }
                Err(e) => {
                    warn!("Unknown block {}: {:?}", hash, e);
                }
            },
            Request::List { dst, tag } => {
                // TODO: support longer file listings.
                let mut txt = format!("l {}\n", tag);
                for f in index.list() {
                    txt.push_str(&format!("{} {}\n", f.hash, f.name));
                }
                let reply = make_packet(parser, dst, &opt.source, txt).await?;
                client
                    .send(tonic::Request::new(ax25ms::SendRequest {
                        frame: Some(ax25ms::Frame { payload: reply }),
                    }))
                    .await?;
                let reply = make_packet(parser, dst, &opt.source, format!("l {}", tag)).await?;
                client
                    .send(tonic::Request::new(ax25ms::SendRequest {
                        frame: Some(ax25ms::Frame { payload: reply }),
                    }))
                    .await?;
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), UploaderError> {
    let opt = Opt::from_args();

    stderrlog::new()
        .module(module_path!())
        .quiet(false)
        .verbosity(3)
        .timestamp(stderrlog::Timestamp::Second)
        .init()
        .unwrap();

    let index = DirectoryIndex::new(&opt.input).unwrap();

    info!("Running…");
    let mut client = RouterServiceClient::connect(opt.router.clone()).await?;
    let mut parser = Ax25ParserClient::connect(opt.parser.clone()).await?;

    info!("Awaiting requests…");
    let mut stream = client
        .stream_frames(ax25ms::StreamRequest {})
        .await?
        .into_inner();

    loop {
        let (src, req) = get_request(&mut stream, &mut parser).await?;

        match parse_request(&src, &req) {
            Ok(reqs) => {
                process_requests(&mut client, &mut parser, &opt, &index, &reqs).await?;
            }
            Err(e) => {
                debug!("Unknown block: {:?}", e);
            }
        }
    }
}
