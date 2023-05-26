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
use std::convert::TryFrom;
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
) -> Result<String, Box<dyn std::error::Error>> {
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
        return Ok(cmd.to_string());
    }
}

async fn transmit(
    client: &mut RouterServiceClient<tonic::transport::Channel>,
    parser: &mut Ax25ParserClient<tonic::transport::Channel>,
    src: String,
    packet_size: usize,
    nb_repair: u32,
    source_data: Vec<u8>,
    packets: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let len = source_data.len();

    let f =
        (usize_to_float(source_data.len()).unwrap() / usize_to_float(packet_size).unwrap()).ceil();
    let max_source_symbols = float_to_usize(f).unwrap();

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
                dst: "CQ".to_string(), // TODO: Set to callsign requesting.
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
        if false {
            let millis: u64 = 8000 * len as u64 / 9600;
            task::sleep(Duration::from_millis(millis)).await;
        }
    }
    Ok(())
}

#[derive(Debug)]
enum Request {
    None,
    Get {
        frequency: String,
        existing: u32,
        id: String,
    },
}

lazy_static! {
    static ref GET_RE: Regex = Regex::new(r"G ([^ ]+) (\d+) (\w+)").unwrap();
}

fn parse_request(s: &str) -> Result<Request, Box<dyn std::error::Error>> {
    if let Some(m) = GET_RE.captures(s) {
        if let Ok(existing) = m[2].parse::<u32>() {
            return Ok(Request::Get {
                frequency: m[1].to_string(),
                existing,
                id: m[3].to_string(),
            });
        }
    }
    Ok(Request::None)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opt = Opt::from_args();

    println!("Running…");
    let mut client = RouterServiceClient::connect(opt.router).await?;
    let mut parser = Ax25ParserClient::connect(opt.parser).await?;

    println!("Awaiting requests…");
    let mut stream = client
        .stream_frames(ax25ms::StreamRequest {})
        .await
        .unwrap()
        .into_inner();

    loop {
        let req = get_request(&mut stream, &mut parser).await?;
        let preq = match parse_request(&req) {
            Ok(Request::None) => continue,
            Ok(Request::Get {
                frequency,
                existing,
                id,
            }) => Request::Get {
                frequency,
                existing,
                id,
            },
            _ => continue,
        };
        println!("Request: {:?}", preq);

        let mut source_data = fs::read(&opt.input).expect("read data");
        // TODO: Stop this hardcoding.
        source_data.resize(3686, 0);
        let packet_size = opt.size;
        let packets = 100;
        let nb_repair = u32::try_from(opt.repair).unwrap();

        transmit(
            &mut client,
            &mut parser,
            opt.source.clone(),
            packet_size,
            nb_repair,
            source_data,
            packets,
        )
        .await?;
    }
}
