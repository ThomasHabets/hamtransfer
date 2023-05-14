use async_std::task;
use ax25::ax25_parser_client::Ax25ParserClient;
use ax25::packet::Ui;
use ax25::Packet;
use ax25::SerializeRequest;
use ax25ms::router_service_client::RouterServiceClient;
use ax25ms::Frame;
use ax25ms::SendRequest;
use rand::prelude::SliceRandom;
use rand::thread_rng;
use std::fs;
use std::time::Duration;

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    //let mut source_data: Vec<u8> = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    let mut source_data = fs::read("axpipe.cc").expect("read data");
    source_data.resize(3686, 0);
    let len = source_data.len();
    //source_data.resize(len, 0);
    //source_data[999] = 98;
    //source_data[len - 1] = 33;
    //println!("{:?}", source_data);

    let packet_size = 200;

    let nb_repair = 50;

    let f =
        (usize_to_float(source_data.len()).unwrap() / usize_to_float(packet_size).unwrap()).ceil();
    let max_source_symbols = float_to_usize(f).unwrap();

    println!("Max source symbols: {}", max_source_symbols);
    println!("Total len: {}", len);
    // State that needs to be sent:
    // * max_source_symbols
    // * total size
    // * max_source_symbols is implicitly sent by seeing the block size.

    let mut encoder = raptor_code::SourceBlockEncoder::new(&source_data, max_source_symbols);
    let n = encoder.nb_source_symbols() + nb_repair;

    // Transmit RPC.
    let mut client = RouterServiceClient::connect("http://[::1]:12001").await?;
    let mut parser = Ax25ParserClient::connect("http://[::1]:12001").await?;

    println!("Total chunks: {}", n);
    let mut txlist: Vec<u32> = (0..n).step_by(1).collect();
    txlist.shuffle(&mut thread_rng());

    for esi in txlist {
        let encoding_symbol = encoder.fountain(esi);
        let len = encoding_symbol.len();

        // Test code that writes it to disk.
        //
        //println!("{:?}", encoding_symbol.len());
        fs::write(format!("tmp/{}", esi), &encoding_symbol).expect("write block");

        // TODO: surely we can default these values?
        let request = tonic::Request::new(SerializeRequest {
            packet: Some(Packet {
                dst: "CQ".to_string(),
                src: "M0THC-1".to_string(),
                fcs: 0,
                aprs: None,
                repeater: vec![],
                command_response: false,
                command_response_la: true,
                rr_dst1: false,
                rr_extseq: false,
                frame_type: Some(ax25::packet::FrameType::Ui(Ui {
                    pid: esi as i32,
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
        println!("Sent esi {} of size {}", esi, &len);
        let millis: u64 = 8000 * len as u64 / 9600;
        task::sleep(Duration::from_millis(millis)).await;
        //task::sleep(Duration::from_secs(1)).await;
    }
    Ok(())
}
